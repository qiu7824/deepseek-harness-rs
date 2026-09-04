//! Durable message counts and compact, content-free current-context accounting.
//! Billing totals remain owned by tokenUsage. Reasoning is a subset of output,
//! and missing provider measurements are represented by null rather than zero.

use cordis::{ArcValue, arc, downcast};
use dsh_session::{SessionEvent, SurfaceOp};
use dsh_session_projection::ProjectionDefinition;
use dsh_token_meter::estimate::{estimate_header, estimate_message};
use serde_json::{Value, json};
use std::sync::Arc;

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn fold(state: &ArcValue, event: &SessionEvent) -> ArcValue {
    let prior = downcast::<Value>(state).expect("contextInsights state");
    let message = dsh_session::surface::derive_event_message(event);
    let usage = if event.type_ == "assistant/message" {
        event.data.get("usage")
    } else if event.type_ == "assistant/chunk" && event.data["chunk"]["type"] == "usage" {
        event.data.get("chunk").and_then(|chunk| chunk.get("usage"))
    } else {
        None
    };
    let reasoning = usage
        .and_then(|value| value.get("reasoningTokens"))
        .and_then(Value::as_u64);
    let header = if event.type_ == "request/header" {
        event
            .data
            .get("header")
            .cloned()
            .and_then(|header| serde_json::from_value::<dsh_session::EpochHeader>(header).ok())
    } else {
        None
    };
    // Streaming text has no new durable figures; do not clone the active surface per token.
    if message.is_none() && reasoning.is_none() && header.is_none() {
        return Arc::clone(state);
    }
    let mut next = prior.clone();
    if let Some(header) = header {
        next["headerTokens"] = json!(estimate_header(Some(&header)));
    }
    if let Some(reasoning) = reasoning {
        let turn = &event.data["turn"];
        let step = &event.data["step"];
        let last = &prior["lastReasoning"];
        let previous = if last["turn"] == *turn && last["step"] == *step {
            number(last, "tokens")
        } else {
            0
        };
        next["reasoningTokens"] = json!(
            number(prior, "reasoningTokens")
                .saturating_sub(previous)
                .saturating_add(reasoning)
        );
        next["lastReasoning"] = json!({"turn": turn, "step": step, "tokens": reasoning});
    }
    if let Some(message) = message {
        let group = match event.type_.as_str() {
            "assistant/message" => "assistant",
            "tool/result" => "tool",
            "user/message" if event.data["source"]["kind"] == "user" => "user",
            _ => "other",
        };
        // Whole-log message counts exclude rewritten copies and injected instructions.
        if matches!(event.surface_op, Some(SurfaceOp::Append)) {
            let field = match group {
                "user" => Some("userMessages"),
                "assistant" => Some("assistantMessages"),
                _ => None,
            };
            if let Some(field) = field {
                next[field] = json!(number(prior, field).saturating_add(1));
            }
        }
        if let Some(operation) = &event.surface_op {
            let mut removed = Vec::new();
            let surface = next["surface"].as_array_mut().expect("surface array");
            if let SurfaceOp::Replace { start, end } = operation {
                surface.retain(|entry| {
                    let seq = entry[0].as_u64().unwrap_or(0);
                    if seq >= *start && seq <= *end {
                        removed.push(entry.clone());
                        false
                    } else {
                        true
                    }
                });
            }
            let tokens = estimate_message(&message);
            surface.push(json!([event.seq.get(), group, tokens]));
            for entry in removed {
                if let Some(group) = entry[1].as_str() {
                    next["roleTokens"][group] = json!(
                        number(&next["roleTokens"], group)
                            .saturating_sub(entry[2].as_u64().unwrap_or(0))
                    );
                }
            }
            next["roleTokens"][group] =
                json!(number(&next["roleTokens"], group).saturating_add(tokens));
        }
    }
    arc(next)
}

pub fn context_insights_projection_definition() -> ProjectionDefinition {
    ProjectionDefinition {
        key: "contextInsights".into(),
        state_version: 1,
        init: Arc::new(|header| {
            arc(json!({
                "createdAt": header.created_at,
                "userMessages": 0, "assistantMessages": 0,
                "reasoningTokens": null, "lastReasoning": null,
                "headerTokens": 0,
                "roleTokens": {"user": 0, "assistant": 0, "tool": 0, "other": 0},
                "surface": []
            }))
        }),
        apply: Arc::new(fold),
        view: Arc::new(|state| {
            let state = downcast::<Value>(state).expect("contextInsights state");
            let mut roles = state["roleTokens"].clone();
            roles["other"] =
                json!(number(&roles, "other").saturating_add(number(state, "headerTokens")));
            arc(json!({
                "createdAt": state["createdAt"],
                "userMessages": state["userMessages"],
                "assistantMessages": state["assistantMessages"],
                "reasoningTokens": state["reasoningTokens"],
                "roleTokens": roles,
                "totalCost": null
            }))
        }),
        schema: Arc::new(|value| {
            let value = downcast::<Value>(value).ok_or("contextInsights must be JSON")?;
            for key in ["createdAt", "userMessages", "assistantMessages"] {
                if value.get(key).and_then(Value::as_u64).is_none() {
                    return Err(format!("invalid contextInsights {key}"));
                }
            }
            for key in ["user", "assistant", "tool", "other"] {
                if value["roleTokens"]
                    .get(key)
                    .and_then(Value::as_u64)
                    .is_none()
                {
                    return Err(format!("invalid contextInsights role {key}"));
                }
            }
            if !value["reasoningTokens"].is_null() && value["reasoningTokens"].as_u64().is_none() {
                return Err("invalid reasoningTokens".into());
            }
            Ok(value.clone())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(seq: u64, kind: &str, data: Value, surface_op: Option<SurfaceOp>) -> SessionEvent {
        serde_json::from_value(json!({"seq": seq, "time": 1000 + seq, "type": kind, "data": data, "surfaceOp": surface_op})).unwrap()
    }
    fn message(kind: &str, text: &str) -> Value {
        json!({"id": "message-id", "role": "user", "content": [{"type": "text", "text": text}], "source": {"kind": kind}})
    }
    fn state() -> ArcValue {
        arc(
            json!({"createdAt": 1, "userMessages": 0, "assistantMessages": 0, "reasoningTokens": null, "lastReasoning": null, "headerTokens": 0, "roleTokens": {"user": 0, "assistant": 0, "tool": 0, "other": 0}, "surface": []}),
        )
    }
    #[test]
    fn replacement_updates_role_estimates_without_recounting_messages() {
        let first = fold(
            &state(),
            &event(
                1,
                "user/message",
                message("user", "hello world"),
                Some(SurfaceOp::Append),
            ),
        );
        assert_eq!(downcast::<Value>(&first).unwrap()["userMessages"], 1);
        let mut replacement = message("plugin", "summary");
        replacement["source"]["plugin"] = json!("compaction");
        let next = fold(
            &first,
            &event(
                2,
                "user/message",
                replacement,
                Some(SurfaceOp::Replace { start: 1, end: 1 }),
            ),
        );
        let next = downcast::<Value>(&next).unwrap();
        assert_eq!(next["userMessages"], 1);
        assert_eq!(next["roleTokens"]["user"], 0);
        assert!(number(&next["roleTokens"], "other") > 0);
        assert_eq!(next["surface"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn streamed_and_final_reasoning_are_one_measurement() {
        let first = fold(
            &state(),
            &event(
                1,
                "assistant/chunk",
                json!({"turn": 1,"step": 1,"chunk": {"type": "usage", "usage": {"reasoningTokens": 10}}}),
                None,
            ),
        );
        let final_state = fold(
            &first,
            &event(
                2,
                "assistant/message",
                json!({"turn": 1,"step": 1,"usage": {"reasoningTokens": 12}}),
                None,
            ),
        );
        assert_eq!(
            downcast::<Value>(&final_state).unwrap()["reasoningTokens"],
            12
        );
        let next = fold(
            &final_state,
            &event(
                3,
                "assistant/message",
                json!({"turn": 2,"step": 1,"usage": {"reasoningTokens": 4}}),
                None,
            ),
        );
        assert_eq!(downcast::<Value>(&next).unwrap()["reasoningTokens"], 16);
        let unchanged = fold(
            &next,
            &event(
                4,
                "assistant/chunk",
                json!({"chunk": {"type": "text-delta", "text": "hello"}}),
                None,
            ),
        );
        assert!(Arc::ptr_eq(&next, &unchanged));
    }
}
