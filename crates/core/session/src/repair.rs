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
    let mut repair = InterruptedTurnRepair::default();
    for event in events {
        repair.observe(event);
    }
    repair.finish()
}

/// Streaming form of interrupted-tail recovery, retaining only unfinished
/// calls in the current turn and the exact final sequence and timestamp.
#[derive(Default)]
pub struct InterruptedTurnRepair {
    open_turn: Option<u64>,
    open_step: Option<u64>,
    pending_calls: IndexMap<String, PendingCall>,
    last: Option<(u64, i64)>,
}

impl InterruptedTurnRepair {
    pub fn observe(&mut self, event: &SessionEvent) {
        self.last = Some((event.seq.get(), event.time));
        match event.type_.as_str() {
            "turn/start" => {
                self.open_turn = event.data.get("turn").and_then(|value| value.as_u64());
                self.open_step = None;
                self.pending_calls.clear();
            }
            "turn/end" => {
                self.open_turn = None;
                self.open_step = None;
                self.pending_calls.clear();
            }
            "step/start" => {
                self.open_step = event.data.get("step").and_then(|value| value.as_u64());
            }
            "step/end" => {
                self.pending_calls.clear();
                self.open_step = None;
            }
            "assistant/message" => {
                let step = event.data.get("step").and_then(|value| value.as_u64());
                if let (Some(step), Some(message)) = (step, event.data.get("message"))
                    && let Some(blocks) = message.get("content").and_then(|value| value.as_array())
                {
                    for block in blocks {
                        if block.get("type").and_then(|value| value.as_str()) == Some("tool-call")
                            && let Some(id) = block.get("id").and_then(|value| value.as_str())
                        {
                            self.pending_calls
                                .entry(id.to_string())
                                .or_insert(PendingCall {
                                    step,
                                    call_seq: None,
                                });
                        }
                    }
                }
            }
            "tool/call" => {
                if let Some(call_id) = event.data.get("callId").and_then(|value| value.as_str())
                    && let Some(entry) = self.pending_calls.get_mut(call_id)
                {
                    entry.call_seq = Some(event.seq.get());
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
                    self.pending_calls.shift_remove(call_id);
                }
            }
            _ => {}
        }
    }

    pub fn finish(self) -> Vec<SessionEvent> {
        let Some(turn) = self.open_turn else {
            return Vec::new();
        };
        let Some((last_seq, time)) = self.last else {
            return Vec::new();
        };

        let mut seq = last_seq + 1;
        let mut closers: Vec<SessionEvent> = Vec::new();

        for (call_id, pending) in self.pending_calls {
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
                seq: crate::SessionSeq::new(seq).expect("repair seq fits the Session wire"),
                time,
                data,
                ignorable: None,
                surface_op: Some(SurfaceOp::Append),
                source_event_seqs: started.then(|| vec![pending.call_seq.unwrap()]),
            };
            seq += 1;
            closers.push(event);
        }

        if let Some(step) = self.open_step {
            closers.push(SessionEvent {
                type_: "step/end".to_string(),
                seq: crate::SessionSeq::new(seq).expect("repair seq fits the Session wire"),
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
            seq: crate::SessionSeq::new(seq).expect("repair seq fits the Session wire"),
            time,
            data: serde_json::json!({"turn": turn, "reason": {"kind": "interrupted"}}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        });
        closers
    }
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
        role: Role::Tool,
        source: MessageSource::Tool {
            call_id: dsh_llm::call_id(call_id),
        },
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        tool_call_id: Some(dsh_llm::call_id(call_id)),
        is_error: Some(true),
    }
}
