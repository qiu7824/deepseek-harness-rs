//! Content-block structure helpers. Rust port of
//! `packages/llm/llm/src/content.ts`.

use crate::message::Message;
use crate::types::{ContentBlock, ImageAttachmentRef};

/// Model-facing stand-in for an image removed to fit a provider request bound.
pub const OFFLOADED_IMAGE_TEXT: &str = "[image omitted to keep the request within its image limit; older images are omitted first. If this image is still needed, read its file again when a path is available; otherwise ask the user to attach it again.]";

/// Representation used to account one request image's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestImageRepresentation {
    Raw,
    Base64,
}

/// Byte accounting and quantized removal policy for one request representation.
pub struct RequestImageOffloadPolicy<'a> {
    pub max_images: Option<usize>,
    pub max_bytes: Option<u64>,
    pub count_quantum: Option<usize>,
    pub byte_quantum: Option<u64>,
    pub representation: RequestImageRepresentation,
    pub byte_length: Option<&'a dyn Fn(&ImageAttachmentRef) -> u64>,
}

/// True when typed model content contains an image block, walking nested
/// tool-result content (TS `contentHasImage`).
pub fn content_has_image(content: &[ContentBlock]) -> bool {
    content.iter().any(|block| match block {
        ContentBlock::Image { .. } => true,
        ContentBlock::ToolResult { content, .. } => content_has_image(content),
        _ => false,
    })
}

fn base64_length(bytes: u64) -> u64 {
    bytes.div_ceil(3).saturating_mul(4)
}

fn collect_image_lengths(
    blocks: &[ContentBlock],
    lengths: &mut Vec<u64>,
    policy: &RequestImageOffloadPolicy<'_>,
) {
    for block in blocks {
        match block {
            ContentBlock::Image { attachment } => {
                let bytes = policy.byte_length.map_or_else(
                    || attachment.bytes.unwrap_or(0),
                    |length| length(attachment),
                );
                lengths.push(match policy.representation {
                    RequestImageRepresentation::Raw => bytes,
                    RequestImageRepresentation::Base64 => base64_length(bytes),
                });
            }
            ContentBlock::ToolResult { content, .. } => {
                collect_image_lengths(content, lengths, policy);
            }
            _ => {}
        }
    }
}

fn replace_oldest_images(blocks: &[ContentBlock], remaining: &mut usize) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Image { .. } if *remaining > 0 => {
                *remaining -= 1;
                ContentBlock::Text {
                    text: OFFLOADED_IMAGE_TEXT.to_string(),
                }
            }
            ContentBlock::ToolResult {
                tool_call_id,
                content,
                is_error,
            } if *remaining > 0 => ContentBlock::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: replace_oldest_images(content, remaining),
                is_error: *is_error,
            },
            _ => block.clone(),
        })
        .collect()
}

/// Return transient request messages whose oldest images are replaced until
/// their accumulated base64 payload fits the configured bound.
pub fn offload_request_images(
    messages: &[Message],
    max_request_image_bytes: Option<u64>,
) -> Vec<Message> {
    offload_request_images_with_policy(
        messages,
        &RequestImageOffloadPolicy {
            max_images: None,
            max_bytes: max_request_image_bytes,
            count_quantum: None,
            byte_quantum: Some(1),
            representation: RequestImageRepresentation::Base64,
            byte_length: None,
        },
    )
}

/// Return a deterministic transient projection whose oldest images are
/// replaced in whole count and byte quanta after a route budget is exceeded.
pub fn offload_request_images_with_policy(
    messages: &[Message],
    policy: &RequestImageOffloadPolicy<'_>,
) -> Vec<Message> {
    let mut lengths = Vec::new();
    for message in messages {
        collect_image_lengths(&message.content, &mut lengths, policy);
    }
    let total = lengths.iter().copied().sum::<u64>();
    let excess_count = policy
        .max_images
        .map_or(0, |max| lengths.len().saturating_sub(max));
    let excess_bytes = policy.max_bytes.map_or(0, |max| total.saturating_sub(max));
    if excess_count == 0 && excess_bytes == 0 {
        return messages.to_vec();
    }

    let count_quantum = policy.count_quantum.unwrap_or(1);
    let byte_quantum = policy.byte_quantum.unwrap_or(1);
    let remove_count = if excess_count == 0 {
        0
    } else {
        excess_count.div_ceil(count_quantum) * count_quantum
    };
    let remove_bytes = if excess_bytes == 0 {
        0
    } else {
        excess_bytes.div_ceil(byte_quantum) * byte_quantum
    };
    let mut count = 0;
    let mut removed_bytes = 0_u64;
    for image_bytes in lengths {
        let byte_target_met = remove_bytes == 0
            || if byte_quantum == 1 {
                removed_bytes >= remove_bytes
            } else {
                removed_bytes > remove_bytes
            };
        if count >= remove_count && byte_target_met {
            break;
        }
        removed_bytes = removed_bytes.saturating_add(image_bytes);
        count += 1;
    }

    let mut remaining = count;
    messages
        .iter()
        .map(|message| {
            let mut projected = message.clone();
            projected.content = replace_oldest_images(&message.content, &mut remaining);
            projected
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, MessageSource, Role};
    use crate::types::ImageAttachmentRef;

    fn image(id: &str, bytes: u64) -> ContentBlock {
        ContentBlock::Image {
            attachment: ImageAttachmentRef {
                attachment_id: id.to_string(),
                media_type: Some("image/png".to_string()),
                bytes: Some(bytes),
                width: Some(1),
                height: Some(1),
                name: None,
            },
        }
    }

    fn message(id: &str, content: Vec<ContentBlock>) -> Message {
        Message {
            id: crate::brand::message_id(id),
            role: Role::User,
            content,
            source: MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        }
    }

    fn count_images(blocks: &[ContentBlock]) -> usize {
        blocks
            .iter()
            .map(|block| match block {
                ContentBlock::Image { .. } => 1,
                ContentBlock::ToolResult { content, .. } => count_images(content),
                _ => 0,
            })
            .sum()
    }

    fn count_offloaded(blocks: &[ContentBlock]) -> usize {
        blocks
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } if text == OFFLOADED_IMAGE_TEXT => 1,
                ContentBlock::ToolResult { content, .. } => count_offloaded(content),
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn walks_nested_tool_result_content() {
        assert!(!content_has_image(&[ContentBlock::Text {
            text: "hi".to_string()
        }]));
        assert!(content_has_image(&[ContentBlock::ToolResult {
            tool_call_id: crate::brand::call_id("c1"),
            content: vec![image("img", 1)],
            is_error: None,
        }]));
    }

    #[test]
    fn base64_budget_replaces_oldest_nested_image_without_mutating_history() {
        let messages = vec![
            message(
                "m1",
                vec![ContentBlock::ToolResult {
                    tool_call_id: crate::brand::call_id("shot"),
                    content: vec![image("old", 3)],
                    is_error: None,
                }],
            ),
            message("m2", vec![image("newer", 3), image("newest", 3)]),
        ];
        let durable = messages.clone();

        let projected = offload_request_images(&messages, Some(8));

        assert_eq!(count_offloaded(&projected[0].content), 1);
        assert_eq!(count_images(&projected[1].content), 2);
        assert_eq!(messages, durable);
    }

    #[test]
    fn raw_byte_quantum_uses_strict_boundary_and_stable_prefix_steps() {
        const MIB: u64 = 1024 * 1024;
        let project = |count: usize| {
            let messages = vec![message(
                "m",
                (0..count)
                    .map(|index| image(&index.to_string(), MIB))
                    .collect(),
            )];
            offload_request_images_with_policy(
                &messages,
                &RequestImageOffloadPolicy {
                    representation: RequestImageRepresentation::Raw,
                    max_images: None,
                    max_bytes: Some(128 * MIB),
                    count_quantum: None,
                    byte_quantum: Some(64 * MIB),
                    byte_length: None,
                },
            )
        };

        assert_eq!(count_images(&project(128)[0].content), 128);
        assert_eq!(count_offloaded(&project(129)[0].content), 65);
        assert_eq!(count_offloaded(&project(192)[0].content), 65);
        assert_eq!(count_offloaded(&project(193)[0].content), 129);
    }

    #[test]
    fn count_excess_rounds_up_to_count_quantum() {
        let messages = vec![message(
            "m",
            (0..601).map(|index| image(&index.to_string(), 1)).collect(),
        )];
        let projected = offload_request_images_with_policy(
            &messages,
            &RequestImageOffloadPolicy {
                representation: RequestImageRepresentation::Raw,
                max_images: Some(600),
                max_bytes: None,
                count_quantum: Some(20),
                byte_quantum: None,
                byte_length: None,
            },
        );
        assert_eq!(count_offloaded(&projected[0].content), 20);
        assert_eq!(count_images(&projected[0].content), 581);
    }

    #[test]
    fn route_owned_lengths_and_both_budgets_select_one_oldest_prefix() {
        let messages = vec![message(
            "m",
            vec![image("old", 100), image("middle", 100), image("new", 100)],
        )];
        let length = |_image: &ImageAttachmentRef| 2;
        let projected = offload_request_images_with_policy(
            &messages,
            &RequestImageOffloadPolicy {
                representation: RequestImageRepresentation::Raw,
                max_images: Some(2),
                max_bytes: Some(3),
                count_quantum: Some(1),
                byte_quantum: Some(1),
                byte_length: Some(&length),
            },
        );
        assert_eq!(count_offloaded(&projected[0].content), 2);
        assert_eq!(count_images(&projected[0].content), 1);
    }

    #[test]
    fn base64_accounting_includes_padding() {
        let exact = vec![message("m", vec![image("a", 3), image("b", 3)])];
        assert_eq!(offload_request_images(&exact, Some(8)), exact);

        let over = vec![message("m", vec![image("a", 4), image("b", 3)])];
        let projected = offload_request_images(&over, Some(8));
        assert_eq!(count_offloaded(&projected[0].content), 1);
        assert_eq!(count_images(&projected[0].content), 1);
    }
}
