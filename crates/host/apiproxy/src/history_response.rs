//! Consume history payloads without duplicating the page's owned JSON buffers.

use crate::api::sessions::{HistoryEntry, SessionHistoryResult};
use serde_json::Value;

/// Keep the contract's serde field names, omission rules and field order while
/// moving the already-JSON event and projection payloads into the response.
pub(super) fn into_json(mut history: SessionHistoryResult) -> Value {
    let events = std::mem::take(&mut history.events);
    let projection_values = history
        .projections
        .as_mut()
        .map(|projections| std::mem::take(&mut projections.values));
    let mut value = serde_json::to_value(history).expect("history metadata serializes");
    value["events"] = Value::Array(events.into_iter().map(entry_into_json).collect());
    if let Some(values) = projection_values {
        value["projections"]["values"] = values;
    }
    value
}

fn entry_into_json(mut entry: HistoryEntry) -> Value {
    crate::public_event::strip(&mut entry.event);
    let data = std::mem::take(&mut entry.event.data);
    let mut value = serde_json::to_value(entry).expect("history entry metadata serializes");
    value["event"]["data"] = data;
    value
}

pub(super) fn subagent_into_json(
    mut history: crate::api::subagents::SubagentHistoryResult,
) -> Value {
    let events = std::mem::take(&mut history.events);
    let projection_values = history
        .projections
        .as_mut()
        .map(|projections| std::mem::take(&mut projections.values));
    let mut value = serde_json::to_value(history).expect("history metadata serializes");
    value["events"] = Value::Array(events.into_iter().map(entry_into_json).collect());
    if let Some(values) = projection_values {
        value["projections"]["values"] = values;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::sessions::SessionProjectionsBlock;
    use serde_json::json;

    fn history(events: Vec<HistoryEntry>) -> SessionHistoryResult {
        SessionHistoryResult {
            events,
            has_more: true,
            has_more_before: true,
            has_more_after: false,
            first_seq: Some(0),
            last_seq: Some(99),
            projections: None,
        }
    }

    #[test]
    fn owned_history_response_preserves_complete_wire_bytes() {
        let events: Vec<HistoryEntry> = serde_json::from_value(json!([
            {
                "event": {
                    "type": "assistant/message", "seq": 0, "time": -1,
                    "data": {"message": {
                        "content": [{"type": "text", "text": "正文 replayState \\ \""}],
                        "source": {"kind": "model", "provider": "fixture",
                            "replayState": {"encrypted_content": "private-nested"},
                            "model": "fixture"}
                    }},
                    "ignorable": false, "surfaceOp": "append", "sourceEventSeqs": []
                }
            },
            {
                "event": {
                    "type": "assistant/message", "seq": 1, "time": 2,
                    "data": {"source": {"replayState": {"state": "private-flat"}, "kind": "model"}},
                    "surfaceOp": {"op": "replace", "start": 0, "end": 0},
                    "sourceEventSeqs": [0]
                }
            },
            {
                "event": {
                    "type": "assistant/chunk", "seq": 2, "time": 3,
                    "data": {"chunk": {"type": "finish", "replayState": "private-chunk",
                        "reason": {"kind": "stop"}}}, "ignorable": true
                }
            },
            {
                "event": {
                    "type": "tool/call", "seq": 3, "time": 4,
                    "data": {"name": "fixture", "arguments": "{\"input\":\"x\"}"}
                },
                "view": {"for": "call", "view": {"card": "generic", "title": "Run",
                    "kind": "execute", "rawInput": {"input": "x"},
                    "content": [{"type": "text", "text": "presented"}],
                    "locations": [{"path": "file.rs", "line": 7}]}}
            },
            {
                "event": {"type": "tool/result", "seq": 4, "time": 5, "data": null},
                "view": {"for": "result", "view": {"card": "terminal",
                    "title": "Done", "output": "complete", "exitCode": 0}}
            },
            {"event": {"type": "custom/value", "seq": 5, "time": 6,
                "data": [null, false, 18446744073709551615u64, {"replayState": "public"}]}}
        ]))
        .unwrap();
        for projections in [
            None,
            Some(SessionProjectionsBlock {
                as_of_seq: 99,
                values: json!({"z": {"value": [1, 2, 3]}, "a": null}),
            }),
        ] {
            let mut page = history(events.clone());
            page.projections = projections;
            // session_history makes its bounded page public before either the
            // previous serde conversion or this owned response conversion.
            for entry in &mut page.events {
                crate::public_event::strip(&mut entry.event);
            }
            let child_page = crate::api::subagents::SubagentHistoryResult {
                events: page.events.clone(),
                has_more: page.has_more,
                projections: page.projections.clone(),
            };
            let expected_child =
                serde_json::to_vec(&serde_json::to_value(&child_page).unwrap()).unwrap();
            assert_eq!(
                serde_json::to_vec(&subagent_into_json(child_page)).unwrap(),
                expected_child
            );
            let expected = serde_json::to_value(&page).unwrap();
            let actual = into_json(page);
            assert_eq!(actual, expected);
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap(),
                "owned conversion must also preserve JSON field order"
            );
            assert!(!actual.to_string().contains("private-"));
        }
        let mut empty = history(vec![]);
        empty.has_more = false;
        empty.has_more_before = false;
        empty.first_seq = None;
        empty.last_seq = None;
        let expected = serde_json::to_value(&empty).unwrap();
        assert_eq!(into_json(empty), expected);
    }

    #[test]
    fn owned_history_response_moves_large_event_and_projection_buffers() {
        let mut entry: HistoryEntry = serde_json::from_value(json!({
            "event": {"type": "custom/value", "seq": 0, "time": 0, "data": null}
        }))
        .unwrap();
        entry.event.data = json!({"text": "e".repeat(2 * 1024 * 1024)});
        let event_ptr = entry.event.data["text"].as_str().unwrap().as_ptr();
        let values = json!({"text": "p".repeat(2 * 1024 * 1024)});
        let projection_ptr = values["text"].as_str().unwrap().as_ptr();
        let mut page = history(vec![entry]);
        page.projections = Some(SessionProjectionsBlock {
            as_of_seq: 0,
            values,
        });
        let actual = into_json(page);
        assert_eq!(
            actual["events"][0]["event"]["data"]["text"]
                .as_str()
                .unwrap()
                .as_ptr(),
            event_ptr
        );
        assert_eq!(
            actual["projections"]["values"]["text"]
                .as_str()
                .unwrap()
                .as_ptr(),
            projection_ptr
        );
    }
}
