//! Process-local provider for the background-job capability seam
//! (`ctx.jobs`). It keeps live records plus a bounded, session-fenced archive
//! of completed human-view transcripts and hands out fresh snapshots, never
//! live state. Rust port of
//! `packages/jobs/jobs-local/src/index.ts`.
//!
//! # Deviations
//!
//! - `done` never rejects at the seam; a panicking producer `done` collapses
//!   into a contained `failed` settlement (the TS rejection branch's Rust
//!   equivalent).
//! - The `waiters` bookkeeping is an atomic counter; settlement releases
//!   every registered waiter through one `Notify` (the TS
//!   `waitResolvers` set + `settled` promise collapse).
//! - The abort predicate is polled every 15 ms.
//! - `start` and the `wait` synchronous prefix require a live tokio runtime
//!   (the `done` settlement driver is a spawned task).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cordis::{Context, Disposer, EventOptions, Listener, Service, make_disposer};
use dsh_agent::{Agent, AgentRegistry};
use dsh_jobs::{
    JobDoneListener, JobHooks, JobId, JobOutcome, JobOutcomeStatus, JobRead, JobRegistry,
    JobSnapshot, JobStart, JobStatus, JobViewRead, JobsChangedListener, KillOutcome, job_id,
};
use dsh_scope::{AnonymousEntries, ScopeLayer, ScopedLayers, scope_of};
use dsh_session::SessionId;
use dsh_timeout::{DeadlineSignal, deadline, timeout_of};
use futures::FutureExt;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use tokio::sync::Notify;

/// Timeout code that distinguishes a bounded wait from caller cancellation
/// (TS `TASK_WAIT_TIMEOUT`).
pub const TASK_WAIT_TIMEOUT: &str = "TASK_WAIT_TIMEOUT";

/// Default maximum number of active jobs in one exact-owner bucket.
pub const DEFAULT_MAX_CONCURRENT_JOBS_PER_OWNER: u64 = 10;
const MAX_VIEW_TRANSCRIPT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARCHIVED_JOBS_PER_SESSION: usize = 32;
const MAX_ARCHIVED_TRANSCRIPT_BYTES: usize = 4 * 1024 * 1024;

/// Configuration for the process-local job registry.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Maximum `running` plus `stopping` jobs per exact owner or in the
    /// shared unowned bucket; omission defaults to 10.
    pub max_concurrent_jobs_per_owner: Option<u64>,
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// The registry's mutable per-job record (never handed out).
struct TrackedJob {
    id: JobId,
    kind: String,
    label: String,
    output_limit_bytes: Option<u64>,
    owner: Option<Arc<dyn Agent>>,
    hooks: Arc<dyn JobHooks>,
    status: Mutex<JobStatus>,
    detail: Mutex<Option<String>>,
    output: Mutex<Option<String>>,
    started_at: u64,
    finished_at: Mutex<Option<u64>>,
    reported: AtomicBool,
    /// Monotonic registration ordinal (the TS registration-order contract
    /// for `list`; `startedAt` alone is ms-grained and unstable).
    ordinal: u64,
    /// Settled once the terminal snapshot is recorded and listeners notified.
    settled: Arc<Notify>,
    /// The settled fact itself (a `Notify` stores no permit; late waiters
    /// must observe the flag—the TS settled promise's value semantics).
    settled_flag: AtomicBool,
    /// Live waits; settlement with a waiter marks the job reported.
    waiters: AtomicU64,
    transcript: Mutex<JobTranscript>,
    admission_key: usize,
    admission_released: AtomicBool,
}

struct JobAdmission<'a> {
    registry: &'a LocalJobRegistry,
    key: usize,
    armed: bool,
}

impl JobAdmission<'_> {
    fn commit(mut self) {
        self.armed = false;
    }
}

impl Drop for JobAdmission<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.registry.release_admission(self.key);
        }
    }
}

#[derive(Clone, Default)]
struct JobTranscript {
    text: String,
    base_cursor: u64,
    next_cursor: u64,
    model_cursor: u64,
    stream: Option<bool>,
    final_captured: bool,
}

#[derive(Clone)]
struct ArchivedJob {
    snapshot: JobSnapshot,
    ordinal: u64,
    transcript: JobTranscript,
}

#[derive(Default)]
struct CompletedArchive {
    by_session: HashMap<SessionId, VecDeque<ArchivedJob>>,
    transcript_bytes: usize,
}

impl CompletedArchive {
    fn insert(&mut self, session: SessionId, job: ArchivedJob) {
        let entries = self.by_session.entry(session).or_default();
        if let Some(index) = entries
            .iter()
            .position(|entry| entry.snapshot.id == job.snapshot.id)
            && let Some(replaced) = entries.remove(index)
        {
            self.transcript_bytes = self
                .transcript_bytes
                .saturating_sub(replaced.transcript.text.len());
        }
        self.transcript_bytes = self
            .transcript_bytes
            .saturating_add(job.transcript.text.len());
        entries.push_back(job);
        while entries.len() > MAX_ARCHIVED_JOBS_PER_SESSION {
            if let Some(removed) = entries.pop_front() {
                self.transcript_bytes = self
                    .transcript_bytes
                    .saturating_sub(removed.transcript.text.len());
            }
        }
        self.evict_transcripts();
    }

    fn evict_transcripts(&mut self) {
        while self.transcript_bytes > MAX_ARCHIVED_TRANSCRIPT_BYTES {
            let oldest = self
                .by_session
                .iter()
                .filter_map(|(session, entries)| {
                    entries
                        .front()
                        .map(|entry| (session.clone(), entry.ordinal))
                })
                .min_by_key(|(_, ordinal)| *ordinal)
                .map(|(session, _)| session);
            let Some(session) = oldest else { break };
            if let Some(removed) = self
                .by_session
                .get_mut(&session)
                .and_then(VecDeque::pop_front)
            {
                self.transcript_bytes = self
                    .transcript_bytes
                    .saturating_sub(removed.transcript.text.len());
            }
            if self
                .by_session
                .get(&session)
                .is_some_and(VecDeque::is_empty)
            {
                self.by_session.remove(&session);
            }
        }
    }

    fn remove_session(&mut self, session: &SessionId) {
        if let Some(entries) = self.by_session.remove(session) {
            let removed = entries
                .iter()
                .map(|entry| entry.transcript.text.len())
                .sum::<usize>();
            self.transcript_bytes = self.transcript_bytes.saturating_sub(removed);
        }
    }

    fn snapshots(&self, session: &SessionId) -> Vec<(u64, JobSnapshot)> {
        self.by_session
            .get(session)
            .into_iter()
            .flatten()
            .map(|entry| (entry.ordinal, entry.snapshot.clone()))
            .collect()
    }

    fn read(
        &self,
        session: &SessionId,
        id: &JobId,
        cursor: Option<u64>,
    ) -> Result<Option<JobViewRead>, String> {
        if let Some(entry) = self
            .by_session
            .get(session)
            .and_then(|entries| entries.iter().find(|entry| &entry.snapshot.id == id))
        {
            let (text, cursor, truncated) = entry.transcript.read_from(cursor);
            return Ok(Some(JobViewRead {
                text,
                cursor,
                truncated,
                snapshot: entry.snapshot.clone(),
            }));
        }
        if self
            .by_session
            .values()
            .any(|entries| entries.iter().any(|entry| &entry.snapshot.id == id))
        {
            return Err(format!("job {id} belongs to another session"));
        }
        Ok(None)
    }
}

impl JobTranscript {
    fn append(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        self.next_cursor = self.next_cursor.saturating_add(value.len() as u64);
        self.text.push_str(value);
        if self.text.len() <= MAX_VIEW_TRANSCRIPT_BYTES {
            return;
        }
        let mut minimum = self.text.len() - MAX_VIEW_TRANSCRIPT_BYTES;
        while minimum < self.text.len() && !self.text.is_char_boundary(minimum) {
            minimum += 1;
        }
        let newline = self.text[minimum..]
            .find('\n')
            .map(|offset| minimum + offset + 1)
            .filter(|offset| *offset <= minimum.saturating_add(16 * 1024));
        let mut remove = newline.unwrap_or(minimum);
        while remove < self.text.len() && !self.text.is_char_boundary(remove) {
            remove += 1;
        }
        self.text.drain(..remove);
        self.base_cursor = self.base_cursor.saturating_add(remove as u64);
    }

    fn read_from(&self, cursor: Option<u64>) -> (String, u64, bool) {
        let requested = cursor.unwrap_or(self.base_cursor);
        let truncated = requested < self.base_cursor;
        let clamped = requested.clamp(self.base_cursor, self.next_cursor);
        let mut offset = (clamped - self.base_cursor) as usize;
        while offset < self.text.len() && !self.text.is_char_boundary(offset) {
            offset += 1;
        }
        (self.text[offset..].to_string(), self.next_cursor, truncated)
    }
}

impl TrackedJob {
    fn is_terminal(&self) -> bool {
        self.status.lock().is_terminal()
    }

    fn snapshot(&self) -> JobSnapshot {
        JobSnapshot {
            id: self.id.clone(),
            kind: self.kind.clone(),
            label: self.label.clone(),
            output_limit_bytes: self.output_limit_bytes,
            owner_session: self.owner.as_ref().map(|owner| owner.id().clone()),
            status: *self.status.lock(),
            detail: self.detail.lock().clone(),
            started_at: self.started_at,
            finished_at: *self.finished_at.lock(),
            reported: self.reported.load(SeqCst),
        }
    }
}

/// One scope's contributions: the job controllers attached from it and the
/// completion listeners registered there (TS `JobLayer`).
struct JobLayer {
    controllers: AnonymousEntries<()>,
    listeners: AnonymousEntries<JobDoneListener>,
    changed: AnonymousEntries<JobsChangedListener>,
}

impl JobLayer {
    fn new() -> Self {
        Self {
            controllers: AnonymousEntries::new(),
            listeners: AnonymousEntries::new(),
            changed: AnonymousEntries::new(),
        }
    }
}

impl ScopeLayer for JobLayer {
    fn is_empty(&self) -> bool {
        self.controllers.is_empty() && self.listeners.is_empty() && self.changed.is_empty()
    }
}

fn render_panic(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&'static str>()
        .map(|message| (*message).to_string())
        .or_else(|| {
            payload
                .downcast_ref::<String>()
                .map(|message| message.clone())
        })
        .unwrap_or_else(|| "<non-string panic>".to_string())
}

/// The in-memory `jobs` registry (TS `LocalJobRegistry`).
pub struct LocalJobRegistry {
    pub ctx: Context,
    max_concurrent_jobs_per_owner: u64,
    store: Mutex<HashMap<JobId, Arc<TrackedJob>>>,
    completed: Mutex<CompletedArchive>,
    admissions: Mutex<HashMap<usize, u64>>,
    counters: Mutex<HashMap<String, u64>>,
    /// Monotonic registration sequence for the `list` order.
    next_ordinal: AtomicU64,
    layers: ScopedLayers<JobLayer>,
    listeners_closed: AtomicBool,
    /// Owner agents with attached scope cleanup, mapped to the exact disposer.
    owner_cleanups: Mutex<HashMap<usize, Disposer>>,
    /// Self handle for detached settlement continuations (the &self
    /// receivers cannot outlive the method; `start`'s spawned driver clones
    /// this).
    self_arc: std::sync::OnceLock<Arc<LocalJobRegistry>>,
}

impl Service for LocalJobRegistry {
    fn service_name(&self) -> &'static str {
        "jobs"
    }
}

impl LocalJobRegistry {
    /// Construct, validate, register as `ctx.jobs`, and attach the teardown
    /// effect (the TS constructor collapse).
    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let max_concurrent_jobs_per_owner = config
            .max_concurrent_jobs_per_owner
            .unwrap_or(DEFAULT_MAX_CONCURRENT_JOBS_PER_OWNER);
        if max_concurrent_jobs_per_owner == 0 {
            panic!("jobs-local: maxConcurrentJobsPerOwner must be a positive integer");
        }
        let registry = Arc::new(Self {
            ctx: ctx.clone(),
            max_concurrent_jobs_per_owner,
            store: Mutex::new(HashMap::new()),
            completed: Mutex::new(CompletedArchive::default()),
            admissions: Mutex::new(HashMap::new()),
            counters: Mutex::new(HashMap::new()),
            next_ordinal: AtomicU64::new(0),
            layers: ScopedLayers::new(|_scope| JobLayer::new(), || {}),
            listeners_closed: AtomicBool::new(false),
            owner_cleanups: Mutex::new(HashMap::new()),
            self_arc: std::sync::OnceLock::new(),
        });
        registry.self_arc.set(registry.clone()).ok();
        let archive = Arc::downgrade(&registry);
        let deleted_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
            let archive = archive.clone();
            let session = args
                .first()
                .and_then(|value| cordis::downcast::<SessionId>(value))
                .cloned();
            Box::pin(async move {
                if let (Some(registry), Some(session)) = (archive.upgrade(), session) {
                    registry.completed.lock().remove_session(&session);
                }
                None
            })
        });
        futures::executor::block_on(ctx.on(
            "workspace/session-deleted",
            deleted_listener,
            EventOptions::default().global(true),
        ));
        let teardown = registry.clone();
        let _ = ctx.effect(
            "jobs teardown",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let teardown = teardown.clone();
                    Box::pin(async move {
                        let _ = teardown.dispose_all().await;
                    })
                }))
            }),
        );
        // Register the ERASED capability seam (the concrete handle is
        // returned to the installer; a same-scope concrete registration
        // would make `get_typed::<Arc<dyn JobRegistry>>` lookups fail).
        let erased: Arc<dyn JobRegistry> = registry.clone();
        ctx.register_service(erased);
        registry
    }

    /// Whether an attached job controller can collect and stop work owned by
    /// `owner` (TS `servesOwner`).
    fn serves_owner(&self, owner: Option<&Arc<dyn Agent>>) -> bool {
        if !self.layers.global.controllers.is_empty() {
            return true;
        }
        let scope = owner.and_then(|owner| scope_of(owner.ctx()));
        self.layers
            .chain_layers(scope.as_ref())
            .into_iter()
            .any(|layer| !layer.controllers.is_empty())
    }

    fn reserve_admission(
        &self,
        owner: Option<&Arc<dyn Agent>>,
    ) -> Result<JobAdmission<'_>, String> {
        let key = owner_key(owner);
        let mut admissions = self.admissions.lock();
        let active = admissions.get(&key).copied().unwrap_or(0);
        if active >= self.max_concurrent_jobs_per_owner {
            return Err(format!(
                "background job limit reached for this owner (limit: {}); use job_kill to stop an unneeded job, wait for it to finish, then retry",
                self.max_concurrent_jobs_per_owner
            ));
        }
        admissions.insert(key, active + 1);
        Ok(JobAdmission {
            registry: self,
            key,
            armed: true,
        })
    }

    fn release_admission(&self, key: usize) {
        let mut admissions = self.admissions.lock();
        let Some(active) = admissions.get_mut(&key) else {
            return;
        };
        *active = active.saturating_sub(1);
        if *active == 0 {
            admissions.remove(&key);
        }
    }

    fn active_task_count(&self, owner: Option<&Arc<dyn Agent>>) -> u64 {
        self.admissions
            .lock()
            .get(&owner_key(owner))
            .copied()
            .unwrap_or(0)
    }

    fn notify_owner_idle(&self, owner: Option<&Arc<dyn Agent>>) {
        let Some(owner) = owner else { return };
        if !self.has_owner_activity(owner) {
            self.ctx
                .emit("jobs/owner-idle", vec![cordis::arc(owner.clone())]);
        }
    }

    fn expect(&self, id: &JobId) -> Result<Arc<TrackedJob>, String> {
        self.store
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown job {id}"))
    }

    fn assert_access(
        &self,
        job: &TrackedJob,
        caller: Option<&Arc<dyn Agent>>,
    ) -> Result<(), String> {
        if let Some(owner) = &job.owner {
            if caller.map(|caller| caller.id()) != Some(owner.id()) {
                return Err(format!("job {} belongs to another session", job.id));
            }
        }
        Ok(())
    }

    fn assert_session_access(job: &TrackedJob, caller: &SessionId) -> Result<(), String> {
        if let Some(owner) = &job.owner
            && owner.id() != caller
        {
            return Err(format!("job {} belongs to another session", job.id));
        }
        Ok(())
    }

    fn pump_transcript(job: &TrackedJob, transcript: &mut JobTranscript) -> bool {
        match job.hooks.read_output() {
            Some(delta) => {
                transcript.stream = Some(true);
                transcript.append(&delta);
            }
            None => {
                transcript.stream.get_or_insert(false);
                if job.is_terminal() && !transcript.final_captured {
                    if let Some(output) = job.output.lock().clone() {
                        transcript.append(&output);
                    }
                    transcript.final_captured = true;
                }
            }
        }
        transcript.stream == Some(true)
    }

    fn archive_job(&self, job: &TrackedJob) {
        let Some(session) = job.owner.as_ref().map(|owner| owner.id().clone()) else {
            return;
        };
        let mut transcript = job.transcript.lock();
        for _ in 0..1_024 {
            let cursor = transcript.next_cursor;
            Self::pump_transcript(job, &mut transcript);
            if transcript.next_cursor == cursor {
                break;
            }
        }
        let archived = ArchivedJob {
            snapshot: job.snapshot(),
            ordinal: job.ordinal,
            transcript: transcript.clone(),
        };
        drop(transcript);
        self.completed.lock().insert(session, archived);
    }

    /// The completion listeners that own `owner`'s notices: the global
    /// layer's first, then each scoped layer along the owner's chain.
    fn listeners_for(&self, owner: Option<&Arc<dyn Agent>>) -> Vec<JobDoneListener> {
        let mut listeners: Vec<JobDoneListener> = self.layers.global.listeners.values();
        let scope = owner.and_then(|owner| scope_of(owner.ctx()));
        for layer in self.layers.chain_layers(scope.as_ref()) {
            listeners.extend(layer.listeners.values());
        }
        listeners
    }

    fn changed_for(&self, owner: Option<&Arc<dyn Agent>>) -> Vec<JobsChangedListener> {
        let mut changed: Vec<JobsChangedListener> = self.layers.global.changed.values();
        let scope = owner.and_then(|owner| scope_of(owner.ctx()));
        for layer in self.layers.chain_layers(scope.as_ref()) {
            changed.extend(layer.changed.values());
        }
        changed
    }

    fn notify_changed(&self, owner: Option<&Arc<dyn Agent>>) {
        for listener in self.changed_for(owner) {
            if let Err(error) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(owner.cloned())))
            {
                self.ctx.logger.warn(
                    &self.ctx,
                    vec![cordis::arc(format!(
                        "jobs: onJobsChanged listener threw: {}",
                        render_panic(error)
                    ))],
                );
            }
        }
    }

    /// Record the first terminal outcome, release waiters, then announce
    /// completion (TS `settle`).
    fn settle(&self, job: &Arc<TrackedJob>, outcome: JobOutcome) {
        let settled_status = match outcome.status {
            JobOutcomeStatus::Completed => JobStatus::Completed,
            JobOutcomeStatus::Killed => JobStatus::Killed,
            JobOutcomeStatus::Failed => JobStatus::Failed,
        };
        {
            let mut status = job.status.lock();
            if status.is_terminal() {
                return;
            }
            *status = settled_status;
        }
        *job.detail.lock() = outcome.detail;
        *job.output.lock() = outcome.output;
        *job.finished_at.lock() = Some(epoch_ms());
        if !job.admission_released.swap(true, SeqCst) {
            self.release_admission(job.admission_key);
        }
        if job.waiters.load(SeqCst) > 0 {
            job.reported.store(true, SeqCst);
        }
        let snapshot = job.snapshot();
        // Release waiters and the settlement observers in one broadcast; the
        // flag carries the fact for late registrations.
        job.settled_flag.store(true, SeqCst);
        job.settled.notify_waiters();
        self.notify_changed(job.owner.as_ref());
        if self.listeners_closed.load(SeqCst) {
            return;
        }
        for listener in self.listeners_for(job.owner.as_ref()) {
            if let Err(error) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener(snapshot.clone(), job.owner.clone())
            })) {
                self.ctx.logger.warn(
                    &self.ctx,
                    vec![cordis::arc(format!(
                        "jobs: onJobDone listener threw for {}: {}",
                        job.id,
                        render_panic(error)
                    ))],
                );
            }
        }
        self.notify_owner_idle(job.owner.as_ref());
    }

    /// Attach one awaited cleanup through the exact owner's scope (TS
    /// `ensureOwnerCleanup`).
    fn ensure_owner_cleanup(&self, owner: &Arc<dyn Agent>) -> Result<(), String> {
        let Some(registry) = self
            .ctx
            .get_typed::<Arc<AgentRegistry>>("agents", false)
            .map(|slot| slot.as_ref().clone())
        else {
            return Err(
                "background job ownership requires the agent registry (load @deepseek-ai/dsh-agent)"
                    .to_string(),
            );
        };
        let live = registry.get(owner.id());
        if !live
            .as_ref()
            .is_some_and(|registered| Arc::ptr_eq(registered, owner))
        {
            return Err(format!(
                "agent \"{}\" is not the registered agent instance (background job owner must be live)",
                owner.id()
            ));
        }
        let key = owner_key(Some(owner));
        if self.owner_cleanups.lock().contains_key(&key) {
            return Ok(());
        }
        let registry_for_effect = self.self_arc.get().expect("installed").clone();
        let owner_for_effect = owner.clone();
        let disposer = owner.ctx().effect(
            "jobs.ownerCleanup()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let registry = registry_for_effect.clone();
                    let owner = owner_for_effect.clone();
                    Box::pin(async move {
                        registry
                            .owner_cleanups
                            .lock()
                            .remove(&owner_key(Some(&owner)));
                        let _ = registry.dispose_owned(&owner).await;
                    })
                }))
            }),
        );
        self.owner_cleanups.lock().insert(key, disposer);
        Ok(())
    }

    /// Cancel, await terminal records, and drop every job owned by one exact
    /// agent lifecycle (TS `disposeOwned`).
    async fn dispose_owned(&self, owner: &Arc<dyn Agent>) -> Result<(), String> {
        let owned: Vec<Arc<TrackedJob>> = self
            .store
            .lock()
            .values()
            .filter(|job| owner_key(job.owner.as_ref()) == owner_key(Some(owner)))
            .cloned()
            .collect();
        self.cancel_for_teardown(&owned, "owner disposed");
        for job in &owned {
            wait_settled(job).await;
            self.archive_job(job);
        }
        {
            let mut store = self.store.lock();
            for job in &owned {
                store.remove(&job.id);
            }
        }
        // Removal is the one visible-set change no per-job record carries.
        if !owned.is_empty() {
            self.notify_changed(Some(owner));
        }
        Ok(())
    }

    /// Close listeners, cancel live jobs, await settlement, and detach owner
    /// effects (TS `disposeAll`). Public (doc-hidden) for the teardown tests;
    /// the service effect calls it on disposal.
    #[doc(hidden)]
    pub async fn dispose_all(&self) -> Result<(), String> {
        self.listeners_closed.store(true, SeqCst);
        let all: Vec<Arc<TrackedJob>> = self.store.lock().values().cloned().collect();
        self.cancel_for_teardown(&all, "jobs service disposed");
        for job in &all {
            wait_settled(job).await;
        }
        let emptied: Vec<Option<Arc<dyn Agent>>> =
            all.iter().map(|job| job.owner.clone()).collect();
        self.store.lock().clear();
        self.admissions.lock().clear();
        {
            let mut completed = self.completed.lock();
            completed.by_session.clear();
            completed.transcript_bytes = 0;
        }
        for owner in &emptied {
            self.notify_changed(owner.as_ref());
        }
        // Detach cross-fiber owner effects after the shared store is
        // quiescent.
        let cleanups: Vec<Disposer> = self
            .owner_cleanups
            .lock()
            .drain()
            .map(|(_, disposer)| disposer)
            .collect();
        for cleanup in cleanups {
            (cleanup)().await;
        }
        Ok(())
    }

    /// Cancel jobs during teardown with per-job containment (TS
    /// `cancelForTeardown`).
    fn cancel_for_teardown(&self, jobs: &[Arc<TrackedJob>], reason: &str) {
        for job in jobs {
            if job.is_terminal() {
                continue;
            }
            job.reported.store(true, SeqCst);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                job.hooks.cancel(Some(reason.to_string()));
            }));
            match outcome {
                Ok(()) => {
                    let changed = {
                        let mut status = job.status.lock();
                        if status.is_terminal() {
                            false
                        } else {
                            *status = JobStatus::Stopping;
                            true
                        }
                    };
                    if changed {
                        self.notify_changed(job.owner.as_ref());
                    }
                }
                Err(error) => {
                    let rendered = render_panic(error);
                    let detail =
                        format!("cancel threw during teardown; work may be orphaned: {rendered}");
                    self.ctx.logger.warn(
                        &self.ctx,
                        vec![cordis::arc(format!(
                            "jobs: cancel of {} threw during teardown; job record forced failed and work may be orphaned: {rendered}",
                            job.id
                        ))],
                    );
                    self.settle(
                        job,
                        JobOutcome {
                            status: JobOutcomeStatus::Failed,
                            detail: Some(detail),
                            output: None,
                        },
                    );
                }
            }
        }
    }
}

/// Exact-owner identity (the TS object-identity `Map<Agent, …>` key
/// collapse). `None` owners share the unowned bucket under key 0.
fn owner_key(owner: Option<&Arc<dyn Agent>>) -> usize {
    owner
        .map(|agent| Arc::as_ptr(agent) as *const () as usize)
        .unwrap_or(0)
}

/// Await one job's settlement, spurious-safe (the flag carries the fact for
/// waiters registered after the broadcast).
async fn wait_settled(job: &Arc<TrackedJob>) {
    loop {
        if job.settled_flag.load(SeqCst) {
            return;
        }
        job.settled.notified().await;
    }
}

impl JobRegistry for LocalJobRegistry {
    fn start(&self, spec: JobStart) -> Result<JobId, String> {
        if !self.serves_owner(spec.owner.as_ref()) {
            return Err(
                "background jobs unavailable: no job controller serves this agent (load @deepseek-ai/dsh-tool-jobs in its composition)"
                    .to_string(),
            );
        }
        if spec.kind.is_empty() {
            return Err("invalid job kind: expected a non-empty string".to_string());
        }
        if spec.label.is_empty() {
            return Err("invalid job label: expected a non-empty string".to_string());
        }
        if spec.output_limit_bytes == Some(0) {
            return Err(format!(
                "invalid outputLimitBytes: expected a positive safe integer, got {:?}",
                spec.output_limit_bytes
            ));
        }
        if let Some(owner) = &spec.owner {
            self.ensure_owner_cleanup(owner)?;
        }
        let admission = self.reserve_admission(spec.owner.as_ref())?;
        let hooks = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)())) {
            Ok(hooks) => hooks,
            Err(error) => {
                return Err(format!(
                    "background job starter panicked: {}",
                    render_panic(error)
                ));
            }
        };
        let count = {
            let mut counters = self.counters.lock();
            let count = counters.get(&spec.kind).copied().unwrap_or(0) + 1;
            counters.insert(spec.kind.clone(), count);
            count
        };
        let id = job_id(format!("{}-{count}", spec.kind));

        let job = Arc::new(TrackedJob {
            id: id.clone(),
            kind: spec.kind.clone(),
            label: spec.label.clone(),
            output_limit_bytes: spec.output_limit_bytes,
            owner: spec.owner.clone(),
            hooks: hooks.clone(),
            status: Mutex::new(JobStatus::Running),
            detail: Mutex::new(None),
            output: Mutex::new(None),
            started_at: epoch_ms(),
            finished_at: Mutex::new(None),
            reported: AtomicBool::new(false),
            ordinal: self.next_ordinal.fetch_add(1, SeqCst) + 1,
            settled: Arc::new(Notify::new()),
            settled_flag: AtomicBool::new(false),
            waiters: AtomicU64::new(0),
            transcript: Mutex::new(JobTranscript::default()),
            admission_key: owner_key(spec.owner.as_ref()),
            admission_released: AtomicBool::new(false),
        });
        self.store.lock().insert(id.clone(), job.clone());
        admission.commit();

        // The producer `done` settles without a consumer (the TS `void
        // hooks.done.then(...)`).
        {
            let registry = self.self_arc.get().expect("installed").clone();
            let job_for_done = job.clone();
            tokio::spawn(async move {
                let outcome = std::panic::AssertUnwindSafe(async { hooks.done().await })
                    .catch_unwind()
                    .await;
                match outcome {
                    Ok(outcome) => registry.settle(&job_for_done, outcome),
                    Err(error) => {
                        // Contain a producer contract violation (`done`
                        // panicked) so cleanup and waiters cannot hang.
                        let detail = render_panic(error);
                        registry.settle(
                            &job_for_done,
                            JobOutcome {
                                status: JobOutcomeStatus::Failed,
                                detail: Some(detail),
                                output: None,
                            },
                        );
                    }
                }
            });
        }
        // Registration is complete and cannot fail from here, so the visible
        // set has genuinely changed.
        self.notify_changed(job.owner.as_ref());
        Ok(id)
    }

    fn list(&self, caller: Option<&Arc<dyn Agent>>) -> Vec<JobSnapshot> {
        let session = caller.map(|caller| caller.id().clone());
        let jobs: Vec<Arc<TrackedJob>> = self
            .store
            .lock()
            .values()
            .filter(|job| match &job.owner {
                None => true,
                Some(owner) => Some(owner.id()) == session.as_ref(),
            })
            .cloned()
            .collect();
        let mut visible = jobs
            .iter()
            .map(|job| (job.ordinal, job.snapshot()))
            .collect::<Vec<_>>();
        if let Some(session) = session.as_ref() {
            let live = visible
                .iter()
                .map(|(_, snapshot)| snapshot.id.clone())
                .collect::<std::collections::HashSet<_>>();
            visible.extend(
                self.completed
                    .lock()
                    .snapshots(session)
                    .into_iter()
                    .filter(|(_, snapshot)| !live.contains(&snapshot.id)),
            );
        }
        visible.sort_by_key(|(ordinal, _)| *ordinal);
        visible.into_iter().map(|(_, snapshot)| snapshot).collect()
    }

    fn list_for_session(&self, caller: &SessionId) -> Vec<JobSnapshot> {
        let jobs = self.store.lock();
        let mut visible = jobs
            .values()
            .filter(|job| job.owner.as_ref().is_none_or(|owner| owner.id() == caller))
            .map(|job| (job.ordinal, job.snapshot()))
            .collect::<Vec<_>>();
        drop(jobs);
        let live = visible
            .iter()
            .map(|(_, snapshot)| snapshot.id.clone())
            .collect::<std::collections::HashSet<_>>();
        visible.extend(
            self.completed
                .lock()
                .snapshots(caller)
                .into_iter()
                .filter(|(_, snapshot)| !live.contains(&snapshot.id)),
        );
        visible.sort_by_key(|(ordinal, _)| *ordinal);
        visible.into_iter().map(|(_, snapshot)| snapshot).collect()
    }

    fn has_owner_activity(&self, owner: &Arc<dyn Agent>) -> bool {
        self.active_task_count(Some(owner)) > 0
    }

    fn get(&self, id: &JobId, caller: Option<&Arc<dyn Agent>>) -> Result<JobSnapshot, String> {
        let job = self.expect(id)?;
        self.assert_access(&job, caller)?;
        Ok(job.snapshot())
    }

    fn read(&self, id: &JobId, caller: Option<&Arc<dyn Agent>>) -> Result<JobRead, String> {
        let job = self.expect(id)?;
        self.assert_access(&job, caller)?;
        let mut transcript = job.transcript.lock();
        let stream = Self::pump_transcript(&job, &mut transcript);
        let text = if stream {
            let (mut text, cursor, truncated) = transcript.read_from(Some(transcript.model_cursor));
            transcript.model_cursor = cursor;
            if truncated {
                text.insert_str(0, "[earlier background output was truncated]\n");
            }
            text
        } else if job.is_terminal() {
            job.output.lock().clone().unwrap_or_default()
        } else {
            String::new()
        };
        if job.is_terminal() {
            job.reported.store(true, SeqCst);
        }
        Ok(JobRead {
            text,
            snapshot: job.snapshot(),
        })
    }

    fn read_view(
        &self,
        id: &JobId,
        caller: Option<&Arc<dyn Agent>>,
        cursor: Option<u64>,
    ) -> Result<JobViewRead, String> {
        let job = self.expect(id)?;
        self.assert_access(&job, caller)?;
        let mut transcript = job.transcript.lock();
        Self::pump_transcript(&job, &mut transcript);
        let (text, cursor, truncated) = transcript.read_from(cursor);
        Ok(JobViewRead {
            text,
            cursor,
            truncated,
            snapshot: job.snapshot(),
        })
    }

    fn read_view_for_session(
        &self,
        id: &JobId,
        caller: &SessionId,
        cursor: Option<u64>,
    ) -> Result<JobViewRead, String> {
        if let Some(job) = self.store.lock().get(id).cloned() {
            Self::assert_session_access(&job, caller)?;
            let mut transcript = job.transcript.lock();
            Self::pump_transcript(&job, &mut transcript);
            let (text, cursor, truncated) = transcript.read_from(cursor);
            return Ok(JobViewRead {
                text,
                cursor,
                truncated,
                snapshot: job.snapshot(),
            });
        }
        self.completed
            .lock()
            .read(caller, id, cursor)?
            .ok_or_else(|| format!("unknown job {id}"))
    }

    fn kill(
        &self,
        id: &JobId,
        caller: Option<&Arc<dyn Agent>>,
        reason: Option<String>,
    ) -> Result<KillOutcome, String> {
        let job = self.expect(id)?;
        self.assert_access(&job, caller)?;
        if job.is_terminal() {
            job.reported.store(true, SeqCst);
            return Ok(KillOutcome::AlreadyFinished);
        }
        // Cancel first so a throw leaves both lifecycle and notice state
        // unchanged.
        job.hooks.cancel(reason);
        {
            let mut status = job.status.lock();
            if !status.is_terminal() {
                *status = JobStatus::Stopping;
            }
        }
        job.reported.store(true, SeqCst);
        self.notify_changed(job.owner.as_ref());
        Ok(KillOutcome::Requested)
    }

    fn wait(
        &self,
        id: &JobId,
        timeout_ms: u64,
        caller: Option<&Arc<dyn Agent>>,
        signal: Option<dsh_jobs::JobAbort>,
    ) -> BoxFuture<'static, Result<JobSnapshot, String>> {
        // The TS async function's synchronous prefix: validation and waiter
        // registration happen at the call.
        let job = match self.expect(id) {
            Ok(job) => job,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        if let Err(error) = self.assert_access(&job, caller) {
            return Box::pin(async move { Err(error) });
        }
        if timeout_ms == 0 {
            return Box::pin(async move {
                Err(format!(
                    "invalid wait timeout: expected a positive number of milliseconds, got {timeout_ms:?}"
                ))
            });
        }
        let live = !job.is_terminal();
        if live {
            if signal.as_ref().is_some_and(|signal| signal()) {
                return Box::pin(async move { Err("wait aborted".to_string()) });
            }
            job.waiters.fetch_add(1, SeqCst);
        }
        let settled = job.settled.clone();
        let job_for_wait = job.clone();
        let counted = live;
        Box::pin(async move {
            let result = if live {
                // The scoped deadline distinguishes a successful wait timeout
                // from caller cancellation and clears its timer on every exit.
                let upstream = DeadlineSignal::never();
                let mut deadline = deadline(Some(&upstream), timeout_ms, TASK_WAIT_TIMEOUT);
                let fused = Arc::new(std::mem::replace(
                    &mut deadline.signal,
                    DeadlineSignal::never(),
                ));
                let poller = signal.map(|abort| {
                    let fused_for_poll = fused.clone();
                    tokio::spawn(async move {
                        loop {
                            if abort() {
                                fused_for_poll.cancel(None);
                                return;
                            }
                            tokio::time::sleep(Duration::from_millis(15)).await;
                        }
                    })
                });
                loop {
                    if job_for_wait.settled_flag.load(SeqCst) {
                        break;
                    }
                    tokio::select! {
                        _ = settled.notified() => break,
                        _ = tokio::time::sleep(Duration::from_millis(15)) => {
                            if fused.is_cancelled() {
                                break;
                            }
                        }
                    }
                }
                if let Some(poller) = poller {
                    poller.abort();
                }
                let timed_out =
                    timeout_of(fused.reason().as_ref(), Some(TASK_WAIT_TIMEOUT)).is_some();
                if fused.is_cancelled() && !timed_out {
                    Err("wait aborted".to_string())
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            };
            if counted {
                job.waiters.fetch_sub(1, SeqCst);
            }
            match result {
                Err(error) => Err(error),
                Ok(()) => {
                    if job.is_terminal() {
                        job.reported.store(true, SeqCst);
                    }
                    Ok(job.snapshot())
                }
            }
        })
    }

    fn on_job_done(&self, caller: &Context, listener: JobDoneListener) -> Disposer {
        self.layers.effect(
            caller,
            move |layer| layer.listeners.append(listener.clone()),
            "jobs.onJobDone()",
            false,
        )
    }

    fn on_jobs_changed(&self, caller: &Context, listener: JobsChangedListener) -> Disposer {
        self.layers.effect(
            caller,
            move |layer| layer.changed.append(listener.clone()),
            "jobs.onJobsChanged()",
            false,
        )
    }

    fn attach_controller(&self, caller: &Context, _name: &str) -> Disposer {
        self.layers.effect(
            caller,
            move |layer| layer.controllers.append(()),
            "jobs.attachController()",
            false,
        )
    }
}

#[cfg(test)]
mod view_tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use cordis::Context;
    use dsh_agent::{
        Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
        InboxNotifications, InboxTarget,
    };
    use dsh_jobs::{JobHooks, JobOutcome, JobRegistry, JobSnapshot, JobStart, JobStatus, job_id};
    use dsh_scope::ScopeKey;
    use dsh_session::{Session, SessionStore, UserMessage, session_id};
    use futures::future::BoxFuture;
    use parking_lot::Mutex;

    use super::{
        ArchivedJob, CompletedArchive, Config, JobTranscript, LocalJobRegistry,
        MAX_ARCHIVED_JOBS_PER_SESSION, MAX_ARCHIVED_TRANSCRIPT_BYTES, MAX_VIEW_TRANSCRIPT_BYTES,
    };

    struct TestAgent {
        id: dsh_session::SessionId,
        options: AgentOptions,
        session: Session,
        inbox: Inbox,
        ctx: Context,
        scope_key: ScopeKey,
    }

    impl Agent for TestAgent {
        fn id(&self) -> &dsh_session::SessionId {
            &self.id
        }
        fn options(&self) -> &AgentOptions {
            &self.options
        }
        fn session(&self) -> &Session {
            &self.session
        }
        fn inbox(&self) -> &Inbox {
            &self.inbox
        }
        fn status(&self) -> AgentStatus {
            AgentStatus::Running
        }
        fn ctx(&self) -> &Context {
            &self.ctx
        }
        fn scope_key(&self) -> &ScopeKey {
            &self.scope_key
        }
        fn cancel(&self, _cause: AgentCancelCause, _options: Option<&CancelOptions>) {}
        fn when_idle(&self) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }
        fn run_maintenance(
            &self,
            _task: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
        ) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }
        fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}
        fn followup(&self, _message: UserMessage) {}
        fn steer(&self, _message: UserMessage) {}
        fn inject(&self, _message: UserMessage) {}
    }

    async fn agent(ctx: &Context, name: &str) -> Arc<dyn Agent> {
        let sessions = ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
            .unwrap_or_else(|| SessionStore::install(ctx));
        let id = session_id(name);
        let session = sessions
            .create(ctx, Some(id.clone()), None)
            .await
            .expect("create test session");
        let inbox = Inbox::new(&session, InboxNotifications::default()).expect("create inbox");
        Arc::new(TestAgent {
            id,
            options: AgentOptions::default(),
            session,
            inbox,
            ctx: ctx.clone(),
            scope_key: ScopeKey::new(),
        })
    }

    struct StreamHooks {
        output: Mutex<VecDeque<String>>,
        done: Arc<tokio::sync::Notify>,
    }

    impl JobHooks for StreamHooks {
        fn cancel(&self, _reason: Option<String>) {}
        fn done(&self) -> BoxFuture<'static, JobOutcome> {
            let done = self.done.clone();
            Box::pin(async move {
                done.notified().await;
                JobOutcome {
                    status: dsh_jobs::JobOutcomeStatus::Completed,
                    detail: None,
                    output: None,
                }
            })
        }
        fn read_output(&self) -> Option<String> {
            Some(self.output.lock().pop_front().unwrap_or_default())
        }
    }

    #[test]
    fn bounded_view_transcript_trims_only_at_utf8_boundaries() {
        let mut transcript = JobTranscript::default();
        transcript.append(&"你".repeat(MAX_VIEW_TRANSCRIPT_BYTES / 3 + 32));
        assert!(transcript.text.len() <= MAX_VIEW_TRANSCRIPT_BYTES);
        let (text, cursor, truncated) = transcript.read_from(Some(0));
        assert!(truncated);
        assert!(!text.is_empty());
        assert_eq!(cursor, transcript.next_cursor);
    }

    fn archived_job(owner: &dsh_session::SessionId, ordinal: u64, bytes: usize) -> ArchivedJob {
        let id = job_id(format!("archive-{ordinal}"));
        let mut transcript = JobTranscript::default();
        transcript.append(&"x".repeat(bytes));
        ArchivedJob {
            snapshot: JobSnapshot {
                id,
                kind: "fixture".to_string(),
                label: format!("archive {ordinal}"),
                output_limit_bytes: None,
                owner_session: Some(owner.clone()),
                status: JobStatus::Completed,
                detail: None,
                started_at: ordinal,
                finished_at: Some(ordinal),
                reported: false,
            },
            ordinal,
            transcript,
        }
    }

    #[test]
    fn completed_archive_evicts_by_session_count_and_global_transcript_budget() {
        let owner = session_id("archive-owner");
        let mut archive = CompletedArchive::default();
        for ordinal in 1..=(MAX_ARCHIVED_JOBS_PER_SESSION as u64 + 2) {
            archive.insert(owner.clone(), archived_job(&owner, ordinal, 1));
        }
        let entries = archive.by_session.get(&owner).expect("owner archive");
        assert_eq!(entries.len(), MAX_ARCHIVED_JOBS_PER_SESSION);
        assert_eq!(entries.front().unwrap().ordinal, 3);

        let mut archive = CompletedArchive::default();
        let other = session_id("archive-other");
        archive.insert(
            owner.clone(),
            archived_job(&owner, 1, MAX_ARCHIVED_TRANSCRIPT_BYTES * 3 / 4),
        );
        archive.insert(
            other.clone(),
            archived_job(&other, 2, MAX_ARCHIVED_TRANSCRIPT_BYTES * 3 / 4),
        );
        assert!(archive.transcript_bytes <= MAX_ARCHIVED_TRANSCRIPT_BYTES);
        assert!(!archive.by_session.contains_key(&owner));
        assert!(archive.by_session.contains_key(&other));
    }

    #[tokio::test]
    async fn view_cursor_never_consumes_model_output_and_keeps_owner_fence() {
        let ctx = Context::root();
        SessionStore::install(&ctx);
        let agents = AgentRegistry::install(&ctx);
        let registry = LocalJobRegistry::install(&ctx, Config::default());
        let _controller = registry.attach_controller(&ctx, "view-test");
        let owner = agent(&ctx, "view-owner").await;
        let other = agent(&ctx, "view-other").await;
        let owner_entry = agents.enter(owner.clone(), None).expect("register owner");
        let _other_entry = agents.enter(other.clone(), None).expect("register other");
        let hooks = Arc::new(StreamHooks {
            output: Mutex::new(VecDeque::from([
                "first\n".to_string(),
                "second\n".to_string(),
            ])),
            done: Arc::new(tokio::sync::Notify::new()),
        });
        let completion = hooks.done.clone();
        let job = registry
            .start(JobStart {
                kind: "fixture".to_string(),
                label: "view cursor fixture".to_string(),
                output_limit_bytes: Some(1024),
                owner: Some(owner.clone()),
                run: Arc::new(move || hooks.clone()),
            })
            .expect("start fixture job");

        let first_view = registry
            .read_view(&job, Some(&owner), None)
            .expect("first UI read");
        assert_eq!(first_view.text, "first\n");
        assert!(!first_view.truncated);

        let model = registry.read(&job, Some(&owner)).expect("model read");
        assert_eq!(model.text, "first\nsecond\n");

        let second_view = registry
            .read_view(&job, Some(&owner), Some(first_view.cursor))
            .expect("incremental UI read");
        assert_eq!(second_view.text, "second\n");
        let repeated = registry
            .read_view(&job, Some(&owner), Some(second_view.cursor))
            .expect("repeat UI read");
        assert_eq!(repeated.text, "");
        assert_eq!(repeated.cursor, second_view.cursor);

        let failure = registry
            .read_view(&job, Some(&other), None)
            .expect_err("cross-owner view must fail");
        assert!(failure.contains("belongs to another session"));
        assert_eq!(
            registry
                .read(&job, Some(&owner))
                .expect("second model read")
                .text,
            ""
        );
        assert!(registry.has_owner_activity(&owner));
        completion.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while registry.has_owner_activity(&owner) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("settled job releases owner activity");
        registry
            .dispose_owned(&owner)
            .await
            .expect("retirement archives completed jobs");
        owner_entry().await;
        assert!(agents.get(owner.id()).is_none());
        assert!(
            registry
                .list_for_session(owner.id())
                .iter()
                .any(|entry| entry.id == job)
        );
        let archived = registry
            .read_view_for_session(&job, owner.id(), None)
            .expect("retired owner can read archived output");
        assert_eq!(archived.text, "first\nsecond\n");
        let failure = registry
            .read_view_for_session(&job, other.id(), None)
            .expect_err("another session cannot read archived output");
        assert!(failure.contains("belongs to another session"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_admission_is_atomic_and_starter_panic_releases_its_slot() {
        let ctx = Context::root();
        SessionStore::install(&ctx);
        let agents = AgentRegistry::install(&ctx);
        let registry = LocalJobRegistry::install(
            &ctx,
            Config {
                max_concurrent_jobs_per_owner: Some(3),
            },
        );
        let _controller = registry.attach_controller(&ctx, "admission-test");
        let owner = agent(&ctx, "admission-owner").await;
        let _entry = agents.enter(owner.clone(), None).expect("register owner");

        let panic = registry
            .start(JobStart {
                kind: "panic".to_string(),
                label: "panicking starter".to_string(),
                output_limit_bytes: None,
                owner: Some(owner.clone()),
                run: Arc::new(|| panic!("starter fixture")),
            })
            .expect_err("starter panic must be contained");
        assert!(panic.contains("starter fixture"));
        assert!(!registry.has_owner_activity(&owner));

        let barrier = Arc::new(std::sync::Barrier::new(9));
        let completions = Arc::new(Mutex::new(Vec::<Arc<tokio::sync::Notify>>::new()));
        let mut starts = Vec::new();
        for index in 0..8 {
            let registry = registry.clone();
            let owner = owner.clone();
            let barrier = barrier.clone();
            let completions = completions.clone();
            starts.push(tokio::task::spawn_blocking(move || {
                barrier.wait();
                registry.start(JobStart {
                    kind: "parallel".to_string(),
                    label: format!("parallel {index}"),
                    output_limit_bytes: None,
                    owner: Some(owner),
                    run: Arc::new(move || {
                        let done = Arc::new(tokio::sync::Notify::new());
                        completions.lock().push(done.clone());
                        Arc::new(StreamHooks {
                            output: Mutex::new(VecDeque::new()),
                            done,
                        })
                    }),
                })
            }));
        }
        barrier.wait();
        let mut accepted = 0;
        for start in starts {
            if start.await.expect("join admission worker").is_ok() {
                accepted += 1;
            }
        }
        assert_eq!(accepted, 3);
        assert_eq!(registry.active_task_count(Some(&owner)), 3);
        for completion in completions.lock().iter() {
            completion.notify_one();
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while registry.has_owner_activity(&owner) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all admitted jobs settle and release their slots");
        assert_eq!(registry.active_task_count(Some(&owner)), 0);
    }
}
