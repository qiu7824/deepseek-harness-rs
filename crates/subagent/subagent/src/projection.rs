//! Pure session projections for subagent identity (mode/label) and
//! active-turn duration. Rust port of
//! `packages/subagent/subagent/src/projection.ts` +
//! `projection-types.ts`.

use std::sync::Arc;

use cordis::{ArcValue, arc, downcast};
use dsh_session::{SessionEvent, SessionHeader};
use dsh_session_projection::ProjectionDefinition;

/// Durable active-turn timing for one descriptor-backed child session.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentTimingProjection {
    /// Milliseconds accumulated across completed turns after the child's own
    /// descriptor.
    pub settled_ms: u64,
    /// Same-cut bounds of the currently open turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<SubagentActiveTiming>,
}

/// Bounds of one open turn inside a timing fold.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SubagentActiveTiming {
    pub since: i64,
    pub through: i64,
}

/// Durable identity of one descriptor-backed subagent session (TS closed
/// union).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum SubagentIdentityProjection {
    #[serde(rename = "one-shot")]
    OneShot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        seq: u64,
    },
    Continuable {
        label: String,
        seq: u64,
    },
}

/// The timing state cell (plain JSON).
fn timing_state(
    descriptor_seen: bool,
    settled_ms: u64,
    active: Option<serde_json::Value>,
    pending: Option<i64>,
) -> ArcValue {
    let mut value = serde_json::json!({
        "descriptorSeen": descriptor_seen,
        "settledMs": settled_ms,
    });
    if let Some(active) = active {
        value["active"] = active;
    }
    if let Some(pending) = pending {
        value["pendingTurnStart"] = serde_json::json!(pending);
    }
    arc(value)
}

/// The timing projection definition (TS
/// `subagentTimingProjectionDefinition`).
pub fn subagent_timing_projection_definition() -> ProjectionDefinition {
    ProjectionDefinition {
        key: "subagentTiming".to_string(),
        schema: Arc::new(|value: &ArcValue| {
            let parsed = downcast::<serde_json::Value>(value)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Ok(parsed)
        }),
        init: Arc::new(|_header| timing_state(false, 0, None, None)),
        apply: Arc::new(|state: &ArcValue, event: &SessionEvent| {
            let value = downcast::<serde_json::Value>(state).expect("timing state is plain JSON");
            if event.type_ == "turn/start" {
                let descriptor_seen = value["descriptorSeen"].as_bool().unwrap_or(false);
                let active = serde_json::json!({ "since": event.time, "through": event.time });
                return if descriptor_seen {
                    timing_state(
                        true,
                        value["settledMs"].as_u64().unwrap_or(0),
                        Some(active),
                        None,
                    )
                } else {
                    timing_state(
                        false,
                        value["settledMs"].as_u64().unwrap_or(0),
                        None,
                        Some(event.time),
                    )
                };
            }
            if event.type_ == "subagent/descriptor" {
                let active_since = value["active"]
                    .get("since")
                    .and_then(|v| v.as_i64())
                    .or_else(|| value["pendingTurnStart"].as_i64());
                return match active_since {
                    None => timing_state(true, 0, None, None),
                    Some(since) => timing_state(
                        true,
                        0,
                        Some(serde_json::json!({ "since": since, "through": event.time })),
                        None,
                    ),
                };
            }
            if event.type_ == "turn/end" {
                let descriptor_seen = value["descriptorSeen"].as_bool().unwrap_or(false);
                if !descriptor_seen {
                    if value["pendingTurnStart"].is_null() {
                        return state.clone();
                    }
                    return timing_state(
                        false,
                        value["settledMs"].as_u64().unwrap_or(0),
                        None,
                        None,
                    );
                }
                let Some(active) = value.get("active") else {
                    return state.clone();
                };
                let since = active
                    .get("since")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(event.time);
                let settled =
                    value["settledMs"].as_u64().unwrap_or(0) + (event.time - since).max(0) as u64;
                return timing_state(true, settled, None, None);
            }
            if let Some(_active) = value.get("active") {
                let mut next = value.clone();
                next["active"]["through"] = serde_json::json!(event.time);
                return arc(next);
            }
            state.clone()
        }),
        view: Arc::new(|state: &ArcValue| {
            let value = downcast::<serde_json::Value>(state).expect("timing state is plain JSON");
            let mut out =
                serde_json::json!({ "settledMs": value["settledMs"].as_u64().unwrap_or(0) });
            if let Some(active) = value.get("active") {
                out["active"] = active.clone();
            }
            arc(out)
        }),
        state_version: 2,
    }
}

/// Interpret one `subagent/descriptor` event's identity; no value when the
/// payload cannot be trusted.
pub(crate) fn descriptor_identity(event: &SessionEvent) -> Option<SubagentIdentityProjection> {
    let descriptor = crate::descriptor::fold_subagent_descriptor(std::slice::from_ref(event))
        .ok()
        .flatten()?;
    match descriptor {
        crate::descriptor::SubagentDescriptorData::OneShot { label, .. } => {
            Some(SubagentIdentityProjection::OneShot {
                label,
                seq: event.seq.get(),
            })
        }
        crate::descriptor::SubagentDescriptorData::Continuable { label, .. } => {
            Some(SubagentIdentityProjection::Continuable {
                label,
                seq: event.seq.get(),
            })
        }
    }
}

/// The identity projection definition (TS
/// `subagentIdentityProjectionDefinition`).
pub fn subagent_identity_projection_definition() -> ProjectionDefinition {
    ProjectionDefinition {
        key: "subagent".to_string(),
        schema: Arc::new(|value: &ArcValue| {
            let parsed = downcast::<serde_json::Value>(value)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Ok(parsed)
        }),
        init: Arc::new(|_header| arc(serde_json::json!({}))),
        apply: Arc::new(|state: &ArcValue, event: &SessionEvent| {
            if event.type_ != "subagent/descriptor" {
                return state.clone();
            }
            match descriptor_identity(event) {
                Some(identity) => arc(serde_json::json!({ "identity": identity })),
                None => arc(serde_json::json!({})),
            }
        }),
        view: Arc::new(|state: &ArcValue| {
            let value = downcast::<serde_json::Value>(state).expect("identity state is plain JSON");
            match value.get("identity") {
                Some(identity) => arc(identity.clone()),
                None => arc(serde_json::Value::Null),
            }
        }),
        state_version: 2,
    }
}

/// The lifecycle witness fields for the listing ladder (TS
/// `LIFECYCLE_WITNESS_KEYS`).
pub fn same_lifecycle(meta: &SessionHeader, expected: &SessionHeader) -> bool {
    meta.version == expected.version
        && meta.id == expected.id
        && meta.created_at == expected.created_at
        && meta.cwd == expected.cwd
        && meta.parent_session == expected.parent_session
        && meta.is_seeded == expected.is_seeded
        && meta.delegation_depth == expected.delegation_depth
}
