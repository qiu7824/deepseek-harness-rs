//! JSON checkpoint and live-view adapter for durable goal state.
use std::sync::Arc;

use cordis::{ArcValue, Context, arc, downcast};
use dsh_session_projection::{ProjectionDefinition, SessionProjectionRegistry};
use serde_json::{Value, json};

use crate::{GOAL_CHANGE_VERSION, apply_goal_requirements_projection, decode_goal_change};

pub fn goal_projection_definition() -> ProjectionDefinition {
    ProjectionDefinition {
        key: "goal".into(),
        state_version: 2,
        init: Arc::new(|_| arc(Value::Null)),
        apply: Arc::new(|state, event| {
            let value = downcast::<Value>(state).expect("goal projection JSON state");
            match apply_goal_requirements_projection(value, event) {
                Some(next) if &next != value => arc(next),
                _ => state.clone(),
            }
        }),
        view: Arc::new(|state| state.clone()),
        schema: Arc::new(|value: &ArcValue| {
            let value = downcast::<Value>(value).ok_or("goal projection must be JSON")?;
            if value.is_null() {
                return Ok(Value::Null);
            }
            let object = value
                .as_object()
                .ok_or("goal projection must be an object or null")?;
            if object.len() != 4 {
                return Err("goal projection has unexpected fields".into());
            }
            let mut change = object.clone();
            let goal = change
                .get_mut("goal")
                .and_then(Value::as_object_mut)
                .ok_or("goal projection has no goal")?;
            let requirements_revision = goal
                .remove("objectiveRevision")
                .and_then(|value| value.as_u64())
                .filter(|revision| *revision > 0)
                .ok_or("goal requirements revision is missing")?;
            if goal
                .get("revision")
                .and_then(Value::as_u64)
                .is_none_or(|revision| requirements_revision > revision)
            {
                return Err("goal requirements revision exceeds the goal revision".into());
            }
            change.insert("kind".into(), json!("goal/change"));
            change.insert("version".into(), json!(GOAL_CHANGE_VERSION));
            change.insert("operation".into(), json!("edit"));
            decode_goal_change(&Value::Object(change))?;
            Ok(value.clone())
        }),
    }
}

pub fn register_goal_projection(ctx: &Context) -> Result<cordis::Disposer, String> {
    let registry = ctx
        .get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
        .ok_or("sessionProjections service is not configured")?;
    registry.register(ctx, goal_projection_definition())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::SessionEvent;

    fn change(seq: u64, revision: u64, phase: &str, operation: &str) -> SessionEvent {
        serde_json::from_value(json!({"seq":seq,"time":100+seq,"type":"goal/change","data":{
            "kind":"goal/change","version":1,"operation":operation,
            "goal":{"id":"goal-test","revision":revision,"objective":"Complete the task","phase":phase,"maxGoalRounds":8},
            "roundsStarted":2,"createdAt":100,"updatedAt":100+seq
        }})).unwrap()
    }

    #[tokio::test]
    async fn registered_goal_view_replays_pause_edit_and_clear_without_agent_activation() {
        let ctx = Context::root();
        let registry = SessionProjectionRegistry::install(&ctx);
        let release = register_goal_projection(&ctx).unwrap();
        assert!(registry.keys().contains(&"goal".to_string()));
        let definition = goal_projection_definition();
        let mut state = arc(Value::Null);
        for event in [
            change(1, 1, "active", "create"),
            change(2, 2, "paused", "pause"),
            change(3, 3, "paused", "edit"),
        ] {
            state = (definition.apply)(&state, &event);
        }
        let value = (definition.schema)(&(definition.view)(&state)).unwrap();
        assert_eq!(value["goal"]["phase"], "paused");
        assert_eq!(value["goal"]["revision"], 3);
        assert_eq!(value["goal"]["objectiveRevision"], 1);
        assert_eq!(value["roundsStarted"], 2);
        let detached =
            arc(serde_json::from_slice::<Value>(&serde_json::to_vec(&value).unwrap()).unwrap());
        assert_eq!((definition.schema)(&detached).unwrap(), value);
        let clear: SessionEvent = serde_json::from_value(json!({"seq":4,"time":104,"type":"goal/change","data":{
            "kind":"goal/change","version":1,"operation":"clear","cleared":{"id":"goal-test","revision":3},"clearedAt":104
        }})).unwrap();
        state = (definition.apply)(&state, &clear);
        assert_eq!((definition.schema)(&state).unwrap(), Value::Null);
        release().await;
        assert!(!registry.keys().contains(&"goal".to_string()));
    }

    #[test]
    fn malformed_or_unrelated_events_keep_the_same_state_reference() {
        let definition = goal_projection_definition();
        let state = (definition.apply)(&arc(Value::Null), &change(1, 1, "active", "create"));
        let mut invalid = change(2, 2, "paused", "pause");
        invalid.data["goal"]["objective"] = json!("");
        assert!(Arc::ptr_eq(&state, &(definition.apply)(&state, &invalid)));
        invalid.type_ = "assistant/chunk".into();
        assert!(Arc::ptr_eq(&state, &(definition.apply)(&state, &invalid)));
        assert!((definition.schema)(&arc(json!({"goal":{}}))).is_err());
    }
}
