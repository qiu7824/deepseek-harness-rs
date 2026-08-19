//! Pure fold for the heuristic context-composition projection. Rust port of
//! `packages/llm/token-meter/src/breakdown-projection.ts`.

use std::sync::Arc;

use cordis::{ArcValue, arc};
use dsh_session::SessionEvent;
use dsh_session_projection::ProjectionDefinition;
use serde_json::Value;

use crate::estimate::{estimate_system_tokens, estimate_tools_tokens};
use crate::surface_projection::{ShadowPriceClaim, fold_surface_projection};

fn validate_breakdown_schema(value: &Value) -> Result<Value, String> {
    for key in ["systemTokens", "toolsTokens", "messageTokens"] {
        if value.get(key).and_then(|v| v.as_u64()).is_none() {
            return Err(format!(
                "contextBreakdown view field {key} must be a non-negative integer"
            ));
        }
    }
    if !value.is_object() || value.as_object().map(|o| o.len()).unwrap_or(0) != 3 {
        return Err("contextBreakdown view carries unexpected keys".to_string());
    }
    Ok(value.clone())
}

/// Token-meter's context-composition projection unit (TS
/// `contextBreakdownProjectionDefinition`).
pub fn context_breakdown_projection_definition() -> ProjectionDefinition {
    let init: Arc<dyn Fn() -> ArcValue + Send + Sync> = Arc::new(|| {
        arc(serde_json::json!({
            "systemTokens": 0,
            "toolsTokens": 0,
            "messageTokens": 0,
        }))
    });
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> =
        Arc::new(|state_value: &ArcValue, event: &SessionEvent| {
            let state: &Value = cordis::downcast(state_value).expect("contextBreakdown state");
            let claim: Option<ShadowPriceClaim> = state
                .get("claim")
                .and_then(|claim| serde_json::from_value(claim.clone()).ok());
            let fold = match fold_surface_projection(claim.as_ref(), event) {
                Ok(fold) => fold,
                Err(_) => return Arc::clone(state_value),
            };
            let mut system_tokens = state
                .get("systemTokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let mut tools_tokens = state
                .get("toolsTokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if event.type_ == "request/header" {
                let header = event.data.get("header");
                if let Some(header) = header {
                    let canonical: Option<dsh_session::EpochHeader> =
                        serde_json::from_value(header.clone()).ok();
                    system_tokens = estimate_system_tokens(canonical.as_ref());
                    tools_tokens = estimate_tools_tokens(canonical.as_ref());
                }
            }
            if system_tokens
                == state
                    .get("systemTokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                && tools_tokens
                    == state
                        .get("toolsTokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                && fold.delta_tokens == 0
                && fold.claim.is_none()
                && state.get("claim").is_none()
            {
                return Arc::clone(state_value);
            }
            let message_tokens = state
                .get("messageTokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as i64
                + fold.delta_tokens;
            let mut next = serde_json::json!({
                "systemTokens": system_tokens,
                "toolsTokens": tools_tokens,
                "messageTokens": message_tokens.max(0) as u64,
            });
            if let Some(claim) = &fold.claim {
                next["claim"] = serde_json::to_value(claim).expect("claim JSON");
            }
            arc(next)
        });
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(|state_value| {
        let state: &Value = cordis::downcast(state_value).expect("contextBreakdown state");
        arc(serde_json::json!({
            "systemTokens": state.get("systemTokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "toolsTokens": state.get("toolsTokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "messageTokens": state.get("messageTokens").and_then(|v| v.as_u64()).unwrap_or(0),
        }))
    });
    ProjectionDefinition {
        key: "contextBreakdown".to_string(),
        schema: Arc::new(|value: &ArcValue| {
            let value: &Value =
                cordis::downcast(value).ok_or_else(|| "view must be JSON".to_string())?;
            validate_breakdown_schema(value)
        }),
        init,
        apply,
        view,
        state_version: 2,
    }
}
