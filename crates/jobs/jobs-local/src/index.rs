//! Process-local provider for the background-job capability seam
//! (`ctx.jobs`). It keeps every record in memory and hands out fresh
//! snapshots, never live state. Rust port of
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

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cordis::{Context, Disposer, Service, make_disposer};
use dsh_agent::{Agent, AgentRegistry};
use dsh_jobs::{
    JobDoneListener, JobHooks, JobId, JobOutcome, JobOutcomeStatus, JobRead, JobRegistry,
    JobSnapshot, JobStart, JobStatus, JobsChangedListener, KillOutcome, job_id,
};
use dsh_scope::{AnonymousEntries, ScopeLayer, ScopedLayers, scope_of};
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
    /// must observe the flag 鈥?the TS settled promise's value semantics).
    settled_flag: AtomicBool,
    /// Live waits; settlement with a waiter marks the job reported.
    waiters: AtomicU64,
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
        .or_else(|| payload.downcast_ref::<String>().map(|message| message.clone()))
        .unwrap_or_else(|| "<non-string panic>".to_string())
}

/// The in-memory `jobs` registry (TS `LocalJobRegistry`).
pub struct LocalJobRegistry {
    pub ctx: Context,
    max_concurrent_jobs_per_owner: u64,
    store: Mutex<HashMap<JobId, Arc<TrackedJob>>>,
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
            counters: Mutex::new(HashMap::new()),
            next_ordinal: AtomicU64::new(0),
            layers: ScopedLayers::new(|_scope| JobLayer::new(), || {}),
            listeners_closed: AtomicBool::new(false),
            owner_cleanups: Mutex::new(HashMap::new()),
            self_arc: std::sync::OnceLock::new(),
        });
        registry.self_arc.set(registry.clone()).ok();
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

    fn active_task_count(&self, owner: Option<&Arc<dyn Agent>>) -> u64 {
        self.store
            .lock()
            .values()
            .filter(|job| {
                owner_key(job.owner.as_ref()) == owner_key(owner)
                    && matches!(*job.status.lock(), JobStatus::Running | JobStatus::Stopping)
            })
            .count() as u64
    }

    fn expect(&self, id: &JobId) -> Result<Arc<TrackedJob>, String> {
        self.store
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown job {id}"))
    }

    fn assert_access(&self, job: &TrackedJob, caller: Option<&Arc<dyn Agent>>) -> Result<(), String> {
        if let Some(owner) = &job.owner {
            if caller.map(|caller| caller.id()) != Some(owner.id()) {
                return Err(format!("job {} belongs to another session", job.id));
            }
        }
        Ok(())
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
            if let Err(error) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener(owner.cloned())
            })) {
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
        if job.is_terminal() {
            return;
        }
        *job.status.lock() = match outcome.status {
            JobOutcomeStatus::Completed => JobStatus::Completed,
            JobOutcomeStatus::Killed => JobStatus::Killed,
            JobOutcomeStatus::Failed => JobStatus::Failed,
        };
        *job.detail.lock() = outcome.detail;
        *job.output.lock() = outcome.output;
        *job.finished_at.lock() = Some(epoch_ms());
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
        if !live.as_ref().is_some_and(|registered| Arc::ptr_eq(registered, owner)) {
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
        for owner in &emptied {
            self.notify_changed(owner.as_ref());
        }
        // Detach cross-fiber owner effects after the shared store is
        // quiescent.
        let cleanups: Vec<Disposer> = self.owner_cleanups.lock().drain().map(|(_, disposer)| disposer).collect();
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
                    *job.status.lock() = JobStatus::Stopping;
                    self.notify_changed(job.owner.as_ref());
                }
                Err(error) => {
                    let rendered = render_panic(error);
                    let detail = format!(
                        "cancel threw during teardown; work may be orphaned: {rendered}"
                    );
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

/// Exact-owner identity (the TS object-identity `Map<Agent, 鈥?` key
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

        let active = self.active_task_count(spec.owner.as_ref());
        if active >= self.max_concurrent_jobs_per_owner {
            return Err(format!(
                "background job limit reached for this owner (limit: {}); use job_kill to stop an unneeded job, wait for it to finish, then retry",
                self.max_concurrent_jobs_per_owner
            ));
        }

        let hooks = (spec.run)();
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
        });
        self.store.lock().insert(id.clone(), job.clone());

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
        let mut jobs: Vec<Arc<TrackedJob>> = self
            .store
            .lock()
            .values()
            .filter(|job| match &job.owner {
                None => true,
                Some(owner) => Some(owner.id()) == session.as_ref(),
            })
            .cloned()
            .collect();
        jobs.sort_by_key(|job| job.ordinal);
        jobs.iter().map(|job| job.snapshot()).collect()
    }

    fn get(&self, id: &JobId, caller: Option<&Arc<dyn Agent>>) -> Result<JobSnapshot, String> {
        let job = self.expect(id)?;
        self.assert_access(&job, caller)?;
        Ok(job.snapshot())
    }

    fn read(&self, id: &JobId, caller: Option<&Arc<dyn Agent>>) -> Result<JobRead, String> {
        let job = self.expect(id)?;
        self.assert_access(&job, caller)?;
        let text = match job.hooks.read_output() {
            Some(delta) => delta,
            None => {
                if job.is_terminal() {
                    job.output.lock().clone().unwrap_or_default()
                } else {
                    String::new()
                }
            }
        };
        if job.is_terminal() {
            job.reported.store(true, SeqCst);
        }
        Ok(JobRead {
            text,
            snapshot: job.snapshot(),
        })
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
        *job.status.lock() = JobStatus::Stopping;
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
