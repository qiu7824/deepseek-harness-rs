//! Pure types of the goal domain: the one home of the `goal`
//! projection-key declaration plus the durable payload vocabulary it
//! carries. Rust port of `packages/goal/goal/src/types.ts`.

use dsh_brand::Branded;

/// Marker for the goal-id brand (TS `GoalId`).
pub enum GoalIdTag {}

/// Identifies one goal across its durable revisions.
pub type GoalId = Branded<GoalIdTag>;

/// Brand a string as a [`GoalId`] (TS `GoalId(id)`).
pub fn goal_id(id: impl Into<String>) -> GoalId {
    Branded::new(id)
}

/// Compare-and-set identity for one exact goal revision (TS `GoalRef`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRef {
    /// Stable goal identity.
    pub id: GoalId,
    /// Positive revision; every durable mutation increments it.
    pub revision: u64,
}

/// Input whose omitted round cap is resolved by the service configuration
/// (TS `CreateGoalRequest`).
#[derive(Debug, Clone)]
pub struct CreateGoalRequest {
    pub objective: String,
    pub max_goal_rounds: Option<u64>,
}

/// Wire-safe acknowledgement of one created goal (TS `CreateGoalResult`).
#[derive(Debug, Clone)]
pub struct CreateGoalResult {
    pub ref_: GoalRef,
}

/// Fields changed by an edit; at least one must be present (TS
/// `EditGoalRequest`).
#[derive(Debug, Clone, Default)]
pub struct EditGoalRequest {
    pub objective: Option<String>,
    pub max_goal_rounds: Option<u64>,
}

/// Durable continuation phase. Activation is process-local and separate (TS
/// `GoalPhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalPhase {
    Active,
    Paused,
    Blocked,
    Complete,
}

impl GoalPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalPhase::Active => "active",
            GoalPhase::Paused => "paused",
            GoalPhase::Blocked => "blocked",
            GoalPhase::Complete => "complete",
        }
    }
}

/// Machine-routable and human-readable explanation for a blocked goal (TS
/// `GoalBlockReason`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalBlockReason {
    /// Stable lower-kebab-case classification chosen by the blocking policy.
    pub code: String,
    /// Non-empty explanation shown to humans and models.
    pub message: String,
}

/// Full durable state written by every non-clear goal mutation (TS
/// `GoalSnapshot`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalSnapshot {
    pub id: GoalId,
    pub revision: u64,
    /// Human-requested completion objective.
    pub objective: String,
    /// Durable lifecycle phase.
    pub phase: GoalPhase,
    /// Present exactly while `phase` is `blocked`.
    pub blocked_reason: Option<GoalBlockReason>,
    /// Total admitted goal-round cap.
    pub max_goal_rounds: u64,
}

/// Whether this live process may automatically continue an active goal (TS
/// `GoalActivation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalActivation {
    Armed,
    Disarmed,
}

/// Current goal projection, including values derived from the session log
/// (TS `GoalView`).
#[derive(Debug, Clone)]
pub struct GoalView {
    pub id: GoalId,
    pub revision: u64,
    pub objective: String,
    pub phase: GoalPhase,
    pub blocked_reason: Option<GoalBlockReason>,
    pub max_goal_rounds: u64,
    /// Highest admitted round number for this goal.
    pub rounds_started: u64,
    /// Epoch milliseconds of the create mutation.
    pub created_at: u64,
    /// Epoch milliseconds of the latest mutation.
    pub updated_at: u64,
    /// Process-local continuation eligibility; never persisted.
    pub activation: GoalActivation,
}

/// The `goal` projection value: the current durable goal with its replay
/// counters (TS `GoalProjection`). Activation is process-local and
/// deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalProjection {
    /// Current durable goal snapshot.
    pub goal: GoalSnapshot,
    /// Highest admitted round number for this goal.
    pub rounds_started: u64,
    /// Epoch milliseconds of the create mutation.
    pub created_at: u64,
    /// Epoch milliseconds of the latest mutation.
    pub updated_at: u64,
}
