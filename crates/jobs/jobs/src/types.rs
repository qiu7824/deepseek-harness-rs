//! Types shared by job producers, the registry, and controllers. The
//! service implementation lives in `./index.rs`. Rust port of
//! `packages/jobs/jobs/src/types.ts` + `brand.ts`.
//!
//! # Deviations
//!
//! - `JobKind` is a plain `String`: the TS declaration-merging union is an
//!   opaque id namespace at runtime (the registry never inspects members).
//! - `AbortSignal` collapses into the repo-wide cancellation predicate
//!   ([`JobAbort`]).
//! - A throwing `JobStart.run` collapses into a panic (the repo-wide
//!   throw-equivalent); `JobHooks.done` never rejects, so its future
//!   resolves the outcome directly.

use std::sync::Arc;

use dsh_agent::Agent;
use dsh_brand::Branded;
use dsh_session::SessionId;
use futures::future::BoxFuture;

/// Marker for the job-id brand (TS `JobId`).
pub enum JobIdTag {}

/// Identifies a background job. The registry generates `<kind>-N`;
/// predictable ids rely on owner authorization rather than secrecy.
pub type JobId = Branded<JobIdTag>;

/// Brand a string as a [`JobId`] (no validation; TS `JobId(id)`).
pub fn job_id(id: impl Into<String>) -> JobId {
    Branded::new(id)
}

/// The abort/cancellation predicate (the TS `AbortSignal` collapse).
pub type JobAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// Task lifecycle: `running`, optionally `stopping`, then exactly one
/// terminal status (TS `JobStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Stopping,
    Completed,
    Killed,
    Failed,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Stopping => "stopping",
            JobStatus::Completed => "completed",
            JobStatus::Killed => "killed",
            JobStatus::Failed => "failed",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Killed | JobStatus::Failed)
    }
}

/// How a job ended (the TS `JobOutcome.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcomeStatus {
    /// Finished.
    Completed,
    /// Cancelled.
    Killed,
    /// Broke.
    Failed,
}

/// Terminal result supplied by a producer through [`JobHooks::done`] (TS
/// `JobOutcome`).
#[derive(Debug, Clone)]
pub struct JobOutcome {
    /// How the job ended.
    pub status: JobOutcomeStatus,
    /// Kind-specific detail rendered into status lines ('exit code: 3',
    /// 'max-tokens').
    pub detail: Option<String>,
    /// Final output for jobs without `read_output`; stream jobs leave it
    /// unset.
    pub output: Option<String>,
}

/// Hooks through which the runtime controls and observes producer work (TS
/// `JobHooks`).
pub trait JobHooks: Send + Sync + 'static {
    /// Request termination. Must be synchronous, idempotent, and eventually
    /// settle [`JobHooks::done`]; throws propagate (the Rust panic
    /// equivalent).
    fn cancel(&self, reason: Option<String>);
    /// Resolves after the producer releases its resources, not merely when
    /// work finishes. Must not reject.
    fn done(&self) -> BoxFuture<'static, JobOutcome>;
    /// Consume output produced since the previous call. The producer formats
    /// truncation and spill notices. Absence marks a final-output-only job;
    /// each job has one consuming cursor.
    fn read_output(&self) -> Option<String>;
}

/// Producer declaration passed to the registry's `start` (TS `JobStart`).
pub struct JobStart {
    /// Producer kind — also the id prefix (`bash`, `subagent`, …).
    pub kind: String,
    /// One-line model-facing label (the command; the delegation
    /// description).
    pub label: String,
    /// Optional UTF-8 byte cap for each complete model-facing completion
    /// notice or output read, including controller status metadata.
    pub output_limit_bytes: Option<u64>,
    /// Owning live agent. Access is fenced by its session id, and agent
    /// disposal cancels and awaits the job. Omitting the owner creates an
    /// unowned job, open to any caller until service disposal.
    pub owner: Option<Arc<dyn Agent>>,
    /// Start the work after preflight and synchronously return its hooks.
    /// Called once; a throw leaves nothing registered, and the producer must
    /// clean up any partially started resources.
    pub run: Arc<dyn Fn() -> Arc<dyn JobHooks> + Send + Sync>,
}

/// A read-only projection of one job, safe to hand to listeners and tools —
/// a fresh object per call, never live registry state (TS `JobSnapshot`).
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    /// The registry-issued id (`<kind>-N`).
    pub id: JobId,
    /// The producer kind the job was registered with.
    pub kind: String,
    /// The producer-supplied one-line label.
    pub label: String,
    /// Producer-owned cap for complete model-facing notices and output reads.
    pub output_limit_bytes: Option<u64>,
    /// Owner session id used for authorization and correlation; absent for
    /// unowned jobs.
    pub owner_session: Option<SessionId>,
    /// Current lifecycle state.
    pub status: JobStatus,
    /// Kind-specific status detail, present once the producer supplied one
    /// (usually terminal).
    pub detail: Option<String>,
    /// Epoch ms when the job was registered.
    pub started_at: u64,
    /// Epoch ms when the job settled; absent while `running`/`stopping`.
    pub finished_at: Option<u64>,
    /// True when a kill, read, wait, or teardown cancel has reported or
    /// committed to report the terminal state.
    pub reported: bool,
}

/// Output and post-read state returned by the registry's `read` (TS
/// `JobRead`).
#[derive(Debug, Clone)]
pub struct JobRead {
    /// Stream kinds: the consuming delta since the previous read.
    /// Final-output kinds: empty while live, the terminal output (or empty)
    /// once settled — idempotent, never consumed.
    pub text: String,
    /// The job's state at read time.
    pub snapshot: JobSnapshot,
}

/// Completion callback with the exact owner supplied at start, or `None` for
/// an unowned job. Returned work is observed but not awaited (TS
/// `JobDoneListener`; the TS promise return collapses to a plain callback).
pub type JobDoneListener = Arc<dyn Fn(JobSnapshot, Option<Arc<dyn Agent>>) + Send + Sync>;

/// Observation callback for a change to what one owner's `list` would return
/// (TS `JobsChangedListener`). A `None` owner means an unowned job changed,
/// so every caller's visible set changed with it.
pub type JobsChangedListener = Arc<dyn Fn(Option<Arc<dyn Agent>>) + Send + Sync>;
