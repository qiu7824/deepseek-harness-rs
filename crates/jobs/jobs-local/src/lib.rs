//! Process-local background-job registry (`ctx.jobs`). Rust port of
//! `packages/jobs/jobs-local`.

pub mod index;
pub mod invariant;

pub use index::{
    Config, DEFAULT_MAX_CONCURRENT_JOBS_PER_OWNER, LocalJobRegistry, TASK_WAIT_TIMEOUT,
};
