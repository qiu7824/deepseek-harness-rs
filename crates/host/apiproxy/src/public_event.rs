//! Browser projection of events; adapter replay remains in authoritative logs.
use dsh_session::SessionEvent;
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeStruct},
};
use serde_json::Value;

fn replay_path(event: &SessionEvent) -> Option<&'static [&'static str]> {
    match event.type_.as_str() {
        "assistant/message" if event.data.pointer("/message/source/replayState").is_some() => {
            Some(&["message", "source", "replayState"])
        }
        "assistant/message" if event.data.pointer("/source/replayState").is_some() => {
            Some(&["source", "replayState"])
        }
        "assistant/chunk" if event.data.pointer("/chunk/replayState").is_some() => {
            Some(&["chunk", "replayState"])
        }
        _ => None,
    }
}

struct WithoutPath<'a> {
    value: &'a Value,
    path: &'static [&'static str],
}
impl Serialize for WithoutPath<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(object) = self.value.as_object() else {
            return self.value.serialize(serializer);
        };
        let Some((omit, remaining)) = self.path.split_first() else {
            return self.value.serialize(serializer);
        };
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in object {
            if key == omit {
                if !remaining.is_empty() {
                    map.serialize_entry(
                        key,
                        &WithoutPath {
                            value,
                            path: remaining,
                        },
                    )?;
                }
            } else {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

pub(crate) struct PublicEvent<'a>(pub &'a SessionEvent);
impl Serialize for PublicEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let event = self.0;
        let Some(path) = replay_path(event) else {
            return event.serialize(serializer);
        };
        let fields = 4
            + usize::from(event.ignorable.is_some())
            + usize::from(event.surface_op.is_some())
            + usize::from(event.source_event_seqs.is_some());
        let mut value = serializer.serialize_struct("SessionEvent", fields)?;
        value.serialize_field("type", &event.type_)?;
        value.serialize_field("seq", &event.seq)?;
        value.serialize_field("time", &event.time)?;
        value.serialize_field(
            "data",
            &WithoutPath {
                value: &event.data,
                path,
            },
        )?;
        if let Some(ignorable) = event.ignorable {
            value.serialize_field("ignorable", &ignorable)?;
        }
        if let Some(surface) = &event.surface_op {
            value.serialize_field("surfaceOp", surface)?;
        }
        if let Some(sources) = &event.source_event_seqs {
            value.serialize_field("sourceEventSeqs", sources)?;
        }
        value.end()
    }
}

pub(crate) fn serialize<S: Serializer>(
    event: &SessionEvent,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    PublicEvent(event).serialize(serializer)
}

/// Drop private state from an already-owned page before it enters transport queues.
pub(crate) fn strip(event: &mut SessionEvent) {
    let Some(path) = replay_path(event) else {
        return;
    };
    let mut value = &mut event.data;
    for key in &path[..path.len() - 1] {
        let Some(next) = value.get_mut(*key) else {
            return;
        };
        value = next;
    }
    if let Some(object) = value.as_object_mut() {
        object.remove(path[path.len() - 1]);
    }
}

/// A mux event copy must not temporarily duplicate opaque replay buffers.
pub(crate) fn clone_for_browser(event: &SessionEvent) -> SessionEvent {
    fn copy(value: &Value, path: &[&str]) -> Value {
        let Some(object) = value.as_object() else {
            return value.clone();
        };
        let Some((omit, remaining)) = path.split_first() else {
            return value.clone();
        };
        Value::Object(
            object
                .iter()
                .filter_map(|(key, value)| {
                    if key == omit {
                        (!remaining.is_empty()).then(|| (key.clone(), copy(value, remaining)))
                    } else {
                        Some((key.clone(), value.clone()))
                    }
                })
                .collect(),
        )
    }
    let Some(path) = replay_path(event) else {
        return event.clone();
    };
    SessionEvent {
        type_: event.type_.clone(),
        seq: event.seq,
        time: event.time,
        data: copy(&event.data, path),
        ignorable: event.ignorable,
        surface_op: event.surface_op.clone(),
        source_event_seqs: event.source_event_seqs.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_and_mux_strip_replay_without_mutating_source_or_user_text() {
        for (kind, data) in [
            (
                "assistant/message",
                serde_json::json!({"message":{"id":"assistant-1","role":"assistant","source":{"kind":"model","provider":"fixture","model":"fixture","replayState":{"encrypted_content":"opaque-private"}},"content":[{"type":"text","text":"replayState is a literal user-facing word"}]}}),
            ),
            (
                "assistant/message",
                serde_json::json!({"id":"assistant-flat","role":"assistant","source":{"kind":"model","provider":"fixture","model":"fixture","replayState":{"encrypted_content":"opaque-private"}},"content":[{"type":"text","text":"replayState is a literal user-facing word"}]}),
            ),
            (
                "assistant/chunk",
                serde_json::json!({"turn":1,"step":1,"chunk":{"type":"finish","reason":{"kind":"stop"},"replayState":{"encrypted_content":"opaque-private"}}}),
            ),
        ] {
            let event: SessionEvent = serde_json::from_value(
                serde_json::json!({"seq":1,"time":1,"type":kind,"data":data}),
            )
            .unwrap();
            let history = crate::api::sessions::HistoryEntry {
                event: event.clone(),
                view: None,
            };
            let mux = crate::api::events::MuxFrame::SessionEventFrame {
                session_id: dsh_session::session_id("fixture"),
                event: event.clone(),
                view: None,
            };
            assert!(
                !serde_json::to_string(&history)
                    .unwrap()
                    .contains("opaque-private")
            );
            assert!(
                !serde_json::to_string(&mux)
                    .unwrap()
                    .contains("opaque-private")
            );
            assert!(
                serde_json::to_string(&event)
                    .unwrap()
                    .contains("opaque-private")
            );
            let public = clone_for_browser(&event);
            assert_eq!(
                serde_json::to_value(PublicEvent(&event)).unwrap(),
                serde_json::to_value(&public).unwrap()
            );
            assert_eq!(
                crate::api::sessions::serialized_event_len(&event),
                serde_json::to_vec(&public).unwrap().len()
            );
            if kind == "assistant/message" {
                assert!(
                    serde_json::to_string(&public)
                        .unwrap()
                        .contains("literal user-facing word")
                );
            }
        }
    }
}
