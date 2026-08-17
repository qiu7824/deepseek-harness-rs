//! Subagent seam: run/result/capability types, the durable child
//! descriptor, delegation depth, the provider trait, the named-provider
//! registry service with one-shot lifecycle events, session projections,
//! and child/descendant listing. Rust port of
//! `packages/subagent/subagent`.
//!
//! # Deviations
//!
//! - The continuation manager and activation setup registry are not ported
//!   yet (continuable operations reject with `CONTINUATION_UNAVAILABLE`).
//! - `SubagentProvider.prepareContinuable` is a defaulted trait method
//!   rejecting with `SUBAGENT_NOT_CONTINUABLE` (the TS optional-method
//!   capability).
//! - The `AbortSignal` seam is the shared abort predicate.

pub mod assistant_output;
pub mod child_agent;
pub mod continuation;
pub mod depth;
pub mod descriptor;
#[path = "descriptor-seed.rs"]
pub mod descriptor_seed;
pub mod error;
pub mod in_process;
pub mod index;
pub mod lifecycle;
pub mod list_children;
pub mod projection;
pub mod run_settlement;
pub mod types;

pub use crate::assistant_output::{AssistantOutputFold, final_assistant_output};
pub use crate::child_agent::{
    ChildComposition, DelegatedPolicyOverrides, SUBAGENT_DELEGATION_CONTEXT, SubagentDepthError,
    append_delegated_policy_overrides, apply_child_composition, capture_delegated_policy_overrides,
    child_session_meta, resolve_child_agent_options, resolve_child_depth,
};
pub use crate::continuation::{
    ContinuableStart, ContinuableStartSpec, ContinuationHost, SubagentContinuationManager,
    SubagentFollowupOptions, SubagentInterruptAuthority, SubagentReportDelivery,
    SubagentReportOptions,
};
pub use crate::depth::{assert_subagent_max_depth, delegation_depth_of};
pub use crate::descriptor::{
    SUBAGENT_DESCRIPTOR_VERSION, SubagentDescriptorData, fold_subagent_descriptor,
    snapshot_subagent_descriptor,
};
pub use crate::descriptor_seed::seed_descriptor_turn;
pub use crate::error::SubagentError;
pub use crate::in_process::{InProcessRunOptions, start_in_process_run, to_stop_reason};
pub use crate::index::{SubagentPlugin, SubagentRuntime};
pub use crate::list_children::{SubagentDescendantListEntry, SubagentListEntry};
pub use crate::projection::{
    SubagentIdentityProjection, SubagentTimingProjection, same_lifecycle,
    subagent_identity_projection_definition, subagent_timing_projection_definition,
};
pub use crate::run_settlement::settle_run;
pub use crate::types::*;
