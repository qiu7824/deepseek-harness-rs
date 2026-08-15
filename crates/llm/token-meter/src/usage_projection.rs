//! Pure folds for durable provider-reported token usage and context
//! occupancy. Rust port of `packages/llm/token-meter/src/usage-projection.ts`.

use std::sync::Arc;

use cordis::{ArcValue, arc};
use dsh_session::SessionEvent;
use dsh_session_projection::ProjectionDefinition;
use serde_json::Value;

use crate::surface_projection::{ShadowPriceClaim, fold_surface_projection};
use crate::types::TokenUsageProjection;

fn zero_buckets() -> Value {
    serde_json::json!({
        "uncachedInputTokens": 0,
        "outputTokens": 0,
        "cacheReadTokens": 0,
        "cacheWriteTokens": 0,
    })
}

fn buckets_from(usage: &Value) -> Value {
    serde_json::json!({
        "uncachedInputTokens": usage.get("inputTokens").and_then(|v| v.as_u64()).unwrap_or(0),
        "outputTokens": usage.get("outputTokens").and_then(|v| v.as_u64()).unwrap_or(0),
        "cacheReadTokens": usage.get("cacheReadTokens").and_then(|v| v.as_u64()).unwrap_or(0),
        "cacheWriteTokens": usage.get("cacheWriteTokens").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

fn buckets_equal(left: &Value, right: &Value) -> bool {
    left == right
}

fn add_replacing(totals: &Value, previous: Option<&Value>, next: &Value) -> Value {
    let subtract = |key: &str| -> u64 {
        let total = totals.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        let previous = previous
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let next = next.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        total.saturating_sub(previous) + next
    };
    serde_json::json!({
        "uncachedInputTokens": subtract("uncachedInputTokens"),
        "outputTokens": subtract("outputTokens"),
        "cacheReadTokens": subtract("cacheReadTokens"),
        "cacheWriteTokens": subtract("cacheWriteTokens"),
    })
}

fn validate_projection_schema(value: &Value) -> Result<Value, String> {
    for key in [
        "uncachedInputTokens",
        "outputTokens",
        "cacheReadTokens",
        "cacheWriteTokens",
    ] {
        if value.get(key).and_then(|v| v.as_u64()).is_none() {
            return Err(format!("tokenUsage view field {key} must be a non-negative integer"));
        }
    }
    if !value.is_object() || value.as_object().map(|o| o.len()).unwrap_or(0) != 4 {
        return Err("tokenUsage view carries unexpected keys".to_string());
    }
    Ok(value.clone())
}

/// Token-meter's session projection unit (TS `tokenUsageProjectionDefinition`).
pub fn token_usage_projection_definition() -> ProjectionDefinition {
    let init: Arc<dyn Fn() -> ArcValue + Send + Sync> = Arc::new(|| {
        arc(serde_json::json!({"totals": zero_buckets(), "last": Value::Null}))
    });
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> =
        Arc::new(|state_value: &ArcValue, event: &SessionEvent| {
            let state: &Value = cordis::downcast(state_value).expect("tokenUsage state");
            let (turn, step, usage): (u64, u64, &Value) = if event.type_ == "assistant/chunk" {
                let chunk = event.data.get("chunk");
                if chunk.and_then(|c| c.get("type")).and_then(|t| t.as_str()) == Some("usage") {
                    let turn = event.data.get("turn").and_then(|v| v.as_u64());
                    let step = event.data.get("step").and_then(|v| v.as_u64());
                    let usage = chunk.and_then(|c| c.get("usage"));
                    match (turn, step, usage) {
                        (Some(turn), Some(step), Some(usage)) => (turn, step, usage),
                        _ => return Arc::clone(state_value),
                    }
                } else {
                    return Arc::clone(state_value);
                }
            } else if event.type_ == "assistant/message" {
                let usage = event.data.get("usage");
                let turn = event.data.get("turn").and_then(|v| v.as_u64());
                let step = event.data.get("step").and_then(|v| v.as_u64());
                match (turn, step, usage) {
                    (Some(turn), Some(step), Some(usage)) => (turn, step, usage),
                    _ => return Arc::clone(state_value),
                }
            } else {
                return Arc::clone(state_value);
            };

            let buckets = buckets_from(usage);
            let last = state.get("last").and_then(|l| l.as_object());
            let previous = match last {
                Some(last)
                    if last.get("turn").and_then(|v| v.as_u64()) == Some(turn)
                        && last.get("step").and_then(|v| v.as_u64()) == Some(step) =>
                {
                    last.get("buckets")
                }
                _ => None,
            };
            if previous.is_some_and(|previous| buckets_equal(previous, &buckets)) {
                return Arc::clone(state_value);
            }
            let totals = add_replacing(state.get("totals").expect("totals"), previous, &buckets);
            arc(serde_json::json!({
                "totals": totals,
                "last": {"turn": turn, "step": step, "buckets": buckets},
            }))
        });
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(|state_value| {
        let state: &Value = cordis::downcast(state_value).expect("tokenUsage state");
        let totals = state.get("totals").cloned().unwrap_or_else(zero_buckets);
        arc(totals)
    });
    ProjectionDefinition {
        key: "tokenUsage".to_string(),
        schema: Arc::new(|value: &ArcValue| {
            let value: &Value = cordis::downcast(value).ok_or_else(|| "view must be JSON".to_string())?;
            validate_projection_schema(value)
        }),
        init,
        apply,
        view,
        state_version: 1,
    }
}

/// Prompt-side pressure of one request: input plus cache traffic, no output.
fn pressure_from(usage: &Value) -> u64 {
    usage.get("inputTokens").and_then(|v| v.as_u64()).unwrap_or(0)
        + usage.get("cacheReadTokens").and_then(|v| v.as_u64()).unwrap_or(0)
        + usage.get("cacheWriteTokens").and_then(|v| v.as_u64()).unwrap_or(0)
}

/// The usage a chunk or finalized message reports for its step, if any.
fn usage_of(event: &SessionEvent) -> Option<&Value> {
    if event.type_ == "assistant/chunk" {
        let chunk = event.data.get("chunk")?;
        if chunk.get("type").and_then(|t| t.as_str()) == Some("usage") {
            return chunk.get("usage");
        }
        return None;
    }
    if event.type_ == "assistant/message" {
        return event.data.get("usage");
    }
    None
}

fn validate_pressure_schema(value: &Value) -> Result<Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "contextPressure view must be an object".to_string())?;
    for (key, field) in object {
        match key.as_str() {
            "pressureTokens" | "projectedTokens" | "contextWindow" => {
                if field.as_u64().is_none() {
                    return Err(format!("contextPressure view field {key} must be a non-negative integer"));
                }
            }
            _ => return Err(format!("contextPressure view carries unexpected key {key}")),
        }
    }
    if let Some(window) = object.get("contextWindow").and_then(|v| v.as_u64()) {
        if window == 0 {
            return Err("contextPressure view contextWindow must be positive".to_string());
        }
    }
    Ok(value.clone())
}

/// Token-meter's context-occupancy projection unit (TS
/// `contextPressureProjectionDefinition`).
pub fn context_pressure_projection_definition() -> ProjectionDefinition {
    let init: Arc<dyn Fn() -> ArcValue + Send + Sync> = Arc::new(|| {
        arc(serde_json::json!({"surfaceTokens": 0}))
    });
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> =
        Arc::new(|state_value: &ArcValue, event: &SessionEvent| {
            let state: &Value = cordis::downcast(state_value).expect("contextPressure state");
            let claim: Option<ShadowPriceClaim> = state
                .get("claim")
                .and_then(|claim| serde_json::from_value(claim.clone()).ok());
            let fold = match fold_surface_projection(claim.as_ref(), event) {
                Ok(fold) => fold,
                Err(_) => return Arc::clone(state_value),
            };
            let mut next = state.clone();
            if event.type_ == "request/context" {
                let window = event.data.get("contextWindow").and_then(|v| v.as_u64());
                if window != state.get("contextWindow").and_then(|v| v.as_u64()) {
                    match window {
                        Some(window) => {
                            next["contextWindow"] = serde_json::json!(window);
                        }
                        None => {
                            next.as_object_mut().expect("object").remove("contextWindow");
                        }
                    }
                }
            }
            if let Some(usage) = usage_of(event) {
                let pressure = pressure_from(usage);
                let surface_tokens = state.get("surfaceTokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let sampled = state.get("sampledSurfaceTokens").and_then(|v| v.as_u64());
                if state.get("pressureTokens").and_then(|v| v.as_u64()) != Some(pressure)
                    || sampled != Some(surface_tokens)
                {
                    next["pressureTokens"] = serde_json::json!(pressure);
                    next["sampledSurfaceTokens"] = serde_json::json!(surface_tokens);
                }
            }
            if fold.delta_tokens != 0 {
                let surface_tokens = next.get("surfaceTokens").and_then(|v| v.as_u64()).unwrap_or(0);
                next["surfaceTokens"] =
                    serde_json::json!((surface_tokens as i64 + fold.delta_tokens).max(0) as u64);
            }
            if state.get("claim").is_none() && fold.claim.is_none() {
                return arc(next);
            }
            next.as_object_mut().expect("object").remove("claim");
            if let Some(claim) = &fold.claim {
                next["claim"] = serde_json::to_value(claim).expect("claim JSON");
            }
            arc(next)
        });
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(|state_value| {
        let state: &Value = cordis::downcast(state_value).expect("contextPressure state");
        let mut view = serde_json::Map::new();
        if let Some(window) = state.get("contextWindow").and_then(|v| v.as_u64()) {
            view.insert("contextWindow".to_string(), serde_json::json!(window));
        }
        if let Some(pressure) = state.get("pressureTokens").and_then(|v| v.as_u64()) {
            view.insert("pressureTokens".to_string(), serde_json::json!(pressure));
            let surface = state.get("surfaceTokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let sampled = state.get("sampledSurfaceTokens").and_then(|v| v.as_u64()).unwrap_or(0);
            view.insert(
                "projectedTokens".to_string(),
                serde_json::json!((pressure as i64 + surface as i64 - sampled as i64).max(0) as u64),
            );
        }
        arc(Value::Object(view))
    });
    ProjectionDefinition {
        key: "contextPressure".to_string(),
        schema: Arc::new(|value: &ArcValue| {
            let value: &Value = cordis::downcast(value).ok_or_else(|| "view must be JSON".to_string())?;
            validate_pressure_schema(value)
        }),
        init,
        apply,
        view,
        state_version: 4,
    }
}

#[allow(dead_code)]
fn token_usage_zero() -> TokenUsageProjection {
    TokenUsageProjection {
        uncached_input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    }
}
