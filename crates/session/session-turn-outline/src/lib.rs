//! Whole-log `turnOutline` projection used by paged conversation navigation.

use std::sync::Arc;

use cordis::{ArcValue, Context, arc, downcast};
use dsh_llm::{ContentBlock, Message, MessageSource};
use dsh_session::{SessionEvent, SessionHeader};
use dsh_session_projection::{ProjectionApply, ProjectionDefinition, SessionProjectionRegistry};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const KEY: &str = "turnOutline";
pub const PROMPT_PREVIEW_LIMIT: usize = 50;
pub const RESPONSE_PREVIEW_LIMIT: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnOutlineEntry {
    pub turn: u64,
    pub seq: u64,
    pub prompt: String,
    pub response: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnOutlineState {
    turns: Vec<TurnOutlineEntry>,
    draft: String,
}

fn empty_state() -> TurnOutlineState {
    TurnOutlineState {
        turns: Vec::new(),
        draft: String::new(),
    }
}

fn clipped_text(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max || text.is_char_boundary(max) {
        let end = text.len().min(max);
        return (&text[..end], text.len() > end);
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

fn preview(content: &[ContentBlock], limit: usize) -> String {
    let scan_limit = limit.saturating_mul(2);
    let mut text = String::new();
    let mut unread = false;
    for block in content {
        let ContentBlock::Text { text: block_text } = block else {
            continue;
        };
        if text.chars().count() >= scan_limit {
            unread = true;
            break;
        }
        let (chunk, clipped) = clipped_text(block_text, scan_limit);
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(chunk);
        if clipped {
            unread = true;
            break;
        }
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = normalized.chars().collect::<Vec<_>>();
    if chars.len() > limit.saturating_sub(1) {
        let body = chars[..limit.saturating_sub(1)]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_string();
        return format!("{body}…");
    }
    if unread && !normalized.is_empty() {
        format!("{normalized}…")
    } else {
        normalized
    }
}

fn decode_state(value: &ArcValue) -> TurnOutlineState {
    let json = downcast::<Value>(value).expect("turn outline state must be JSON");
    serde_json::from_value(json.clone()).expect("turn outline state must be valid")
}

fn encode_state(state: &TurnOutlineState) -> ArcValue {
    arc(serde_json::to_value(state).expect("turn outline state serializes"))
}

fn message_from_event(event: &SessionEvent) -> Option<Message> {
    match event.type_.as_str() {
        "user/message" => serde_json::from_value(event.data.clone()).ok(),
        "assistant/message" => event
            .data
            .get("message")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        _ => None,
    }
}

fn validate_view(value: &ArcValue) -> Result<Value, String> {
    let json =
        downcast::<Value>(value).ok_or_else(|| "turn outline view must be JSON".to_string())?;
    let entries: Vec<TurnOutlineEntry> = serde_json::from_value(json.clone())
        .map_err(|error| format!("turn outline view is invalid: {error}"))?;
    let mut previous = None;
    for entry in &entries {
        if entry.prompt.chars().count() > PROMPT_PREVIEW_LIMIT
            || entry.response.chars().count() > RESPONSE_PREVIEW_LIMIT
        {
            return Err("turn outline preview exceeds its card budget".to_string());
        }
        if previous.is_some_and(|turn| entry.turn <= turn) {
            return Err("turn outline entries must be strictly increasing by turn".to_string());
        }
        previous = Some(entry.turn);
    }
    serde_json::to_value(entries).map_err(|error| error.to_string())
}

pub fn turn_outline_projection_definition() -> ProjectionDefinition {
    let apply: ProjectionApply = Arc::new(|state_value, event| {
        let state = decode_state(state_value);
        match event.type_.as_str() {
            "turn/start" => {
                let Some(turn) = event.data.get("turn").and_then(Value::as_u64) else {
                    return Arc::clone(state_value);
                };
                if state.turns.last().is_some_and(|last| turn <= last.turn) {
                    return Arc::clone(state_value);
                }
                let mut next = state;
                next.turns.push(TurnOutlineEntry {
                    turn,
                    seq: event.seq.get(),
                    prompt: String::new(),
                    response: String::new(),
                });
                next.draft.clear();
                encode_state(&next)
            }
            "user/message" => {
                let Some(message) = message_from_event(event) else {
                    return Arc::clone(state_value);
                };
                if !matches!(message.source, MessageSource::User { .. })
                    || state
                        .turns
                        .last()
                        .is_none_or(|last| !last.prompt.is_empty())
                {
                    return Arc::clone(state_value);
                }
                let prompt = preview(&message.content, PROMPT_PREVIEW_LIMIT);
                if prompt.is_empty() {
                    return Arc::clone(state_value);
                }
                let mut next = state;
                next.turns.last_mut().expect("turn checked").prompt = prompt;
                encode_state(&next)
            }
            "assistant/message" => {
                let Some(message) = message_from_event(event) else {
                    return Arc::clone(state_value);
                };
                let draft = preview(&message.content, RESPONSE_PREVIEW_LIMIT);
                if draft.is_empty() || draft == state.draft {
                    return Arc::clone(state_value);
                }
                let mut next = state;
                next.draft = draft;
                encode_state(&next)
            }
            "turn/end" => {
                if state.draft.is_empty() {
                    return Arc::clone(state_value);
                }
                let mut next = state;
                let draft = std::mem::take(&mut next.draft);
                let Some(last) = next.turns.last_mut() else {
                    return Arc::clone(state_value);
                };
                if last.response == draft {
                    return encode_state(&next);
                }
                last.response = draft;
                encode_state(&next)
            }
            _ => Arc::clone(state_value),
        }
    });
    ProjectionDefinition {
        key: KEY.to_string(),
        schema: Arc::new(validate_view),
        init: Arc::new(|_header: &SessionHeader| encode_state(&empty_state())),
        apply,
        view: Arc::new(|state_value| {
            let state: Arc<Value> =
                cordis::downcast_arc(state_value).expect("turn outline state must be JSON");
            Arc::new(
                state
                    .get("turns")
                    .cloned()
                    .expect("turn outline state carries turns"),
            )
        }),
        state_version: 2,
    }
}

pub fn apply(ctx: &Context) -> Result<(), String> {
    let registry = ctx
        .get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "session-turn-outline requires sessionProjections".to_string())?;
    registry.register(ctx, turn_outline_projection_definition())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_llm::{MessageSource, create_assistant_message, create_user_message};
    use dsh_session::{SessionEvent, session_id};

    fn event(type_: &str, seq: u64, data: Value) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq: dsh_session::SessionSeq::new(seq).expect("test sequence is valid"),
            time: seq as i64,
            data,
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    fn header() -> SessionHeader {
        SessionHeader {
            version: dsh_session::SESSION_FORMAT_VERSION,
            id: session_id("outline-test"),
            created_at: 0,
            cwd: None,
            parent_session: None,
            is_seeded: false,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }
    }

    #[test]
    fn folds_every_turn_with_bounded_prompt_and_settled_response_previews() {
        let definition = turn_outline_projection_definition();
        let mut state = (definition.init)(&header());
        state = (definition.apply)(
            &state,
            &event("turn/start", 0, serde_json::json!({"turn": 1})),
        );
        let user = create_user_message(
            vec![ContentBlock::Text {
                text: "  first\n prompt  ".into(),
            }],
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        state = (definition.apply)(
            &state,
            &event("user/message", 1, serde_json::to_value(user).unwrap()),
        );
        let assistant = create_assistant_message(
            vec![ContentBlock::Text {
                text: "final answer".into(),
            }],
            dsh_llm::ModelMessageSource {
                provider: "p".into(),
                model: "m".into(),
                replay_state: None,
            },
        );
        state = (definition.apply)(
            &state,
            &event(
                "assistant/message",
                2,
                serde_json::json!({"turn":1,"step":1,"message":assistant}),
            ),
        );
        let draft_view = (definition.view)(&state);
        let draft: &Value = downcast(&draft_view).unwrap();
        assert_eq!(draft[0]["response"], "");
        state = (definition.apply)(
            &state,
            &event(
                "turn/end",
                3,
                serde_json::json!({"turn": 1, "reason":{"kind":"completed"}}),
            ),
        );
        let view_value = (definition.view)(&state);
        let view: &Value = downcast(&view_value).unwrap();
        assert_eq!(view[0]["turn"], 1);
        assert_eq!(view[0]["seq"], 0);
        assert_eq!(view[0]["prompt"], "first prompt");
        assert_eq!(view[0]["response"], "final answer");
    }

    #[test]
    fn malformed_and_non_user_messages_do_not_pollute_the_outline() {
        let definition = turn_outline_projection_definition();
        let mut state = (definition.init)(&header());
        state = (definition.apply)(
            &state,
            &event("turn/start", 0, serde_json::json!({"turn": 1})),
        );
        let plugin = create_user_message(
            vec![ContentBlock::Text {
                text: "hidden".into(),
            }],
            MessageSource::Plugin {
                plugin: "test".into(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        );
        state = (definition.apply)(
            &state,
            &event("user/message", 1, serde_json::to_value(plugin).unwrap()),
        );
        let same = (definition.apply)(
            &state,
            &event("user/message", 2, serde_json::json!({"broken": true})),
        );
        assert!(Arc::ptr_eq(&state, &same));
        let view_value = (definition.view)(&state);
        let view: &Value = downcast(&view_value).unwrap();
        assert_eq!(view[0]["prompt"], "");
    }

    #[tokio::test]
    async fn apply_registers_the_projection() {
        let ctx = Context::root();
        let registry = SessionProjectionRegistry::install(&ctx);
        apply(&ctx).expect("register outline");
        assert!(registry.keys().iter().any(|key| key == KEY));
    }
}
