//! Model-facing `job_output`, `job_list`, and `job_kill` tools over
//! `ctx.jobs`, plus completion-notice delivery to the owning agent. Rust
//! port of `packages/jobs/tool-jobs`.
//!
//! # Deviations
//!
//! - The TS synchronous `apply` is an async `apply` here (the claimed
//!   listener registration goes through the async `ctx.on`); callers await
//!   it.
//! - `maxConsecutiveWakes` is a `u64`: JS `Infinity` is not representable
//!   in the JSON config space, and fractional budgets are rejected when the
//!   config JSON is decoded.
//! - The `spentWakes` budget is keyed by the exact owner `Arc` pointer (the
//!   TS `WeakMap<Agent, number>` identity collapse).

pub mod index;
pub mod invariant;

pub use index::{
    CompletionDelivery, Config, INJECT, NAME, ToolJobsPlugin, ToolJobsService, apply, status_line,
};
pub use invariant::{PACKAGE_NAME as INVARIANT_PACKAGE_NAME, ToolJobsInvariantPlugin};
