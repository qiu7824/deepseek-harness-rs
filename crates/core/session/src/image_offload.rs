//! Durable, occurrence-based image omissions. Immutable message events retain
//! their attachments; only the model-request projection substitutes text.
use crate::{Session, SessionEvent};
use dsh_llm::{ContentBlock, Message};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
fn image_count(blocks: &[ContentBlock]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Image { offloaded, .. } => usize::from(*offloaded != Some(true)),
            ContentBlock::ToolResult { content, .. } => image_count(content),
            _ => 0,
        })
        .sum()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputImages {
    pub count: usize,
    pub omitted: BTreeSet<usize>,
}

pub(crate) fn input_images(event: &SessionEvent) -> Option<InputImages> {
    if !matches!(event.type_.as_str(), "user/message" | "tool/result") {
        return None;
    }
    fn visit(blocks: &[ContentBlock], images: &mut InputImages) {
        for block in blocks {
            match block {
                ContentBlock::Image { offloaded, .. } => {
                    if *offloaded == Some(true) {
                        images.omitted.insert(images.count);
                    }
                    images.count += 1;
                }
                ContentBlock::ToolResult { content, .. } => visit(content, images),
                _ => {}
            }
        }
    }
    let message = crate::surface::derive_event_message(event)?;
    let mut images = InputImages {
        count: 0,
        omitted: BTreeSet::new(),
    };
    visit(&message.content, &mut images);
    Some(images)
}
pub(crate) type Targets = Vec<(u64, Vec<usize>)>;
pub(crate) fn validate(
    data: &Value,
    nodes: &[u64],
    counts: &BTreeMap<u64, usize>,
    omitted: &BTreeMap<u64, BTreeSet<usize>>,
) -> Result<Targets, String> {
    if data.as_object().is_none_or(|map| map.len() != 1) {
        return Err("image/offload requires only targets".into());
    }
    let targets = data["targets"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or("image/offload requires nonempty targets")?;
    let mut unique = BTreeSet::new();
    let mut result = Vec::new();
    for target in targets {
        if target.as_object().is_none_or(|map| map.len() != 2) {
            return Err("image/offload target requires seq and imageIndexes".into());
        }
        let seq = target["seq"]
            .as_u64()
            .filter(|s| nodes.contains(s) && unique.insert(*s))
            .ok_or("image/offload requires unique current input nodes")?;
        let count = counts
            .get(&seq)
            .ok_or("image/offload target must be an input message")?;
        let indexes = target["imageIndexes"]
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or("image/offload requires nonempty imageIndexes")?;
        let mut selected = Vec::new();
        let mut previous = None;
        for index in indexes {
            let index = index
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .filter(|v| {
                    *v < *count
                        && previous.is_none_or(|p| *v > p)
                        && !omitted.get(&seq).is_some_and(|set| set.contains(v))
                })
                .ok_or(
                    "image/offload indexes must be increasing, present, and not already omitted",
                )?;
            selected.push(index);
            previous = Some(index);
        }
        result.push((seq, selected));
    }
    Ok(result)
}
pub(crate) fn project(mut message: Message, selected: &BTreeSet<usize>) -> Message {
    fn visit(blocks: &mut [ContentBlock], selected: &BTreeSet<usize>, index: &mut usize) {
        for block in blocks {
            match block {
                ContentBlock::Image { offloaded, .. } => {
                    let omit = selected.contains(index);
                    *index += 1;
                    if omit {
                        *offloaded = Some(true);
                    }
                }
                ContentBlock::ToolResult { content, .. } => visit(content, selected, index),
                _ => {}
            }
        }
    }
    visit(&mut message.content, selected, &mut 0);
    message
}
impl Session {
    pub fn offload_oldest_images(&self, count: usize) -> Result<bool, String> {
        self.offload_images_in_nodes(count, None)
    }
    pub fn offload_images_in_nodes(
        &self,
        count: usize,
        selected_nodes: Option<&[u64]>,
    ) -> Result<bool, String> {
        if count == 0 {
            return Ok(false);
        }
        let mut remaining = count;
        let nodes = selected_nodes
            .map(|nodes| nodes.to_vec())
            .unwrap_or(self.surface()?.nodes);
        let mut omitted = BTreeMap::<u64, BTreeSet<usize>>::new();
        self.visit_events(0, None, |event| {
            if event.type_ == "image/offload" {
                if let Some(targets) = event.data["targets"].as_array() {
                    for target in targets {
                        if let Some(seq) = target["seq"].as_u64() {
                            omitted.entry(seq).or_default().extend(
                                target["imageIndexes"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|i| i.as_u64().map(|n| n as usize)),
                            );
                        }
                    }
                }
            }
            Ok(true)
        })?;
        let mut targets = Vec::new();
        for seq in nodes {
            let Some(event) = self.read_event(seq)? else {
                continue;
            };
            let Some(images) = input_images(&event) else {
                continue;
            };
            let selected: Vec<_> = (0..images.count)
                .filter(|index| {
                    !images.omitted.contains(index)
                        && !omitted.get(&seq).is_some_and(|set| set.contains(index))
                })
                .take(remaining)
                .collect();
            remaining -= selected.len();
            if !selected.is_empty() {
                targets.push(json!({"seq":seq,"imageIndexes":selected}));
            }
            if remaining == 0 {
                break;
            }
        }
        if targets.is_empty() {
            return Ok(false);
        }
        self.append("image/offload", json!({"targets":targets}), None)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image() -> ContentBlock {
        ContentBlock::Image {
            attachment: serde_json::from_value(
                json!({"attachmentId":"same-original","name":"original.png"}),
            )
            .unwrap(),

            offloaded: None,
        }
    }
    fn session() -> Session {
        let session = Session::create(crate::session_id("offload"), None, None, None).unwrap();
        let message = dsh_llm::create_user_message(
            vec![image(), image()],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        session
            .append(
                "user/message",
                serde_json::to_value(message).unwrap(),
                Some(crate::SurfaceIntent {
                    surface_op: crate::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        session
    }
    #[test]
    fn durable_offload_replays_preserves_originals_and_distinguishes_occurrences() {
        let session = session();
        let original = session.events().as_ref().clone();
        assert!(session.offload_oldest_images(1).unwrap());
        let messages = session.derive_messages().unwrap();
        assert_eq!(image_count(&messages[0].content), 1);
        assert!(matches!(
            &messages[0].content[0],
            ContentBlock::Image {
                offloaded: Some(true),
                ..
            }
        ));
        assert_eq!(session.events()[0], original[0]);
        let events = session.events().as_ref().clone();
        let restored = Session::from_restore(
            crate::session_id("offload"),
            events.clone(),
            session.header(),
            crate::SessionLogOffset::ZERO,
        )
        .unwrap();
        assert_eq!(restored.derive_messages().unwrap(), messages);
        let fork =
            Session::create(crate::session_id("offload-fork"), Some(events), None, None).unwrap();
        assert_eq!(image_count(&fork.derive_messages().unwrap()[0].content), 1);
        assert!(restored.offload_oldest_images(1).unwrap());
        assert_eq!(
            image_count(&restored.derive_messages().unwrap()[0].content),
            0
        );
        assert!(!restored.offload_oldest_images(1).unwrap());
    }
    #[test]
    fn imported_offloaded_tool_images_keep_indexes_and_are_not_selected_again() {
        let session =
            Session::create(crate::session_id("native-images"), None, None, None).unwrap();
        let mut omitted = image();
        if let ContentBlock::Image { offloaded, .. } = &mut omitted {
            *offloaded = Some(true);
        }
        let message = dsh_llm::create_tool_result_message(dsh_llm::ToolResultMessageInput {
            call_id: dsh_llm::call_id("images"),
            content: vec![omitted, image()],
            is_error: false,
        });
        session
            .append(
                "tool/result",
                crate::tool_result_data(1, 1, &message, None, None),
                Some(crate::SurfaceIntent {
                    surface_op: crate::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        let duplicate = json!({"targets":[{"seq":0,"imageIndexes":[0]}]});
        assert!(
            session
                .append("image/offload", duplicate.clone(), None)
                .is_err()
        );
        let mut streaming = crate::surface::StreamingSurfaceFold::default();
        streaming.push(&session.events()[0]).unwrap();
        let invalid = serde_json::from_value(
            json!({"type":"image/offload","seq":1,"time":1,"data":duplicate}),
        )
        .unwrap();
        assert!(streaming.push(&invalid).is_err());
        assert!(session.offload_oldest_images(1).unwrap());
        assert_eq!(
            session.events()[1].data["targets"][0]["imageIndexes"],
            json!([1])
        );
        assert!(!dsh_llm::content_has_image(
            &session.derive_messages().unwrap()[0].content
        ));
        assert!(!session.offload_oldest_images(1).unwrap());
        assert_eq!(
            session.events()[0].data["message"]["content"][1].get("offloaded"),
            None
        );
    }

    #[test]
    fn invalid_or_repeated_targets_are_atomic_failures() {
        let session = session();
        let before = session.events().len();
        for data in [
            json!({"targets":[]}),
            json!({"targets":[{"seq":0,"imageIndexes":[1,0]}]}),
            json!({"targets":[{"seq":0,"imageIndexes":[2]}]}),
            json!({"targets":[{"seq":999,"imageIndexes":[0]}]}),
        ] {
            assert!(session.append("image/offload", data, None).is_err());
            assert_eq!(session.events().len(), before);
        }
        session.offload_oldest_images(1).unwrap();
        assert!(
            session
                .append(
                    "image/offload",
                    json!({"targets":[{"seq":0,"imageIndexes":[0]}]}),
                    None
                )
                .is_err()
        );
        let mut streaming = crate::surface::StreamingSurfaceFold::default();
        for event in session.events().iter() {
            streaming.push(event).unwrap();
        }
        assert_eq!(
            streaming.finish(),
            crate::surface::fold_surface(&session.events()).unwrap()
        );
    }
}
