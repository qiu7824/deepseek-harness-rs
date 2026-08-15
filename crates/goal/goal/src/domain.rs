//! Host-side vocabulary of the goal domain: durable change payloads, message
//! attribution, replay folds, and the scoped `goal/changed` event. Rust port
//! of `packages/goal/goal/src/domain.ts`.

use dsh_agent::Agent;

use crate::types::{GoalPhase, GoalRef, GoalSnapshot, GoalView};

/// Goal state-changing verbs recorded in the durable source change (TS
/// `GoalOperation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalOperation {
    Create,
    Edit,
    Pause,
    Resume,
    Complete,
    Block,
    Clear,
}

impl GoalOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalOperation::Create => "create",
            GoalOperation::Edit => "edit",
            GoalOperation::Pause => "pause",
            GoalOperation::Resume => "resume",
            GoalOperation::Complete => "complete",
            GoalOperation::Block => "block",
            GoalOperation::Clear => "clear",
        }
    }
}

/// Full-snapshot goal mutation committed by a durable `goal/change` event
/// (TS `GoalSnapshotChangeMeta`).
#[derive(Debug, Clone)]
pub struct GoalSnapshotChangeMeta {
    pub operation: GoalOperation,
    pub goal: GoalSnapshot,
    pub rounds_started: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Tombstone retained when the current goal is cleared (TS
/// `GoalClearChangeMeta`).
#[derive(Debug, Clone)]
pub struct GoalClearChangeMeta {
    pub cleared: GoalRef,
    pub cleared_at: u64,
}

/// Durable change union carried by the goal domain's own session event (TS
/// `GoalChangeMeta`).
#[derive(Debug, Clone)]
pub enum GoalChangeMeta {
    Snapshot(GoalSnapshotChangeMeta),
    Clear(GoalClearChangeMeta),
}

impl GoalChangeMeta {
    pub fn operation(&self) -> GoalOperation {
        match self {
            GoalChangeMeta::Snapshot(change) => change.operation,
            GoalChangeMeta::Clear(_) => GoalOperation::Clear,
        }
    }
}

/// Message attribution for admitted continuation rounds (TS
/// `GoalMessageSource`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalMessageSource {
    pub goal_id: crate::types::GoalId,
    pub revision: u64,
    /// Positive admitted continuation round.
    pub round: u64,
}

/// Pure replay fold of durable goal facts (TS `FoldedGoal`).
#[derive(Debug, Clone, Default)]
pub struct FoldedGoal {
    /// Current goal, absent after a clear or before the first create.
    pub goal: Option<GoalSnapshot>,
    /// Highest admitted round for the current goal.
    pub rounds_started: u64,
    /// Current goal creation time, absent without a current goal.
    pub created_at: Option<u64>,
    /// Current goal mutation time, absent without a current goal.
    pub updated_at: Option<u64>,
    /// Latest mutation ref, including a clear tombstone.
    pub last_ref: Option<GoalRef>,
}

/// Live notification after one durable goal mutation commits (TS
/// `GoalChanged`).
#[derive(Debug, Clone)]
pub struct GoalChanged {
    pub operation: GoalOperation,
    pub ref_: GoalRef,
    /// Absent for a clear tombstone.
    pub goal: Option<GoalView>,
}

/// Stable error codes for rejected goal reads and mutations (TS
/// `GoalErrorCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalErrorCode {
    AgentNotLive,
    NotFound,
    AlreadyExists,
    StaleRevision,
    InvalidObjective,
    InvalidMaxRounds,
    InvalidBlockReason,
    InvalidEdit,
    InvalidTransition,
}

impl GoalErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalErrorCode::AgentNotLive => "GOAL_AGENT_NOT_LIVE",
            GoalErrorCode::NotFound => "GOAL_NOT_FOUND",
            GoalErrorCode::AlreadyExists => "GOAL_ALREADY_EXISTS",
            GoalErrorCode::StaleRevision => "GOAL_STALE_REVISION",
            GoalErrorCode::InvalidObjective => "GOAL_INVALID_OBJECTIVE",
            GoalErrorCode::InvalidMaxRounds => "GOAL_INVALID_MAX_ROUNDS",
            GoalErrorCode::InvalidBlockReason => "GOAL_INVALID_BLOCK_REASON",
            GoalErrorCode::InvalidEdit => "GOAL_INVALID_EDIT",
            GoalErrorCode::InvalidTransition => "GOAL_INVALID_TRANSITION",
        }
    }
}

/// Error returned by the goal domain boundary (TS `GoalError`).
#[derive(Debug, Clone)]
pub struct GoalError {
    pub message: String,
    pub code: GoalErrorCode,
}

impl GoalError {
    pub fn new(message: impl Into<String>, code: GoalErrorCode) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }
}

impl std::fmt::Display for GoalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GoalError {}

/// The `goal/changed` event payload (TS scoped emit). Carries a live agent
/// handle, so no `Debug`.
#[derive(Clone)]
pub struct GoalChangedPayload {
    pub agent: std::sync::Arc<dyn Agent>,
    pub change: GoalChanged,
}

/// Canonical phase order helpers shared by folds and validation.
pub fn phase_from_str(value: &str) -> Option<GoalPhase> {
    match value {
        "active" => Some(GoalPhase::Active),
        "paused" => Some(GoalPhase::Paused),
        "blocked" => Some(GoalPhase::Blocked),
        "complete" => Some(GoalPhase::Complete),
        _ => None,
    }
}
