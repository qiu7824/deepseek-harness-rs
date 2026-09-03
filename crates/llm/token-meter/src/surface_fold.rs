//! The measurement service's positional surface fold. Rust port of
//! `packages/llm/token-meter/src/surface-fold.ts`.

use dsh_llm::{ContentBlock, ImageAttachmentRef};
use dsh_session::surface::is_surface_event;
use dsh_session::{SessionEvent, derive_event_message};

use crate::estimate::estimate_message;

/// Internal node facts retained without committing to a provider route.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterSurfaceNode {
    pub seq: u64,
    pub heuristic_tokens: u64,
    pub image_free_tokens: u64,
    pub images: Vec<ImageAttachmentRef>,
}

/// One surface event's placement and fixed-heuristic price.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceTokenFold {
    pub tokens: u64,
    pub nodes: Vec<MeterSurfaceNode>,
}

fn collect_images(blocks: &[ContentBlock], images: &mut Vec<ImageAttachmentRef>) {
    for block in blocks {
        match block {
            ContentBlock::Image { attachment } => images.push(attachment.clone()),
            ContentBlock::ToolResult { content, .. } => collect_images(content, images),
            _ => {}
        }
    }
}

fn without_images(blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Image { .. } => None,
            ContentBlock::ToolResult {
                tool_call_id,
                content,
                is_error,
            } => Some(ContentBlock::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: without_images(content),
                is_error: *is_error,
            }),
            other => Some(other.clone()),
        })
        .collect()
}

fn node_for(event: &SessionEvent) -> MeterSurfaceNode {
    let message = derive_event_message(event);
    let heuristic_tokens = message.as_ref().map(estimate_message).unwrap_or(0);
    let mut images = Vec::new();
    let image_free_tokens = message
        .as_ref()
        .map(|message| {
            collect_images(&message.content, &mut images);
            let mut projected = message.clone();
            projected.content = without_images(&message.content);
            estimate_message(&projected)
        })
        .unwrap_or(0);
    MeterSurfaceNode {
        seq: event.seq.get(),
        heuristic_tokens,
        image_free_tokens,
        images,
    }
}

/// Fold one surface event onto a route-neutral surface.
pub fn fold_surface_tokens(
    nodes: &[MeterSurfaceNode],
    event: &SessionEvent,
) -> Result<SurfaceTokenFold, String> {
    if !is_surface_event(event) {
        return Ok(SurfaceTokenFold {
            tokens: 0,
            nodes: nodes.to_vec(),
        });
    }
    let node = node_for(event);
    match &event.surface_op {
        None => Ok(SurfaceTokenFold {
            tokens: node.heuristic_tokens,
            nodes: nodes.to_vec(),
        }),
        Some(dsh_session::SurfaceOp::Append) => {
            let mut next = nodes.to_vec();
            next.push(node.clone());
            Ok(SurfaceTokenFold {
                tokens: node.heuristic_tokens,
                nodes: next,
            })
        }
        Some(dsh_session::SurfaceOp::Replace { start, end }) => {
            let start_idx = nodes.iter().position(|existing| existing.seq == *start);
            let end_idx = nodes.iter().position(|existing| existing.seq == *end);
            let (Some(start_idx), Some(end_idx)) = (start_idx, end_idx) else {
                return Err(format!(
                    "token surface: replace at seq {} has invalid current range {start}-{end}",
                    event.seq
                ));
            };
            if start_idx > end_idx {
                return Err(format!(
                    "token surface: replace at seq {} has invalid current range {start}-{end}",
                    event.seq
                ));
            }
            let mut next = nodes.to_vec();
            next.splice(start_idx..=end_idx, [node.clone()]);
            Ok(SurfaceTokenFold {
                tokens: node.heuristic_tokens,
                nodes: next,
            })
        }
    }
}
