//! Pure fold for the heuristic context-composition projection. Rust port of
//! `packages/llm/token-meter/src/breakdown-projection.ts`.

use std::sync::Arc;

use cordis::{ArcValue, arc};
use dsh_session::SessionEvent;
use dsh_session_projection::{ProjectionApply, ProjectionDefinition};
use serde_json::Value;

use crate::estimate::estimate_message;
use crate::estimate::{estimate_system_tokens, estimate_tools_tokens};
use dsh_session::surface::{derive_event_message, is_surface_event};

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
    let init: Arc<dyn Fn(&dsh_session::SessionHeader) -> ArcValue + Send + Sync> = Arc::new(|_| {
        arc(serde_json::json!({
            "systemTokens": 0,
            "toolsTokens": 0,
            "messageTokens": 0,
        }))
    });
    let apply: ProjectionApply = Arc::new(|state_value: &ArcValue, event: &SessionEvent| {
        let state: &Value = cordis::downcast(state_value).expect("contextBreakdown state");
        if event.type_ != "request/header" && !is_surface_event(event) {
            return Arc::clone(state_value);
        }
        // Active surface positions and prices only; never retain message text,
        // images or credentials in the composition cache.
        let mut nodes: Vec<(u64, u64, bool)> = state
            .get("nodes")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let mut head_system = state
            .get("headerSystem")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let mut tools_tokens = state
            .get("toolsTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if event.type_ == "request/header" {
            let canonical: Option<dsh_session::EpochHeader> = event
                .data
                .get("header")
                .and_then(|header| serde_json::from_value(header.clone()).ok());
            head_system = estimate_system_tokens(canonical.as_ref());
            tools_tokens = estimate_tools_tokens(canonical.as_ref());
        } else {
            let message = derive_event_message(event);
            let node = (
                event.seq.get(),
                message.as_ref().map(estimate_message).unwrap_or(0),
                message
                    .as_ref()
                    .is_some_and(|message| message.role == dsh_llm::Role::System),
            );
            match event.surface_op {
                Some(dsh_session::SurfaceOp::Append) => {
                    if event.type_ == "system/message" && event.data["prefix"] == true {
                        nodes.insert(0, node);
                    } else {
                        nodes.push(node);
                    }
                }
                Some(dsh_session::SurfaceOp::Replace { start, end }) => {
                    let first = nodes.iter().position(|node| node.0 == start);
                    let last = nodes.iter().position(|node| node.0 == end);
                    let (Some(first), Some(last)) = (first, last) else {
                        return Arc::clone(state_value);
                    };
                    if first > last {
                        return Arc::clone(state_value);
                    }
                    nodes.splice(first..=last, [node]);
                }
                None => return Arc::clone(state_value),
            }
        }
        let system_tokens = head_system
            + nodes
                .iter()
                .filter(|node| node.2)
                .map(|node| node.1)
                .sum::<u64>();
        let message_tokens = nodes
            .iter()
            .filter(|node| !node.2)
            .map(|node| node.1)
            .sum::<u64>();
        let next = serde_json::json!({
            "headerSystem":head_system,"nodes":nodes,
            "systemTokens":system_tokens,"toolsTokens":tools_tokens,"messageTokens":message_tokens
        });
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
        state_version: 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(seq: u64, text: &str, system: bool, op: dsh_session::SurfaceOp) -> SessionEvent {
        let message = dsh_llm::create_message(
            if system {
                dsh_llm::Role::System
            } else {
                dsh_llm::Role::User
            },
            if text.is_empty() {
                vec![]
            } else {
                vec![dsh_llm::ContentBlock::Text { text: text.into() }]
            },
            dsh_llm::MessageSource::Plugin {
                plugin: "fixture".into(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        );
        serde_json::from_value(json!({"seq":seq,"time":0,"type":if system {"system/message"}else{"user/message"},
            "data":if system {json!({"message":message,"prefix":seq==0})}else{json!(message)},"surfaceOp":op})).unwrap()
    }

    #[test]
    fn v3_system_prices_follow_append_clear_and_positional_compaction_without_double_counting() {
        use dsh_session::SurfaceOp::{Append, Replace};
        let definition = context_breakdown_projection_definition();
        let mut state = arc(json!({"systemTokens":0,"toolsTokens":0,"messageTokens":0}));
        let mut apply = |event: SessionEvent| {
            state = (definition.apply)(&state, &event);
            cordis::downcast::<Value>(&(definition.view)(&state))
                .unwrap()
                .clone()
        };
        let first = apply(event(0, "System instruction", true, Append));
        assert!(first["systemTokens"].as_u64().unwrap() > 0);
        assert_eq!(first["messageTokens"], 0);
        let user = apply(event(1, "Task", false, Append));
        let changed = apply(event(
            2,
            "Longer system instruction with new rules",
            true,
            Replace { start: 0, end: 0 },
        ));
        assert!(changed["systemTokens"].as_u64() > first["systemTokens"].as_u64());
        assert_eq!(changed["messageTokens"], user["messageTokens"]);
        let tail = apply(event(3, "Updated instruction", true, Append));
        assert!(tail["systemTokens"].as_u64() > changed["systemTokens"].as_u64());
        // Surface order is [2,1,3], unlike log order. An old compactor may have
        // replaced a region containing systems; replays must still price it.
        let compacted = apply(event(4, "Checkpoint", false, Replace { start: 2, end: 3 }));
        assert_eq!(compacted["systemTokens"], 0);
        assert!(compacted["messageTokens"].as_u64().unwrap() > 0);
        let restored = apply(event(5, "Current instruction", true, Append));
        let cleared = apply(event(6, "", true, Replace { start: 5, end: 5 }));
        assert_eq!(cleared["systemTokens"], 0);
        assert_eq!(cleared["messageTokens"], restored["messageTokens"]);
    }
}
