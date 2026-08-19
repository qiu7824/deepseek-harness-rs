//! Crash-recovery repair for an interrupted session log. Rust port of
//! `packages/core/session/src/repair.ts`.

use dsh_llm::{ContentBlock, Message, MessageSource, Role};
use indexmap::IndexMap;

use crate::types::{SessionEvent, SurfaceOp};

/// Recovery code for an assistant tool request that never reached a
/// recorded call start.
pub const TOOL_NOT_STARTED: &str = "TOOL_NOT_STARTED";

/// Recovery code for a recorded tool call whose completed outcome was not
/// durably recorded.
pub const TOOL_OUTCOME_UNKNOWN: &str = "TOOL_OUTCOME_UNKNOWN";

/// One call pending its `tool/result` while scanning the durable tail.
#[derive(Debug, Clone)]
struct PendingCall {
    step: u64,
    call_seq: Option<u64>,
}

/// Return deterministic synthetic events that close an open tail turn
/// (TS `interruptedTurnClosers`).
pub fn interrupted_turn_closers(events: &[SessionEvent]) -> Vec<SessionEvent> {
    let mut open_turn: Option<u64> = None;
    let mut open_step: Option<u64> = None;
    let mut pending_calls: IndexMap<String, PendingCall> = IndexMap::new();

    for event in events {
        match event.type_.as_str() {
            "turn/start" => {
                open_turn = event.data.get("turn").and_then(|value| value.as_u64());
                open_step = None;
                pending_calls.clear();
            }
            "turn/end" => {
                open_turn = None;
                open_step = None;
                pending_calls.clear();
            }
            "step/start" => {
                open_step = event.data.get("step").and_then(|value| value.as_u64());
            }
            "step/end" => {
                pending_calls.clear();
                open_step = None;
            }
            "assistant/message" => {
                let step = event.data.get("step").and_then(|value| value.as_u64());
                if let (Some(step), Some(message)) = (step, event.data.get("message")) {
                    if let Some(blocks) = message.get("content").and_then(|value| value.as_array())
                    {
                        for block in blocks {
                            if block.get("type").and_then(|value| value.as_str())
                                == Some("tool-call")
                            {
                                if let Some(id) = block.get("id").and_then(|value| value.as_str()) {
                                    pending_calls.entry(id.to_string()).or_insert(PendingCall {
                                        step,
                                        call_seq: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            "tool/call" => {
                if let Some(call_id) = event.data.get("callId").and_then(|value| value.as_str()) {
                    if let Some(entry) = pending_calls.get_mut(call_id) {
                        entry.call_seq = Some(event.seq);
                    }
                }
            }
            "tool/result" => {
                let call_id = event
                    .data
                    .get("message")
                    .and_then(|value| value.get("source"))
                    .and_then(|value| value.get("callId"))
                    .and_then(|value| value.as_str());
                if let Some(call_id) = call_id {
                    pending_calls.shift_remove(call_id);
                }
            }
            _ => {}
        }
    }

    let Some(turn) = open_turn else {
        return Vec::new();
    };
    let Some(last) = events.last() else {
        return Vec::new();
    };

    let mut seq = last.seq + 1;
    let time = last.time;
    let mut closers: Vec<SessionEvent> = Vec::new();

    for (call_id, pending) in pending_calls {
        let started = pending.call_seq.is_some();
        let message = interrupted_tool_result_message(&call_id, seq, started);
        let data = serde_json::json!({
            "turn": turn,
            "step": pending.step,
            "message": message,
            "error": if started {
                serde_json::json!({"name": "ToolOutcomeUnknownError", "code": TOOL_OUTCOME_UNKNOWN})
            } else {
                serde_json::json!({"name": "ToolNotStartedError", "code": TOOL_NOT_STARTED})
            },
        });
        // `sourceEventSeqs` rides the EVENT envelope (TS repair.ts), never
        // the data payload.
        let event = SessionEvent {
            type_: "tool/result".to_string(),
            seq,
            time,
            data,
            ignorable: None,
            surface_op: Some(SurfaceOp::Append),
            source_event_seqs: started.then(|| vec![pending.call_seq.unwrap()]),
        };
        seq += 1;
        closers.push(event);
    }

    if let Some(step) = open_step {
        closers.push(SessionEvent {
            type_: "step/end".to_string(),
            seq,
            time,
            data: serde_json::json!({"turn": turn, "step": step}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        });
        seq += 1;
    }
    closers.push(SessionEvent {
        type_: "turn/end".to_string(),
        seq,
        time,
        data: serde_json::json!({"turn": turn, "reason": {"kind": "interrupted"}}),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    });
    closers
}

/// Build the deterministic interrupted tool-result message.
fn interrupted_tool_result_message(call_id: &str, seq: u64, started: bool) -> Message {
    let text = if started {
        "The tool call was interrupted after it was recorded, but no result was durably recorded. Its outcome is unknown. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly."
    } else {
        "The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."
    };
    Message {
        id: dsh_llm::message_id(format!("interrupted-tool-result-{call_id}-{seq}")),
        role: Role::User,
        source: MessageSource::Tool {
            call_id: dsh_llm::call_id(call_id),
        },
        content: vec![ContentBlock::ToolResult {
            tool_call_id: dsh_llm::call_id(call_id),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            is_error: Some(true),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as JsonValue;

    fn event(type_: &str, seq: u64, data: JsonValue) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time: 1000,
            data,
            ignorable: None,
            surface_op: match type_ {
                "assistant/message" | "tool/result" => Some(SurfaceOp::Append),
                _ => None,
            },
            source_event_seqs: None,
        }
    }

    #[test]
    fn balanced_log_yields_no_closers() {
        let events = vec![
            event("turn/start", 0, serde_json::json!({"turn": 1})),
            event("step/start", 1, serde_json::json!({"turn": 1, "step": 1})),
            event("step/end", 2, serde_json::json!({"turn": 1, "step": 1})),
            event(
                "turn/end",
                3,
                serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
            ),
        ];
        assert!(interrupted_turn_closers(&events).is_empty());
        assert!(interrupted_turn_closers(&[]).is_empty());
    }

    #[test]
    fn open_turn_without_calls_closes_step_and_turn() {
        let events = vec![
            event("turn/start", 0, serde_json::json!({"turn": 1})),
            event("step/start", 1, serde_json::json!({"turn": 1, "step": 1})),
        ];
        let closers = interrupted_turn_closers(&events);
        assert_eq!(closers.len(), 2);
        assert_eq!(closers[0].type_, "step/end");
        assert_eq!(closers[0].seq, 2);
        assert_eq!(closers[0].data, serde_json::json!({"turn": 1, "step": 1}));
        assert_eq!(closers[1].type_, "turn/end");
        assert_eq!(closers[1].seq, 3);
        assert_eq!(
            closers[1].data,
            serde_json::json!({"turn": 1, "reason": {"kind": "interrupted"}})
        );
        assert_eq!(
            closers[0].time, 1000,
            "closers reuse the last real timestamp"
        );
        assert_eq!(closers[1].time, 1000);
    }

    #[test]
    fn unmatched_assistant_call_gets_error_result() {
        let events = vec![
            event("turn/start", 0, serde_json::json!({"turn": 1})),
            event("step/start", 1, serde_json::json!({"turn": 1, "step": 1})),
            event(
                "assistant/message",
                2,
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "id": "m1", "role": "assistant",
                        "content": [{"type": "tool-call", "id": "c1", "name": "run", "arguments": "{}"}],
                        "source": {"kind": "model", "provider": "p", "model": "m"},
                    },
                }),
            ),
        ];
        let closers = interrupted_turn_closers(&events);
        assert_eq!(closers.len(), 3);
        let result = &closers[0];
        assert_eq!(result.type_, "tool/result");
        assert_eq!(result.seq, 3);
        assert_eq!(result.data["turn"], 1);
        assert_eq!(result.data["step"], 1);
        assert_eq!(result.data["error"]["code"], TOOL_NOT_STARTED);
        assert!(
            result.source_event_seqs.is_none(),
            "not-started call cites no tool/call seq"
        );
        let message: Message = serde_json::from_value(result.data["message"].clone()).unwrap();
        assert_eq!(message.role, Role::User);
        assert_eq!(message.id.as_str(), "interrupted-tool-result-c1-3");
        let MessageSource::Tool { call_id } = &message.source else {
            panic!("expected tool source");
        };
        assert_eq!(call_id.as_str(), "c1");
        let Some(ContentBlock::ToolResult { is_error, .. }) = message.content.first() else {
            panic!("expected tool-result block");
        };
        assert_eq!(*is_error, Some(true));

        // step and turn close after the synthetic results
        assert_eq!(closers[1].type_, "step/end");
        assert_eq!(closers[2].type_, "turn/end");
    }

    #[test]
    fn recorded_call_gets_unknown_outcome_and_citation() {
        let events = vec![
            event("turn/start", 0, serde_json::json!({"turn": 2})),
            event("step/start", 1, serde_json::json!({"turn": 2, "step": 1})),
            event(
                "assistant/message",
                2,
                serde_json::json!({
                    "turn": 2, "step": 1,
                    "message": {
                        "id": "m1", "role": "assistant",
                        "content": [{"type": "tool-call", "id": "c1", "name": "run", "arguments": "{}"}],
                        "source": {"kind": "model", "provider": "p", "model": "m"},
                    },
                }),
            ),
            event(
                "tool/call",
                3,
                serde_json::json!({"turn": 2, "step": 1, "callId": "c1", "name": "run", "arguments": "{}"}),
            ),
        ];
        let closers = interrupted_turn_closers(&events);
        assert_eq!(closers.len(), 3);
        let result = &closers[0];
        assert_eq!(result.data["error"]["code"], TOOL_OUTCOME_UNKNOWN);
        assert_eq!(result.source_event_seqs, Some(vec![3]));
        assert!(
            !result
                .data
                .as_object()
                .unwrap()
                .contains_key("sourceEventSeqs")
        );
    }

    #[test]
    fn completed_result_clears_pending_call() {
        let events = vec![
            event("turn/start", 0, serde_json::json!({"turn": 1})),
            event("step/start", 1, serde_json::json!({"turn": 1, "step": 1})),
            event(
                "assistant/message",
                2,
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "id": "m1", "role": "assistant",
                        "content": [{"type": "tool-call", "id": "c1", "name": "run", "arguments": "{}"}],
                        "source": {"kind": "model", "provider": "p", "model": "m"},
                    },
                }),
            ),
            event(
                "tool/result",
                3,
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "id": "m2", "role": "user",
                        "content": [{"type": "tool-result", "toolCallId": "c1",
                            "content": [{"type": "text", "text": "ok"}], "isError": false}],
                        "source": {"kind": "tool", "callId": "c1"},
                    },
                }),
            ),
        ];
        let closers = interrupted_turn_closers(&events);
        assert_eq!(
            closers.len(),
            2,
            "no synthetic tool result; only step/end + turn/end"
        );
        assert_eq!(closers[0].type_, "step/end");
        assert_eq!(closers[1].type_, "turn/end");
    }
}
