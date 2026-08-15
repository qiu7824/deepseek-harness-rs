//! Background-job capability seam (`ctx.jobs`). Rust port of
//! `packages/jobs/jobs`.

pub mod index;
pub mod invariant;
pub mod types;

pub use index::{JobRegistry, KillOutcome};
pub use types::{
    JobAbort, JobDoneListener, JobHooks, JobId, JobIdTag, JobOutcome, JobOutcomeStatus, JobRead,
    JobSnapshot, JobStart, JobStatus, JobsChangedListener, job_id,
};
