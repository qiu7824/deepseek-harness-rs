//! Same-session goal domain (`ctx.goals`). Rust port of
//! `packages/goal/goal`.

pub mod domain;
pub mod fold;
pub mod index;
pub mod invariant;
pub mod requirements;
pub mod projection;
pub mod runtime;
pub mod types;

#[cfg(test)]
mod requirements_tests;

pub use domain::{
    FoldedGoal, GoalChangeMeta, GoalChanged, GoalChangedPayload, GoalClearChangeMeta, GoalError,
    GoalErrorCode, GoalMessageSource, GoalOperation, GoalSnapshotChangeMeta, phase_from_str,
};
pub use fold::{
    GoalFoldState, apply_goal_change, apply_goal_event, decode_goal_change, empty_goal_fold_state,
    fold_goal, goal_change_ref,
};
pub use index::{
    Config, DEFAULT_MAX_GOAL_ROUNDS, GoalRequirementsLease, GoalService, ResolvedConfig,
    apply_goal_projection,
};
pub use runtime::GOAL_CHANGE_VERSION;
pub use projection::{goal_projection_definition, register_goal_projection};
pub use types::{
    CreateGoalRequest, CreateGoalResult, EditGoalRequest, GoalActivation, GoalBlockReason, GoalId,
    GoalIdTag, GoalPhase, GoalProjection, GoalRef, GoalSnapshot, GoalView, goal_id,
};

pub use requirements::{
    GOAL_COMPLETION_GUARD_SERVICE, GOAL_USER_CONTROL_SERVICE, GoalCompletionCommitGuard,
    GoalCompletionError, GoalCompletionGuard, GoalCompletionPermit, GoalRequirementsIdentity,
    GoalUserControl, apply_goal_requirements_projection,
};
