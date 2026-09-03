//! Strict session projection of the Schedule domain's active reminder set.

use std::sync::Arc;

use cordis::{ArcValue, arc, downcast};
use dsh_session::SessionEvent;
use dsh_session_projection::{ProjectionApply, ProjectionDefinition};
use serde_json::Value;

use crate::domain::{FoldedSchedules, apply_change, decode_schedule_change};
use crate::types::{ScheduleId, ScheduleRecord};

fn empty_state() -> Value {
    serde_json::json!({
        "active": [],
        "seenIds": [],
    })
}

fn folded_from_state(state: &Value) -> Result<FoldedSchedules, String> {
    let object = state
        .as_object()
        .ok_or_else(|| "schedule projection state must be an object".to_string())?;
    if object.len() != 2 || !object.contains_key("active") || !object.contains_key("seenIds") {
        return Err("schedule projection state carries unexpected keys".to_string());
    }

    let active: Vec<ScheduleRecord> = serde_json::from_value(
        object
            .get("active")
            .cloned()
            .ok_or_else(|| "schedule projection state is missing active".to_string())?,
    )
    .map_err(|error| format!("schedule projection active is invalid: {error}"))?;
    let seen_ids: Vec<ScheduleId> = serde_json::from_value(
        object
            .get("seenIds")
            .cloned()
            .ok_or_else(|| "schedule projection state is missing seenIds".to_string())?,
    )
    .map_err(|error| format!("schedule projection seenIds is invalid: {error}"))?;

    let mut unique_seen = std::collections::HashSet::new();
    for id in &seen_ids {
        if !unique_seen.insert(id.as_str().to_string()) {
            return Err("schedule projection seen ids must be unique".to_string());
        }
    }
    let mut unique_active = std::collections::HashSet::new();
    for record in &active {
        let id = record.id().as_str().to_string();
        if !unique_seen.contains(&id) {
            return Err("every active schedule id must have been seen".to_string());
        }
        if !unique_active.insert(id) {
            return Err("active schedule ids must be unique".to_string());
        }
    }
    Ok(FoldedSchedules { active, seen_ids })
}

fn state_from_folded(folded: &FoldedSchedules) -> Value {
    serde_json::json!({
        "active": folded.active,
        "seenIds": folded.seen_ids,
    })
}

fn validate_view(value: &ArcValue) -> Result<Value, String> {
    let json = downcast::<Value>(value)
        .ok_or_else(|| "schedule projection view must be JSON".to_string())?;
    let records: Vec<ScheduleRecord> = serde_json::from_value(json.clone())
        .map_err(|error| format!("schedule projection view is invalid: {error}"))?;
    Ok(serde_json::to_value(records).expect("schedule records serialize"))
}

/// Projection definition sharing the Schedule domain's strict transition authority.
pub fn schedule_projection_definition() -> ProjectionDefinition {
    let apply: ProjectionApply = Arc::new(|state_value, event: &SessionEvent| {
        let state = downcast::<Value>(state_value).expect("schedule projection state must be JSON");
        if event.type_ != "schedule/change" {
            return Arc::clone(state_value);
        }
        let mut folded = folded_from_state(state).expect("schedule projection state must be valid");
        let change = decode_schedule_change(&event.data)
            .expect("schedule projection rejected durable event: malformed schedule/change");
        apply_change(&mut folded, &change)
            .expect("schedule projection rejected durable event: invalid Schedule transition");
        arc(state_from_folded(&folded))
    });
    ProjectionDefinition {
        key: "schedule".to_string(),
        schema: Arc::new(validate_view),
        init: Arc::new(|_header| arc(empty_state())),
        apply,
        view: Arc::new(|state_value| {
            let state =
                downcast::<Value>(state_value).expect("schedule projection state must be JSON");
            let folded = folded_from_state(state).expect("schedule projection state must be valid");
            arc(serde_json::to_value(folded.active).expect("schedule records serialize"))
        }),
        // v3 removes the obsolete physical seedLength cut. Projections fold
        // the logical child log, including inherited events, so forked state
        // remains visible and only future schedule changes mutate the child.
        state_version: 3,
    }
}
