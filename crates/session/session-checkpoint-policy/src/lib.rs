//! Semantic durability checkpoints for model requests, top-level tool
//! dispatch, and completed agent steps. Rust port of
//! `packages/session/session-checkpoint-policy`.

pub mod index;
pub mod invariant;

pub use index::{
    INJECT, NAME, aborted_before_dispatch_result, after_checkpoint, apply,
    needs_tool_checkpoint,
};
