//! The `sessionStats` projection unit: a pure fold of step boundaries,
//! stream chunks, tool pairs, and assembled assistant messages into
//! whole-log counts and wall times. Rust port of
//! `packages/session/session-stats/src/projection.ts`.
//!
//! `step/end` — not `assistant/message` — is the counted step event because
//! it is the step lifecycle authority: the loop appends exactly one per
//! entered step, in a `finally`, so completed, failed, cancelled, and
//! max-tokens steps all land one.
//!
//! The wall-time folds mirror the client window fold field by field: model
//! time is `step/start` → `assistant/message`, first token is the first
//! non-empty delta chunk and survives an in-step `llm/retry`, decode spans
//! first token → assembled message on steps that also report output tokens,
//! and tool time pairs `tool/call` → `tool/result` by callId.

use std::sync::Arc;

use cordis::ArcValue;
use dsh_session::SessionEvent;
use dsh_session_projection::ProjectionDefinition;

use crate::types::SessionStatsProjection;

/// Provider-reported completion tokens, guarded the way the window fold
/// guards node usage (TS `usageOutputTokens`).
fn usage_output_tokens(usage: Option<&serde_json::Value>) -> Option<u64> {
    let usage = usage?;
    usage.get("outputTokens")?.as_u64()
}

fn number(state: &serde_json::Value, key: &str) -> u64 {
    state
        .get(key)
        .and_then(|value| value.as_u64())
        .expect("sessionStats state field")
}

/// The `sessionStats` unit registered on `ctx.sessionProjections`
/// (exported for the unit spec).
pub fn session_stats_projection_definition() -> ProjectionDefinition {
    let init: Arc<dyn Fn() -> ArcValue + Send + Sync> = Arc::new(|| {
        cordis::arc(serde_json::json!({
            "turns": 0, "steps": 0,
            "llmMs": 0, "toolMs": 0, "ttftMs": 0, "ttftSteps": 0,
            "decodeMs": 0, "decodeTokens": 0,
            "lastTurn": serde_json::Value::Null,
            "openStep": serde_json::Value::Null,
            "pendingCalls": {},
        }))
    });
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> =
        Arc::new(move |state_value: &ArcValue, event: &SessionEvent| {
            let state: &serde_json::Value = cordis::downcast(state_value).expect("sessionStats state");
            let data = &event.data;
            match event.type_.as_str() {
                "step/start" => {
                    let mut next = state.clone();
                    next["openStep"] = serde_json::json!({
                        "turn": data.get("turn"),
                        "step": data.get("step"),
                        "startTime": event.time,
                        "firstTokenTime": serde_json::Value::Null,
                    });
                    cordis::arc(next)
                }
                "assistant/chunk" => {
                    let open = state.get("openStep").expect("state field");
                    if open.is_null()
                        || open.get("turn") != data.get("turn")
                        || open.get("step") != data.get("step")
                        || !open.get("firstTokenTime").is_some_and(|t| t.is_null())
                    {
                        return Arc::clone(state_value);
                    }
                    let chunk = match data.get("chunk").cloned() {
                        Some(chunk) => match serde_json::from_value::<dsh_llm::StreamChunk>(chunk) {
                            Ok(chunk) => chunk,
                            Err(_) => return Arc::clone(state_value),
                        },
                        None => return Arc::clone(state_value),
                    };
                    if !dsh_llm::is_token_delta(&chunk) {
                        return Arc::clone(state_value);
                    }
                    let mut next = state.clone();
                    let mut open = open.clone();
                    open["firstTokenTime"] = serde_json::json!(event.time);
                    next["openStep"] = open;
                    cordis::arc(next)
                }
                "assistant/message" => {
                    let open = state.get("openStep").expect("state field");
                    if open.is_null()
                        || open.get("turn") != data.get("turn")
                        || open.get("step") != data.get("step")
                    {
                        return Arc::clone(state_value);
                    }
                    let start_time = open.get("startTime").and_then(|t| t.as_i64()).unwrap_or(0);
                    let mut next = state.clone();
                    next["llmMs"] = serde_json::json!(number(state, "llmMs") + (event.time - start_time).max(0) as u64);
                    next["openStep"] = serde_json::Value::Null;
                    let first_token = open.get("firstTokenTime").and_then(|t| t.as_i64());
                    if let Some(first_token) = first_token {
                        next["ttftMs"] = serde_json::json!(number(state, "ttftMs") + (first_token - start_time).max(0) as u64);
                        next["ttftSteps"] = serde_json::json!(number(state, "ttftSteps") + 1);
                        if let Some(output) = usage_output_tokens(data.get("usage")) {
                            next["decodeMs"] = serde_json::json!(number(state, "decodeMs") + (event.time - first_token).max(0) as u64);
                            next["decodeTokens"] = serde_json::json!(number(state, "decodeTokens") + output);
                        }
                    }
                    cordis::arc(next)
                }
                "tool/call" => {
                    let Some(call_id) = data.get("callId").and_then(|v| v.as_str()) else {
                        return Arc::clone(state_value);
                    };
                    let mut next = state.clone();
                    next["pendingCalls"][call_id] = serde_json::json!(event.time);
                    cordis::arc(next)
                }
                "tool/result" => {
                    let Some(call_id) = data
                        .get("message")
                        .and_then(|m| m.get("source"))
                        .and_then(|s| s.get("callId"))
                        .and_then(|v| v.as_str())
                    else {
                        return Arc::clone(state_value);
                    };
                    let dispatched = state
                        .get("pendingCalls")
                        .and_then(|calls| calls.get(call_id))
                        .and_then(|t| t.as_i64());
                    let Some(dispatched) = dispatched else {
                        return Arc::clone(state_value);
                    };
                    let mut next = state.clone();
                    next["toolMs"] = serde_json::json!(number(state, "toolMs") + (event.time - dispatched).max(0) as u64);
                    next["pendingCalls"]
                        .as_object_mut()
                        .expect("pendingCalls object")
                        .remove(call_id);
                    cordis::arc(next)
                }
                "step/end" => {
                    let turn = data.get("turn");
                    let mut next = state.clone();
                    if state.get("lastTurn") == turn {
                        // same turn: turns unchanged
                    } else {
                        next["turns"] = serde_json::json!(number(state, "turns") + 1);
                    }
                    next["steps"] = serde_json::json!(number(state, "steps") + 1);
                    next["lastTurn"] = turn.cloned().unwrap_or(serde_json::Value::Null);
                    next["openStep"] = serde_json::Value::Null;
                    cordis::arc(next)
                }
                "turn/end" => {
                    let pending = state
                        .get("pendingCalls")
                        .and_then(|calls| calls.as_object())
                        .expect("pendingCalls object");
                    if pending.is_empty() {
                        Arc::clone(state_value)
                    } else {
                        let mut next = state.clone();
                        next["pendingCalls"] = serde_json::json!({});
                        cordis::arc(next)
                    }
                }
                _ => Arc::clone(state_value),
            }
        });
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(|state_value: &ArcValue| {
        let state: &serde_json::Value = cordis::downcast(state_value).expect("sessionStats state");
        cordis::arc(serde_json::json!({
            "turns": state.get("turns"),
            "steps": state.get("steps"),
            "llmMs": state.get("llmMs"),
            "toolMs": state.get("toolMs"),
            "ttftMs": state.get("ttftMs"),
            "ttftSteps": state.get("ttftSteps"),
            "decodeMs": state.get("decodeMs"),
            "decodeTokens": state.get("decodeTokens"),
        }))
    });
    let schema: Arc<dyn Fn(&ArcValue) -> Result<serde_json::Value, String> + Send + Sync> =
        Arc::new(|value: &ArcValue| {
            let value: &serde_json::Value =
                cordis::downcast(value).ok_or_else(|| "view must produce a JSON value".to_string())?;
            let expected = [
                "turns", "steps", "llmMs", "toolMs", "ttftMs", "ttftSteps", "decodeMs", "decodeTokens",
            ];
            for key in expected {
                let field = value
                    .get(key)
                    .ok_or_else(|| format!("sessionStats view missing {key}"))?;
                if key == "ttftSteps" || key == "steps" || key == "turns" || key == "decodeTokens" {
                    if field.as_u64().is_none() {
                        return Err(format!("sessionStats view field {key} must be a non-negative integer"));
                    }
                } else if field.as_u64().is_none() {
                    return Err(format!("sessionStats view field {key} must be a non-negative number"));
                }
            }
            if !value.is_object() || value.as_object().map(|object| object.len()).unwrap_or(0) != expected.len() {
                return Err("sessionStats view carries unexpected keys".to_string());
            }
            let _ = SessionStatsProjection::from_wire(value)?;
            Ok(value.clone())
        });
    ProjectionDefinition {
        key: "sessionStats".to_string(),
        schema,
        init,
        apply,
        view,
        state_version: 1,
    }
}
