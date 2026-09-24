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
//! Model time sums monotonic network durations from terminal request phases,
//! including failed, cancelled and superseded attempts. Legacy records without
//! request timing fall back to step start → message or the closing boundary.
//! First token is the first
//! non-empty delta chunk and survives an in-step `llm/retry`, decode spans
//! first token → assembled message on steps that also report output tokens,
//! and tool time pairs `tool/call` → `tool/result` by callId.

use std::sync::Arc;

use cordis::ArcValue;
use dsh_session::SessionEvent;
use dsh_session_projection::ProjectionDefinition;

use crate::types::SessionStatsProjection;

#[cfg(test)]
mod measurement_tests {
    use super::*;
    use serde_json::{Value, json};
    #[test]
    fn request_rates_pair_tokens_with_positive_monotonic_duration_and_keep_sources() {
        let definition = session_stats_projection_definition();
        let header=serde_json::from_value(json!({"version":dsh_session::SESSION_FORMAT_VERSION,"id":"rate-test","createdAt":0,"isSeeded":false})).unwrap();
        let mut state = (definition.init)(&header);
        let mut seq = 0;
        let mut apply = |data: Value| {
            let event: SessionEvent = serde_json::from_value(
                json!({"seq":seq,"time":0,"type":"request/phase","data":data}),
            )
            .unwrap();
            seq += 1;
            state = (definition.apply)(&state, &event);
        };
        let sample = |instance: &str, attempt: &str, duration: i64, tokens: u64| json!({"turn":1,"step":1,"phase":"completed","measurement":"request-average","provider":"fixture","model":"model","executionInstanceId":instance,"attemptId":attempt,"networkElapsedMs":duration,"outputTokens":tokens});
        apply(sample("host-a", "zero", 0, 999999));
        apply(sample("host-a", "negative", -1, 999999));
        apply(sample("host-a", "request", 10000, 900));
        apply(sample("host-a", "request", 10000, 900));
        apply(sample("host-b", "request", 10000, 900));
        let view = (definition.view)(&state);
        let value: &Value = cordis::downcast(&view).unwrap();
        assert_eq!(value["requestSamples"], 2);
        assert_eq!(value["requestMs"], 20000);
        assert_eq!(value["requestOutputTokens"], 1800);
        assert_eq!(value["requestSources"].as_array().unwrap().len(), 2);
        assert_eq!(
            value["requestOutputTokens"].as_f64().unwrap()
                / (value["requestMs"].as_f64().unwrap() / 1000.0),
            90.0
        );
        (definition.schema)(&view).unwrap();
    }
}

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

fn measured_attempt(data: &serde_json::Value) -> Option<serde_json::Value> {
    if data["measurement"] != "request-average" {
        return None;
    }
    let instance = data["executionInstanceId"]
        .as_str()
        .filter(|v| !v.is_empty())?;
    let attempt = data["attemptId"].as_str().filter(|v| !v.is_empty())?;
    let duration = data.get("networkElapsedMs")?;
    if !duration.is_null() && duration.as_u64().is_none() {
        return None;
    }
    Some(serde_json::json!([
        instance,
        attempt,
        data["turn"],
        data["step"]
    ]))
}

fn legacy_model_elapsed(open: &serde_json::Value, end: i64) -> u64 {
    if open["hasRequestTiming"] == true {
        return 0;
    }
    open["startTime"]
        .as_i64()
        .map(|start| end.saturating_sub(start).max(0) as u64)
        .unwrap_or(0)
}

/// The `sessionStats` unit registered on `ctx.sessionProjections`
/// (exported for the unit spec).
pub fn session_stats_projection_definition() -> ProjectionDefinition {
    let init: Arc<dyn Fn(&dsh_session::SessionHeader) -> ArcValue + Send + Sync> = Arc::new(|_| {
        cordis::arc(serde_json::json!({
            "turns": 0, "steps": 0,
            "llmMs": 0, "toolMs": 0, "ttftMs": 0, "ttftSteps": 0,
            "decodeMs": 0, "decodeTokens": 0,
            "requestMs":0,"requestOutputTokens":0,"requestSamples":0,"requestSources":{},"requestPhase":null,"lastMeasuredAttempt":null,"lastTimedAttempt":null,
            "lastTurn": serde_json::Value::Null,
            "openStep": serde_json::Value::Null,
            "pendingCalls": {},
        }))
    });
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> = Arc::new(
        move |state_value: &ArcValue, event: &SessionEvent| {
            let state: &serde_json::Value =
                cordis::downcast(state_value).expect("sessionStats state");
            let data = &event.data;
            match event.type_.as_str() {
                "request/phase" => {
                    let mut next = state.clone();
                    next["requestPhase"] = data.clone();
                    if let Some(identity) = measured_attempt(data) {
                        let open = &state["openStep"];
                        if !open.is_null()
                            && open.get("turn") == data.get("turn")
                            && open.get("step") == data.get("step")
                        {
                            next["openStep"]["hasRequestTiming"] = serde_json::json!(true);
                        }
                        if matches!(
                            data["phase"].as_str(),
                            Some("completed" | "failed" | "cancelled" | "superseded")
                        ) && state["lastTimedAttempt"] != identity
                        {
                            next["lastTimedAttempt"] = identity;
                            next["llmMs"] = serde_json::json!(
                                number(state, "llmMs")
                                    .saturating_add(data["networkElapsedMs"].as_u64().unwrap_or(0))
                            );
                        }
                    }
                    if data["phase"] == "completed" && data["measurement"] == "request-average" {
                        if let (
                            Some(duration),
                            Some(tokens),
                            Some(attempt),
                            Some(instance),
                            Some(provider),
                            Some(model),
                        ) = (
                            data["networkElapsedMs"].as_u64().filter(|n| *n > 0),
                            data["outputTokens"].as_u64(),
                            data["attemptId"].as_str().filter(|v| !v.is_empty()),
                            data["executionInstanceId"]
                                .as_str()
                                .filter(|v| !v.is_empty()),
                            data["provider"].as_str(),
                            data["model"].as_str(),
                        ) {
                            let identity =
                                serde_json::json!([instance, attempt, data["turn"], data["step"]]);
                            if state["lastMeasuredAttempt"] != identity {
                                next["lastMeasuredAttempt"] = identity;
                                next["requestMs"] = serde_json::json!(
                                    number(state, "requestMs").saturating_add(duration)
                                );
                                next["requestOutputTokens"] = serde_json::json!(
                                    number(state, "requestOutputTokens").saturating_add(tokens)
                                );
                                next["requestSamples"] = serde_json::json!(
                                    number(state, "requestSamples").saturating_add(1)
                                );
                                let key =
                                    serde_json::json!([instance, provider, model]).to_string();
                                let mut source = state["requestSources"][&key].clone();
                                if source.is_null() {
                                    source = serde_json::json!({"executionInstanceId":instance,"provider":provider,"model":model,"durationMs":0,"outputTokens":0,"samples":0});
                                }
                                source["durationMs"] = serde_json::json!(
                                    source["durationMs"]
                                        .as_u64()
                                        .unwrap_or(0)
                                        .saturating_add(duration)
                                );
                                source["outputTokens"] = serde_json::json!(
                                    source["outputTokens"]
                                        .as_u64()
                                        .unwrap_or(0)
                                        .saturating_add(tokens)
                                );
                                source["samples"] = serde_json::json!(
                                    source["samples"].as_u64().unwrap_or(0).saturating_add(1)
                                );
                                next["requestSources"][key] = source;
                            }
                        }
                    }
                    cordis::arc(next)
                }
                "step/start" => {
                    let mut next = state.clone();
                    next["requestPhase"] = serde_json::Value::Null;
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
                        Some(chunk) => {
                            match serde_json::from_value::<dsh_llm::StreamChunk>(chunk) {
                                Ok(chunk) => chunk,
                                Err(_) => return Arc::clone(state_value),
                            }
                        }
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
                    next["llmMs"] = serde_json::json!(
                        number(state, "llmMs")
                            .saturating_add(legacy_model_elapsed(open, event.time))
                    );
                    next["openStep"] = serde_json::Value::Null;
                    let first_token = open.get("firstTokenTime").and_then(|t| t.as_i64());
                    if let Some(first_token) = first_token {
                        next["ttftMs"] = serde_json::json!(
                            number(state, "ttftMs") + (first_token - start_time).max(0) as u64
                        );
                        next["ttftSteps"] = serde_json::json!(number(state, "ttftSteps") + 1);
                        if let Some(output) = usage_output_tokens(data.get("usage"))
                            && event.time > first_token
                        {
                            next["decodeMs"] = serde_json::json!(
                                number(state, "decodeMs")
                                    + (event.time - first_token).max(0) as u64
                            );
                            next["decodeTokens"] =
                                serde_json::json!(number(state, "decodeTokens") + output);
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
                    next["toolMs"] = serde_json::json!(
                        number(state, "toolMs") + (event.time - dispatched).max(0) as u64
                    );
                    next["pendingCalls"]
                        .as_object_mut()
                        .expect("pendingCalls object")
                        .remove(call_id);
                    cordis::arc(next)
                }
                "step/end" => {
                    let turn = data.get("turn");
                    let mut next = state.clone();
                    let open = &state["openStep"];
                    if !open.is_null()
                        && open.get("turn") == turn
                        && open.get("step") == data.get("step")
                    {
                        next["llmMs"] = serde_json::json!(
                            number(state, "llmMs")
                                .saturating_add(legacy_model_elapsed(open, event.time))
                        );
                    }
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
                    let open = &state["openStep"];
                    let closes_step = !open.is_null() && open.get("turn") == data.get("turn");
                    if pending.is_empty() && state["requestPhase"].is_null() && !closes_step {
                        Arc::clone(state_value)
                    } else {
                        let mut next = state.clone();
                        if closes_step {
                            next["llmMs"] = serde_json::json!(
                                number(state, "llmMs")
                                    .saturating_add(legacy_model_elapsed(open, event.time))
                            );
                            next["openStep"] = serde_json::Value::Null;
                        }
                        next["requestPhase"] = serde_json::Value::Null;
                        next["pendingCalls"] = serde_json::json!({});
                        cordis::arc(next)
                    }
                }
                _ => Arc::clone(state_value),
            }
        },
    );
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(
        |state_value: &ArcValue| {
            let state: &serde_json::Value =
                cordis::downcast(state_value).expect("sessionStats state");
            cordis::arc(serde_json::json!({
                "turns": state.get("turns"),
                "steps": state.get("steps"),
                "llmMs": state.get("llmMs"),
                "toolMs": state.get("toolMs"),
                "ttftMs": state.get("ttftMs"),
                "ttftSteps": state.get("ttftSteps"),
                "decodeMs": state.get("decodeMs"),
                "decodeTokens": state.get("decodeTokens"),
                "requestMs":state.get("requestMs"),
                "requestOutputTokens":state.get("requestOutputTokens"),
                "requestSamples":state.get("requestSamples"),
                "requestSources":state["requestSources"].as_object().map(|sources|sources.values().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                "requestPhase":state.get("requestPhase"),
            }))
        },
    );
    let schema: Arc<dyn Fn(&ArcValue) -> Result<serde_json::Value, String> + Send + Sync> =
        Arc::new(|value: &ArcValue| {
            let value: &serde_json::Value = cordis::downcast(value)
                .ok_or_else(|| "view must produce a JSON value".to_string())?;
            let expected = [
                "turns",
                "steps",
                "llmMs",
                "toolMs",
                "ttftMs",
                "ttftSteps",
                "decodeMs",
                "decodeTokens",
                "requestMs",
                "requestOutputTokens",
                "requestSamples",
            ];
            for key in expected {
                let field = value
                    .get(key)
                    .ok_or_else(|| format!("sessionStats view missing {key}"))?;
                if key == "ttftSteps" || key == "steps" || key == "turns" || key == "decodeTokens" {
                    if field.as_u64().is_none() {
                        return Err(format!(
                            "sessionStats view field {key} must be a non-negative integer"
                        ));
                    }
                } else if field.as_u64().is_none() {
                    return Err(format!(
                        "sessionStats view field {key} must be a non-negative number"
                    ));
                }
            }
            if !value.is_object()
                || value.as_object().map(|object| object.len()).unwrap_or(0) != expected.len() + 2
            {
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
        state_version: 3,
    }
}
