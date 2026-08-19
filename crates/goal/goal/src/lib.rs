//! Same-session goal domain (`ctx.goals`). Rust port of
//! `packages/goal/goal`.

pub mod domain;
pub mod fold;
pub mod index;
pub mod invariant;
pub mod runtime;
pub mod types;

pub use domain::{
    FoldedGoal, GoalChangeMeta, GoalChanged, GoalChangedPayload, GoalClearChangeMeta, GoalError,
    GoalErrorCode, GoalMessageSource, GoalOperation, GoalSnapshotChangeMeta, phase_from_str,
};
pub use fold::{
    GoalFoldState, apply_goal_change, apply_goal_event, decode_goal_change, empty_goal_fold_state,
    fold_goal, goal_change_ref,
};
pub use index::{
    Config, DEFAULT_MAX_GOAL_ROUNDS, GoalService, ResolvedConfig, apply_goal_projection,
};
pub use runtime::GOAL_CHANGE_VERSION;
pub use types::{
    CreateGoalRequest, CreateGoalResult, EditGoalRequest, GoalActivation, GoalBlockReason, GoalId,
    GoalIdTag, GoalPhase, GoalProjection, GoalRef, GoalSnapshot, GoalView, goal_id,
};
