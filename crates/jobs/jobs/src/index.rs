//! The background-job Service Definition (`ctx.jobs`). It owns the contract
//! for job ids, session-scoped access, lifecycle state, completion listeners,
//! and owner cleanup while producers retain their execution resources. The
//! process-local registry lives in `dsh-jobs-local`. Rust port of
//! `packages/jobs/jobs/src/index.ts`.
//!
//! # Deviations
//!
//! - The TS abstract-class load fence (`new.target` check) is a compile-time
//!   fact in Rust: the trait has no runtime instance, so a composition row
//!   naming this package cannot register an empty `ctx.jobs`.
//! - Synchronous TS throws (`get`/`read`/`kill` contract misuse) collapse
//!   into `Result<_, String>`.

use std::sync::Arc;

use cordis::{Context, Disposer, Service};
use dsh_agent::Agent;
use dsh_session::SessionId;
use futures::future::BoxFuture;

pub use crate::types::{
    JobAbort, JobDoneListener, JobHooks, JobId, JobOutcome, JobOutcomeStatus, JobRead, JobSnapshot,
    JobStart, JobStatus, JobViewRead, JobsChangedListener, job_id,
};

/// The outcome of a kill request (TS `'requested' | 'already-finished'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    /// Live work; the cancellation was requested.
    Requested,
    /// The job had already finished.
    AlreadyFinished,
}

/// Abstract background job registry. Subclass, implement the abstract
/// methods, and register the subclass as `ctx.jobs` (one implementation per
/// context; registering a second panics, which is cordis' standard
/// duplicate-service behavior).
///
/// Implementations must honor these semantics:
/// - Registrations outlive producer and controller fibers. Owner and service
///   disposal cancel live work and await compliant producers; a throwing
///   teardown cancel force-fails only the record. Teardown cancellation also
///   marks the record reported.
/// - Owned-job access is fenced by the owner's session id. Ids are
///   predictable, so authorization — not secrecy — is the boundary.
/// - Settlement is first-wins: one terminal record, released waiters, and one
///   round of contained listener notification. Completion is announced last,
///   after the record is committed.
/// - `start` refuses work while no attached job controller serves the spec's
///   owner.
pub trait JobRegistry: Send + Sync + 'static {
    /// Preflight access, validation, owner cleanup, and
    /// implementation-owned admission before starting and atomically
    /// registering work. Any preflight rejection leaves no job id or
    /// execution resource. A throwing starter leaves nothing registered;
    /// after it returns, registration cannot fail.
    fn start(&self, spec: JobStart) -> Result<JobId, String>;

    /// List caller-owned and unowned jobs in registration order without
    /// exposing another session's labels.
    fn list(&self, caller: Option<&Arc<dyn Agent>>) -> Vec<JobSnapshot>;

    /// List caller-owned, archived, and unowned jobs using a durable session
    /// id, without requiring an active Agent runtime.
    fn list_for_session(&self, caller: &SessionId) -> Vec<JobSnapshot>;

    /// Whether this exact owner still has running or stopping work. Hosts
    /// use this to keep the owning Agent alive while background execution is
    /// expected to continue.
    fn has_owner_activity(&self, owner: &Arc<dyn Agent>) -> bool;

    /// Return a non-consuming snapshot without changing its read cursor or
    /// notice state.
    fn get(&self, id: &JobId, caller: Option<&Arc<dyn Agent>>) -> Result<JobSnapshot, String>;

    /// Read the next stream delta, or the idempotent final output after
    /// settlement. A terminal read marks the job reported.
    fn read(&self, id: &JobId, caller: Option<&Arc<dyn Agent>>) -> Result<JobRead, String>;

    /// Read a bounded human-view transcript without advancing the model's
    /// consuming [`Self::read`] cursor. A caller should feed the returned
    /// cursor into its next view read for incremental output.
    fn read_view(
        &self,
        id: &JobId,
        caller: Option<&Arc<dyn Agent>>,
        cursor: Option<u64>,
    ) -> Result<JobViewRead, String>;

    /// Read a human-view transcript using only its durable session owner.
    /// This remains available after an idle Agent runtime has retired, while
    /// preserving the same session-id authorization fence as live reads.
    fn read_view_for_session(
        &self,
        id: &JobId,
        caller: &SessionId,
        cursor: Option<u64>,
    ) -> Result<JobViewRead, String>;

    /// Request cancellation, then mark the job stopping and reported. A
    /// producer throw propagates without changing job state.
    fn kill(
        &self,
        id: &JobId,
        caller: Option<&Arc<dyn Agent>>,
        reason: Option<String>,
    ) -> Result<KillOutcome, String>;

    /// Wait for settlement or timeout without cancelling the job. Caller
    /// abort rejects only while the job is live; after settlement the
    /// terminal snapshot wins.
    fn wait(
        &self,
        id: &JobId,
        timeout_ms: u64,
        caller: Option<&Arc<dyn Agent>>,
        signal: Option<JobAbort>,
    ) -> BoxFuture<'static, Result<JobSnapshot, String>>;

    /// Register an effect-scoped completion listener. It receives the
    /// settlements of the owners its registering context's scope covers;
    /// each listener is contained; returned work is observed but not
    /// awaited. No listener runs after service disposal.
    fn on_job_done(&self, caller: &Context, listener: JobDoneListener) -> Disposer;

    /// Register an effect-scoped observer of visible-set changes. It fires
    /// after every commit that changes what `list` returns for that owner.
    /// This is not a superset of [`JobRegistry::on_job_done`]: it carries no
    /// delivery meaning and marks nothing reported.
    fn on_jobs_changed(&self, caller: &Context, listener: JobsChangedListener) -> Disposer;

    /// Attach an effect-scoped controller that can read and stop jobs. It
    /// serves the owners its registering context's scope covers, and `start`
    /// refuses an owner no attached controller serves.
    fn attach_controller(&self, caller: &Context, name: &str) -> Disposer;
}

impl Service for dyn JobRegistry {
    fn service_name(&self) -> &'static str {
        "jobs"
    }
}
