//! Pure replay fold and strict decoder for durable goal changes. Rust port
//! of `packages/goal/goal/src/fold.ts`.

use std::collections::HashSet;

use dsh_session::SessionEvent;
use serde_json::Value;

use crate::domain::{GoalChangeMeta, GoalClearChangeMeta, GoalOperation, GoalSnapshotChangeMeta};
use crate::runtime::GOAL_CHANGE_VERSION;
use crate::types::{GoalBlockReason, GoalPhase, GoalRef, GoalSnapshot, goal_id};

/// Mutable accumulator kept private to the pure fold (TS `GoalFoldState`).
#[derive(Debug, Clone, Default)]
pub struct GoalFoldState {
    pub goal: Option<GoalSnapshot>,
    pub rounds_started: u64,
    pub created_at: Option<u64>,
    pub updated_at: Option<u64>,
    pub last_ref: Option<GoalRef>,
    pub seen_goal_ids: HashSet<String>,
}

/// Build an empty replay accumulator (TS `emptyGoalFoldState`).
pub fn empty_goal_fold_state() -> GoalFoldState {
    GoalFoldState::default()
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

fn field_keys(value: &serde_json::Map<String, Value>) -> String {
    let mut keys: Vec<&str> = value.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys.join(",")
}

fn positive_integer(value: &Value, field: &str) -> Result<u64, String> {
    match value.as_u64() {
        Some(integer) if integer >= 1 => Ok(integer),
        _ => Err(format!(
            "goal change {field} must be a positive safe integer"
        )),
    }
}

fn non_negative_integer(value: &Value, field: &str) -> Result<u64, String> {
    match value.as_u64() {
        Some(integer) => Ok(integer),
        _ => Err(format!(
            "goal change {field} must be a non-negative safe integer"
        )),
    }
}

fn decode_block_reason(value: &Value) -> Result<GoalBlockReason, String> {
    let Some(object) = value.as_object() else {
        return Err(
            "goal change goal.blockedReason must have exactly code and message fields".to_string(),
        );
    };
    if field_keys(object) != "code,message" {
        return Err(
            "goal change goal.blockedReason must have exactly code and message fields".to_string(),
        );
    }
    let code = object.get("code").and_then(Value::as_str);
    let kebab = regex::Regex::new(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$").expect("valid regex");
    let Some(code) = code else {
        return Err("goal change goal.blockedReason.code must be lower-kebab-case".to_string());
    };
    if !kebab.is_match(code) {
        return Err("goal change goal.blockedReason.code must be lower-kebab-case".to_string());
    }
    let message = object.get("message").and_then(Value::as_str);
    match message {
        Some(message) if !message.trim().is_empty() && message == message.trim() => {
            Ok(GoalBlockReason {
                code: code.to_string(),
                message: message.to_string(),
            })
        }
        _ => Err(
            "goal change goal.blockedReason.message must be non-empty and normalized".to_string(),
        ),
    }
}

fn decode_snapshot(value: &Value) -> Result<GoalSnapshot, String> {
    let Some(object) = value.as_object() else {
        return Err("goal change goal must be a record".to_string());
    };
    let id = object.get("id").and_then(Value::as_str);
    match id {
        Some(id) if !id.is_empty() => {}
        _ => return Err("goal change goal.id must be a non-empty string".to_string()),
    }
    let objective = object.get("objective").and_then(Value::as_str);
    match objective {
        Some(objective) if !objective.trim().is_empty() && objective == objective.trim() => {}
        _ => return Err("goal change goal.objective must be non-empty and normalized".to_string()),
    }
    let phase = object
        .get("phase")
        .and_then(Value::as_str)
        .and_then(crate::domain::phase_from_str)
        .ok_or_else(|| "goal change goal.phase is invalid".to_string())?;
    let expected_keys = if phase == GoalPhase::Blocked {
        "blockedReason,id,maxGoalRounds,objective,phase,revision"
    } else {
        "id,maxGoalRounds,objective,phase,revision"
    };
    if field_keys(object) != expected_keys {
        return Err(format!(
            "goal change goal for phase {} must have exactly {expected_keys} fields",
            phase.as_str()
        ));
    }
    Ok(GoalSnapshot {
        id: goal_id(id.expect("checked")),
        revision: positive_integer(&object["revision"], "goal.revision")?,
        objective: objective.expect("checked").to_string(),
        phase,
        max_goal_rounds: positive_integer(&object["maxGoalRounds"], "goal.maxGoalRounds")?,
        blocked_reason: if phase == GoalPhase::Blocked {
            Some(decode_block_reason(&object["blockedReason"])?)
        } else {
            None
        },
    })
}

fn decode_ref(value: &Value) -> Result<GoalRef, String> {
    let Some(object) = value.as_object() else {
        return Err("goal clear tombstone must have exactly id and revision fields".to_string());
    };
    if field_keys(object) != "id,revision" {
        return Err("goal clear tombstone must have exactly id and revision fields".to_string());
    }
    let id = object.get("id").and_then(Value::as_str);
    match id {
        Some(id) if !id.is_empty() => {}
        _ => return Err("goal clear tombstone id must be a non-empty string".to_string()),
    }
    Ok(GoalRef {
        id: goal_id(id.expect("checked")),
        revision: positive_integer(&object["revision"], "cleared.revision")?,
    })
}

/// Decode a value that declares itself as a goal change. Unrelated values
/// return `None`; malformed goal changes fail replay loudly (TS
/// `decodeGoalChange`).
pub fn decode_goal_change(value: &Value) -> Result<Option<GoalChangeMeta>, String> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if object.get("kind").and_then(Value::as_str) != Some("goal/change") {
        return Ok(None);
    }
    if object.get("version").and_then(Value::as_u64) != Some(GOAL_CHANGE_VERSION as u64) {
        return Err(format!(
            "unsupported goal change version {}",
            object
                .get("version")
                .map(|v| v.to_string())
                .unwrap_or_default()
        ));
    }
    if object.get("operation").and_then(Value::as_str) == Some("clear") {
        let expected = "cleared,clearedAt,kind,operation,version";
        if field_keys(object) != expected {
            return Err(format!(
                "goal clear change must have exactly {expected} fields"
            ));
        }
        return Ok(Some(GoalChangeMeta::Clear(GoalClearChangeMeta {
            cleared: decode_ref(&object["cleared"])?,
            cleared_at: non_negative_integer(&object["clearedAt"], "clearedAt")?,
        })));
    }
    let operation = match object.get("operation").and_then(Value::as_str) {
        Some("create") => GoalOperation::Create,
        Some("edit") => GoalOperation::Edit,
        Some("pause") => GoalOperation::Pause,
        Some("resume") => GoalOperation::Resume,
        Some("complete") => GoalOperation::Complete,
        Some("block") => GoalOperation::Block,
        _ => return Err("goal change operation is invalid".to_string()),
    };
    let expected = "createdAt,goal,kind,operation,roundsStarted,updatedAt,version";
    if field_keys(object) != expected {
        return Err(format!(
            "goal snapshot change must have exactly {expected} fields"
        ));
    }
    let created_at = non_negative_integer(&object["createdAt"], "createdAt")?;
    let updated_at = non_negative_integer(&object["updatedAt"], "updatedAt")?;
    if updated_at < created_at {
        return Err("goal change updatedAt cannot precede createdAt".to_string());
    }
    Ok(Some(GoalChangeMeta::Snapshot(GoalSnapshotChangeMeta {
        operation,
        goal: decode_snapshot(&object["goal"])?,
        rounds_started: non_negative_integer(&object["roundsStarted"], "roundsStarted")?,
        created_at,
        updated_at,
    })))
}

/// Return the revision identity carried by a snapshot or tombstone (TS
/// `goalChangeRef`).
pub fn goal_change_ref(change: &GoalChangeMeta) -> GoalRef {
    match change {
        GoalChangeMeta::Clear(clear) => clear.cleared.clone(),
        GoalChangeMeta::Snapshot(change) => GoalRef {
            id: change.goal.id.clone(),
            revision: change.goal.revision,
        },
    }
}

fn require_same_definition(
    current: &GoalSnapshot,
    next: &GoalSnapshot,
    operation: GoalOperation,
) -> Result<(), String> {
    if next.objective != current.objective || next.max_goal_rounds != current.max_goal_rounds {
        return Err(format!(
            "goal {} cannot change objective or maxGoalRounds",
            operation.as_str()
        ));
    }
    Ok(())
}

fn require_next_revision(
    current: &GoalSnapshot,
    next: &GoalRef,
    operation: GoalOperation,
) -> Result<(), String> {
    if next.id != current.id || next.revision != current.revision + 1 {
        return Err(format!(
            "goal {} must advance the current goal by one revision",
            operation.as_str()
        ));
    }
    Ok(())
}

fn validate_snapshot_transition(
    state: &GoalFoldState,
    change: &GoalSnapshotChangeMeta,
    current: &GoalSnapshot,
) -> Result<(), String> {
    let next = &change.goal;
    require_next_revision(
        current,
        &GoalRef {
            id: next.id.clone(),
            revision: next.revision,
        },
        change.operation,
    )?;
    let updated_at = state
        .updated_at
        .ok_or_else(|| "current goal fold lacks updatedAt".to_string())?;
    if Some(change.created_at) != state.created_at
        || change.updated_at < updated_at
        || change.rounds_started != state.rounds_started
    {
        return Err(format!(
            "goal {} does not preserve the current counters and timestamps",
            change.operation.as_str()
        ));
    }
    match change.operation {
        GoalOperation::Edit => {
            if next.phase != current.phase || next.blocked_reason != current.blocked_reason {
                return Err("goal edit cannot change phase or blocked reason".to_string());
            }
        }
        GoalOperation::Pause => {
            require_same_definition(current, next, change.operation)?;
            if current.phase != GoalPhase::Active || next.phase != GoalPhase::Paused {
                return Err("goal pause has an invalid phase transition".to_string());
            }
        }
        GoalOperation::Resume => {
            require_same_definition(current, next, change.operation)?;
            let resumable = matches!(
                current.phase,
                GoalPhase::Active | GoalPhase::Paused | GoalPhase::Blocked
            );
            if !resumable
                || next.phase != GoalPhase::Active
                || state.rounds_started >= next.max_goal_rounds
            {
                return Err(
                    "goal resume has an invalid phase transition or exhausted round budget"
                        .to_string(),
                );
            }
        }
        GoalOperation::Complete => {
            require_same_definition(current, next, change.operation)?;
            if current.phase == GoalPhase::Complete || next.phase != GoalPhase::Complete {
                return Err("goal complete has an invalid phase transition".to_string());
            }
        }
        GoalOperation::Block => {
            require_same_definition(current, next, change.operation)?;
            if current.phase != GoalPhase::Active || next.phase != GoalPhase::Blocked {
                return Err("goal block has an invalid phase transition".to_string());
            }
        }
        GoalOperation::Create | GoalOperation::Clear => {
            return Err("goal create cannot be validated as a current-goal transition".to_string());
        }
    }
    Ok(())
}

/// Validate and apply one decoded change to a mutable accumulator (TS
/// `applyGoalChange`).
pub fn apply_goal_change(state: &mut GoalFoldState, change: &GoalChangeMeta) -> Result<(), String> {
    let ref_ = goal_change_ref(change);
    match change {
        GoalChangeMeta::Clear(clear) => {
            let current = state
                .goal
                .as_ref()
                .ok_or_else(|| "goal clear requires a current goal".to_string())?;
            require_next_revision(current, &clear.cleared, GoalOperation::Clear)?;
            let updated_at = state
                .updated_at
                .ok_or_else(|| "current goal fold lacks updatedAt".to_string())?;
            if clear.cleared_at < updated_at {
                return Err(
                    "goal clear timestamp cannot precede the current goal update".to_string(),
                );
            }
            state.goal = None;
            state.rounds_started = 0;
            state.created_at = None;
            state.updated_at = None;
            state.last_ref = Some(ref_);
            return Ok(());
        }
        GoalChangeMeta::Snapshot(change) => {
            if change.operation == GoalOperation::Create {
                let existing_phase_ok = state
                    .goal
                    .as_ref()
                    .map(|current| current.phase == GoalPhase::Complete)
                    .unwrap_or(true);
                if change.goal.revision != 1
                    || change.goal.phase != GoalPhase::Active
                    || change.rounds_started != 0
                    || !existing_phase_ok
                    || state.seen_goal_ids.contains(change.goal.id.as_str())
                {
                    return Err(
                        "goal create requires a fresh active revision-one goal with zero rounds"
                            .to_string(),
                    );
                }
                state
                    .seen_goal_ids
                    .insert(change.goal.id.as_str().to_string());
            } else {
                let current = state.goal.as_ref().ok_or_else(|| {
                    format!("goal {} requires a current goal", change.operation.as_str())
                })?;
                validate_snapshot_transition(state, change, current)?;
            }
            state.goal = Some(change.goal.clone());
            state.rounds_started = change.rounds_started;
            state.created_at = Some(change.created_at);
            state.updated_at = Some(change.updated_at);
            state.last_ref = Some(ref_);
            return Ok(());
        }
    }
}

/// Narrow one `user/message` source to a valid goal attribution.
fn goal_source(source: Option<&Value>) -> Result<Option<crate::domain::GoalMessageSource>, String> {
    let Some(source) = source else {
        return Ok(None);
    };
    let Some(object) = source.as_object() else {
        return Ok(None);
    };
    if object.get("kind").and_then(Value::as_str) != Some("goal") {
        return Ok(None);
    }
    let goal_id_value = object.get("goalId").and_then(Value::as_str);
    let revision = object.get("revision").and_then(Value::as_u64);
    let round = object.get("round").and_then(Value::as_u64);
    match (goal_id_value, revision, round) {
        (Some(goal_id), Some(revision), Some(round))
            if !goal_id.is_empty() && revision >= 1 && round >= 1 =>
        {
            Ok(Some(crate::domain::GoalMessageSource {
                goal_id: crate::types::goal_id(goal_id),
                revision,
                round,
            }))
        }
        _ => Err("goal message source is invalid".to_string()),
    }
}

/// Apply one session event to the strict durable goal fold (TS
/// `applyGoalEvent`).
pub fn apply_goal_event(state: &mut GoalFoldState, event: &SessionEvent) -> Result<(), String> {
    if event.type_ == "goal/change" {
        let change = decode_goal_change(&event.data)?.ok_or_else(|| {
            format!(
                "goal change at session event {} has an invalid kind",
                event.seq
            )
        })?;
        return apply_goal_change(state, &change);
    }
    if event.type_ == "user/message" {
        let source = goal_source(event.data.get("source"))?;
        let Some(source) = source else {
            return Ok(());
        };
        let current = state.goal.as_ref();
        let Some(current) = current else {
            return Err(format!(
                "goal round at session event {} is not the next admitted round of the active goal",
                event.seq
            ));
        };
        if current.phase != GoalPhase::Active
            || source.goal_id != current.id
            || source.revision != current.revision
            || source.round != state.rounds_started + 1
            || source.round > current.max_goal_rounds
        {
            return Err(format!(
                "goal round at session event {} is not the next admitted round of the active goal",
                event.seq
            ));
        }
        state.rounds_started = source.round;
    }
    Ok(())
}

/// Fold current goal state from a contiguous session event log (TS
/// `foldGoal`).
pub fn fold_goal(events: &[SessionEvent]) -> Result<crate::domain::FoldedGoal, String> {
    let mut state = empty_goal_fold_state();
    for event in events {
        apply_goal_event(&mut state, event)?;
    }
    Ok(crate::domain::FoldedGoal {
        goal: state.goal,
        rounds_started: state.rounds_started,
        created_at: state.created_at,
        updated_at: state.updated_at,
        last_ref: state.last_ref,
    })
}
