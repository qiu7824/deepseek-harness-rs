//! Shared agent-loop scheduler defaults. Rust port of
//! `packages/core/agent-loop/src/constants.ts`.

/// Default maximum in-flight parallel-safe calls per agent step.
pub const DEFAULT_MAX_PARALLEL_TOOL_CALLS: u64 = 10;
