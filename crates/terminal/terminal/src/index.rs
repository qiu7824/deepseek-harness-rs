//! Owner-scoped persistent PTY registry. Backends own terminal mechanics
//! while this service owns ids, publication, authorization, and awaited
//! cleanup. Rust port of `packages/terminal/terminal/src/index.ts`.
//!
//! # Deviations
//!
//! - Rust futures are lazy where TS `async` functions run their synchronous
//!   prefix at call time. To preserve the TS-observable ordering (name
//!   reservations visible immediately, `disposing` set at call, kill's
//!   `closing` fence set before callers observe it), the synchronous prefix
//!   of `spawn`, `kill`, `dispose_owned`, and `dispose_all` runs at the call
//!   and returns `Result<BoxFuture, _>` / `BoxFuture` carrying the rest.
//! - Caller-cancellation reason objects collapse into
//!   [`TerminalFailure::Aborted`]; backend `done` rejections collapse into
//!   panics (the repo-wide error-channel convention).
//! - The service methods that spawn tasks (`start_send`, owner-cleanup
//!   registration) require a live tokio runtime.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};

use cordis::{Context, Disposer, Service, make_disposer};
use dsh_agent::{Agent, AgentRegistry};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::types::{
    TerminalAbort, TerminalBackend, TerminalBackendSession, TerminalBackendSpawnSpec,
    TerminalError, TerminalErrorCode, TerminalFailure, TerminalReadRequest, TerminalReadResult,
    TerminalSendOperation, TerminalSendRequest, TerminalSessionId, TerminalSessionSnapshot,
    TerminalSignal, TerminalSignalResult, TerminalSpawnRequest, TerminalSpawnResult,
    terminal_session_id,
};

/// Exact-owner identity for the registry maps (the TS object-identity
/// `Map<Agent, …>` keys).
pub fn owner_key(owner: &Arc<dyn Agent>) -> usize {
    Arc::as_ptr(owner) as *const () as usize
}

/// Published-session bookkeeping (TS `SessionRecord`).
pub struct SessionRecord {
    pub id: TerminalSessionId,
    pub owner: Arc<dyn Agent>,
    pub name: Option<String>,
    pub type_: String,
    pub session: Arc<dyn TerminalBackendSession>,
    pub active: Mutex<Option<Arc<dyn TerminalSendOperation>>>,
    pub closing: Mutex<Option<Shared<BoxFuture<'static, Result<(), String>>>>>,
    /// Bumped whenever a NEW close fence is installed; error paths clear the
    /// fence only while the generation is unchanged (the TS `record.closing
    /// === closing` identity guard — `Shared` has no public pointer).
    close_generation: AtomicU64,
}

impl SessionRecord {
    /// Install a close fence and return its generation (bumped per install).
    fn install_closing(&self, fence: Shared<BoxFuture<'static, Result<(), String>>>) -> u64 {
        *self.closing.lock() = Some(fence);
        self.close_generation.fetch_add(1, SeqCst) + 1
    }

    /// Clear the close fence only while no newer fence was installed (the
    /// TS `record.closing === closing` identity guard — `Shared` exposes no
    /// public pointer, so identity rides the generation counter).
    fn clear_closing_if_current(&self, generation: u64) {
        if self.close_generation.load(SeqCst) == generation {
            *self.closing.lock() = None;
        }
    }
}

/// One unpublished in-flight spawn (TS `PendingSpawn` + `SpawnReservation`).
pub struct PendingSpawn {
    pub owner_key: usize,
    aborted: AtomicBool,
    abort_error: Mutex<Option<TerminalError>>,
    settled: AtomicBool,
    notify: Arc<Notify>,
    cleanup_failure: Mutex<Option<TerminalFailure>>,
}

impl PendingSpawn {
    /// Fire the reservation's abort (TS `controller.abort(reason)`); the
    /// settlement promise resolves only in `release`.
    pub fn abort(&self, reason: TerminalError) {
        *self.abort_error.lock() = Some(reason);
        self.aborted.store(true, SeqCst);
    }

    pub fn aborted(&self) -> bool {
        self.aborted.load(SeqCst)
    }
}

/// Per-owner registry state (TS `reservedNames` + `ownerCleanups` collapsed).
struct OwnerState {
    #[allow(dead_code)]
    agent: Arc<dyn Agent>,
    reserved_names: HashSet<String>,
    disposer: Option<Disposer>,
}

/// Name reservation guard (TS `releaseName`).
struct NameRelease {
    service: Arc<TerminalSessionService>,
    key: usize,
    name: String,
}

impl NameRelease {
    fn release(&self) {
        let mut owners = self.service.owners.lock();
        if let Some(state) = owners.get_mut(&self.key) {
            state.reserved_names.remove(&self.name);
        }
    }
}

/// In-process registry for replaceable PTY backends and exact-Agent sessions
/// (TS `TerminalSessionService`).
pub struct TerminalSessionService {
    ctx: Context,
    backends: Arc<Mutex<Vec<(String, Arc<dyn TerminalBackend>)>>>,
    sessions: Mutex<HashMap<TerminalSessionId, Arc<SessionRecord>>>,
    owners: Mutex<HashMap<usize, OwnerState>>,
    pending: Mutex<HashMap<usize, Vec<Arc<PendingSpawn>>>>,
    disposed: Mutex<HashSet<usize>>,
    next_id: AtomicU64,
    disposing: AtomicBool,
}

fn coded(message: impl Into<String>, code: TerminalErrorCode) -> TerminalFailure {
    TerminalFailure::Coded(TerminalError::new(message, code))
}

fn service_disposing() -> TerminalError {
    TerminalError::new("PTY service is disposing", TerminalErrorCode::ServiceDisposing)
}

fn owner_not_live(owner: &Arc<dyn Agent>) -> TerminalError {
    TerminalError::new(
        format!("agent \"{}\" is not the registered PTY owner", owner.id()),
        TerminalErrorCode::OwnerNotLive,
    )
}

impl TerminalSessionService {
    /// Construct, register as `ctx.terminals`, and attach the teardown effect
    /// (the TS constructor + `super(ctx, 'terminals')` collapse).
    pub fn install(ctx: &Context) -> Arc<Self> {
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            backends: Arc::new(Mutex::new(Vec::new())),
            sessions: Mutex::new(HashMap::new()),
            owners: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            disposed: Mutex::new(HashSet::new()),
            next_id: AtomicU64::new(0),
            disposing: AtomicBool::new(false),
        });
        let teardown = service.clone();
        let _ = ctx.effect(
            "pty teardown",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let teardown = teardown.clone();
                    Box::pin(async move {
                        let _ = teardown.dispose_all().await;
                    })
                }))
            }),
        );
        ctx.register_service(service.clone());
        service
    }

    fn assert_active(&self) -> Result<(), TerminalFailure> {
        if self.disposing.load(SeqCst) {
            Err(TerminalFailure::Coded(service_disposing()))
        } else {
            Ok(())
        }
    }

    fn is_live_owner(&self, owner: &Arc<dyn Agent>) -> bool {
        let key = owner_key(owner);
        if self.disposed.lock().contains(&key) {
            return false;
        }
        let Some(registry) = self
            .ctx
            .get_typed::<Arc<AgentRegistry>>("agents", false)
            .map(|slot| slot.as_ref().clone())
        else {
            return false;
        };
        registry
            .get(owner.id())
            .is_some_and(|registered| Arc::ptr_eq(&registered, owner))
    }

    fn ensure_owner_cleanup(
        self: &Arc<Self>,
        owner: &Arc<dyn Agent>,
    ) -> Result<(), TerminalFailure> {
        if !self.is_live_owner(owner) {
            return Err(TerminalFailure::Coded(owner_not_live(owner)));
        }
        let key = owner_key(owner);
        if self.owners.lock().contains_key(&key) {
            return Ok(());
        }
        let service = self.clone();
        let owned = owner.clone();
        let disposer = owner.ctx().effect(
            "pty.ownerCleanup()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let service = service.clone();
                    let owned = owned.clone();
                    Box::pin(async move {
                        let key = owner_key(&owned);
                        service.disposed.lock().insert(key);
                        service.owners.lock().remove(&key);
                        let _ = service.dispose_owned(&owned).await;
                    })
                }))
            }),
        );
        self.owners.lock().insert(
            key,
            OwnerState {
                agent: owner.clone(),
                reserved_names: HashSet::new(),
                disposer: Some(disposer),
            },
        );
        Ok(())
    }

    fn backend(&self, type_: &str) -> Result<Arc<dyn TerminalBackend>, TerminalFailure> {
        self.backends
            .lock()
            .iter()
            .find(|(candidate, _)| candidate == type_)
            .map(|(_, backend)| backend.clone())
            .ok_or_else(|| {
                coded(
                    format!("no PTY backend registered for \"{type_}\""),
                    TerminalErrorCode::NoBackend,
                )
            })
    }

    fn reserve_name(
        self: &Arc<Self>,
        owner: &Arc<dyn Agent>,
        name: &Option<String>,
    ) -> Result<Option<NameRelease>, TerminalFailure> {
        let Some(name) = name.as_deref() else {
            return Ok(None);
        };
        let key = owner_key(owner);
        {
            let sessions = self.sessions.lock();
            if sessions.values().any(|record| {
                owner_key(&record.owner) == key && record.name.as_deref() == Some(name)
            }) {
                return Err(coded(
                    format!("PTY session name \"{name}\" already exists for this owner"),
                    TerminalErrorCode::DuplicateName,
                ));
            }
        }
        let mut owners = self.owners.lock();
        let state = owners
            .get_mut(&key)
            .expect("owner cleanup is registered before name reservation");
        if state.reserved_names.contains(name) {
            return Err(coded(
                format!("PTY session name \"{name}\" is already being created"),
                TerminalErrorCode::DuplicateName,
            ));
        }
        state.reserved_names.insert(name.to_string());
        Ok(Some(NameRelease {
            service: self.clone(),
            key,
            name: name.to_string(),
        }))
    }

    fn reserve_spawn(self: &Arc<Self>, owner: &Arc<dyn Agent>) -> Arc<PendingSpawn> {
        let pending = Arc::new(PendingSpawn {
            owner_key: owner_key(owner),
            aborted: AtomicBool::new(false),
            abort_error: Mutex::new(None),
            settled: AtomicBool::new(false),
            notify: Arc::new(Notify::new()),
            cleanup_failure: Mutex::new(None),
        });
        self.pending
            .lock()
            .entry(pending.owner_key)
            .or_default()
            .push(pending.clone());
        pending
    }

    fn release_spawn(
        &self,
        pending: &Arc<PendingSpawn>,
        cleanup_failure: Option<TerminalFailure>,
    ) {
        if let Some(failure) = cleanup_failure {
            *pending.cleanup_failure.lock() = Some(failure);
        } else {
            self.remove_pending(pending);
        }
        pending.settled.store(true, SeqCst);
        pending.notify.notify_one();
    }

    fn remove_pending(&self, pending: &Arc<PendingSpawn>) {
        let mut map = self.pending.lock();
        let Some(list) = map.get_mut(&pending.owner_key) else {
            return;
        };
        list.retain(|candidate| !Arc::ptr_eq(candidate, pending));
        if list.is_empty() {
            map.remove(&pending.owner_key);
        }
    }

    fn snapshot(&self, record: &Arc<SessionRecord>, motd: bool) -> TerminalSpawnResult {
        TerminalSpawnResult {
            session_id: record.id.clone(),
            name: record.name.clone(),
            type_: record.type_.clone(),
            pid: record.session.pid(),
            status: record.session.status(),
            motd: if motd { record.session.motd() } else { String::new() },
        }
    }

    fn expect_owned(
        &self,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
    ) -> Result<Arc<SessionRecord>, TerminalFailure> {
        let record = self.sessions.lock().get(id).cloned();
        let Some(record) = record else {
            return Err(coded(
                format!("unknown PTY session {id}"),
                TerminalErrorCode::NoSession,
            ));
        };
        if !Arc::ptr_eq(&record.owner, owner) {
            return Err(coded(
                format!("PTY session {id} belongs to another agent"),
                TerminalErrorCode::ForeignSession,
            ));
        }
        Ok(record)
    }

    // ---- public surface ----

    /// Register one backend type for this effect scope (TS
    /// `registerBackend`). Returns a disposer that removes exactly this
    /// contribution.
    pub fn register_backend(
        self: &Arc<Self>,
        backend: Arc<dyn TerminalBackend>,
    ) -> Result<Disposer, TerminalFailure> {
        if backend.type_().is_empty() {
            return Err(TerminalFailure::Plain(
                "pty backend type must be non-empty".to_string(),
            ));
        }
        {
            let backends = self.backends.lock();
            if backends.iter().any(|(type_, _)| type_ == &backend.type_()) {
                return Err(coded(
                    format!("a PTY backend named \"{}\" is already registered", backend.type_()),
                    TerminalErrorCode::DuplicateBackend,
                ));
            }
        }
        // The TS set rides the effect setup (observable immediately); the
        // Rust effect executes asynchronously, so publish here and let the
        // disposer remove exactly this contribution.
        self.backends.lock().push((backend.type_(), backend.clone()));
        let backends = self.backends.clone();
        let disposer = self.ctx.effect(
            "pty.registerBackend()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let backends = backends.clone();
                    let backend = backend.clone();
                    Box::pin(async move {
                        let mut list = backends.lock();
                        if let Some(index) = list
                            .iter()
                            .position(|(type_, registered)| {
                                type_ == &backend.type_() && Arc::ptr_eq(registered, &backend)
                            })
                        {
                            list.remove(index);
                        }
                    })
                }))
            }),
        );
        Ok(disposer)
    }

    /// List registered backend types in registration order (TS
    /// `listBackends`).
    pub fn list_backends(&self) -> Vec<String> {
        self.backends.lock().iter().map(|(type_, _)| type_.clone()).collect()
    }

    /// Create and publish one owner-scoped session after backend setup
    /// succeeds. The TS synchronous prefix (activity fence, owner cleanup,
    /// backend lookup, name reservation, spawn reservation) runs at the call;
    /// the returned future carries backend setup, rollback, and publication.
    pub fn spawn(
        self: &Arc<Self>,
        owner: Arc<dyn Agent>,
        request: TerminalSpawnRequest,
        signal: Option<TerminalAbort>,
    ) -> Result<BoxFuture<'static, Result<TerminalSpawnResult, TerminalFailure>>, TerminalFailure>
    {
        self.assert_active()?;
        if signal.as_ref().is_some_and(|signal| signal()) {
            return Err(TerminalFailure::Aborted);
        }
        self.ensure_owner_cleanup(&owner)?;
        let backend = self.backend(&request.type_)?;
        if request.name.as_deref() == Some("") {
            return Err(TerminalFailure::Plain(
                "PTY session name must be non-empty".to_string(),
            ));
        }
        let release_name = self.reserve_name(&owner, &request.name)?;
        let reservation = self.reserve_spawn(&owner);
        let backend_signal: TerminalAbort = {
            let caller = signal.clone();
            let reservation = reservation.clone();
            Arc::new(move || {
                caller.as_ref().is_some_and(|abort| abort())
                    || reservation.aborted.load(SeqCst)
            })
        };
        let session_id = terminal_session_id(format!(
            "pty-{}",
            self.next_id.fetch_add(1, SeqCst) + 1
        ));
        let service = self.clone();
        Ok(Box::pin(async move {
            let spec = TerminalBackendSpawnSpec {
                session_id: session_id.clone(),
                owner: owner.clone(),
                type_: request.type_.clone(),
                name: request.name.clone(),
                cwd: request.cwd.clone(),
                signal: Some(backend_signal),
            };
            let mut session: Option<Arc<dyn TerminalBackendSession>> = None;
            let mut cleanup_failure: Option<TerminalFailure> = None;
            let mut failure: Option<TerminalFailure> = None;
            match backend.spawn(spec).await {
                Ok(created) => {
                    session = Some(created);
                    // Post-setup gates (TS order: caller signal → disposing →
                    // owner live).
                    if signal.as_ref().is_some_and(|signal| signal()) {
                        failure = Some(TerminalFailure::Aborted);
                    } else if service.disposing.load(SeqCst) {
                        failure = Some(TerminalFailure::Coded(service_disposing()));
                    } else if !service.is_live_owner(&owner) {
                        failure = Some(TerminalFailure::Coded(owner_not_live(&owner)));
                    }
                }
                Err(error) => {
                    cleanup_failure = error.cleanup_error.map(TerminalFailure::Plain);
                    failure = Some(TerminalFailure::Plain(error.spawn_error));
                }
            }
            let Some(failure) = failure else {
                // Publish after setup succeeds.
                let record = Arc::new(SessionRecord {
                    id: session_id.clone(),
                    owner: owner.clone(),
                    name: request.name.clone(),
                    type_: request.type_.clone(),
                    session: session.expect("successful spawn owns its session"),
                    active: Mutex::new(None),
                    closing: Mutex::new(None),
                    close_generation: AtomicU64::new(0),
                });
                service.sessions.lock().insert(session_id.clone(), record.clone());
                let result = service.snapshot(&record, true);
                service.release_spawn(&reservation, None);
                if let Some(release_name) = release_name {
                    release_name.release();
                }
                return Ok(result);
            };

            // Roll back an unpublished session.
            let mut rollback_failure: Option<TerminalFailure> = None;
            if let Some(created) = &session {
                if !service.sessions.lock().contains_key(&session_id) {
                    if let Err(close_error) = created.close("PTY spawn rolled back").await {
                        rollback_failure = Some(TerminalFailure::Plain(close_error));
                        cleanup_failure = rollback_failure.clone();
                    }
                }
            }
            // Cancellation overrides the failure (TS: caller signal first,
            // then the reservation).
            let mut failure = failure;
            if signal.as_ref().is_some_and(|signal| signal()) {
                failure = TerminalFailure::Aborted;
            } else if reservation.aborted.load(SeqCst) {
                if let Some(reason) = reservation.abort_error.lock().clone() {
                    failure = TerminalFailure::Coded(reason);
                }
            }
            let failure = if rollback_failure.is_some()
                && !signal.as_ref().is_some_and(|signal| signal())
            {
                TerminalFailure::Aggregate {
                    message: "PTY spawn and rollback both failed".to_string(),
                    failures: vec![failure, rollback_failure.expect("rollback failure set")],
                }
            } else {
                failure
            };
            // The release mirrors the TS `finally`: it must run on EVERY
            // failure path, including the aggregate (an early return here
            // would leave the reservation unsettled and hang disposal).
            service.release_spawn(&reservation, cleanup_failure);
            if let Some(release_name) = release_name {
                release_name.release();
            }
            Err(failure)
        }))
    }

    /// Test whether an exact owner has a published session or unpublished
    /// spawn (TS `hasOwnerActivity`).
    pub fn has_owner_activity(&self, owner: &Arc<dyn Agent>) -> bool {
        let key = owner_key(owner);
        let has_pending = self
            .pending
            .lock()
            .get(&key)
            .is_some_and(|list| !list.is_empty());
        let has_session = self
            .sessions
            .lock()
            .values()
            .any(|record| Arc::ptr_eq(&record.owner, owner));
        has_pending || has_session
    }

    /// Start one exclusive interactive send (TS `startSend`).
    pub fn start_send(
        self: &Arc<Self>,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        request: TerminalSendRequest,
    ) -> Result<Arc<dyn TerminalSendOperation>, TerminalFailure> {
        let record = self.expect_owned(owner, id)?;
        if record.closing.lock().is_some() {
            return Err(TerminalFailure::Plain(format!("PTY session {id} is closing")));
        }
        if record.active.lock().is_some() {
            return Err(coded(
                format!("PTY session {id} already has an active send"),
                TerminalErrorCode::SendActive,
            ));
        }
        let operation = record.session.start_send(&request);
        *record.active.lock() = Some(operation.clone());
        let record = record.clone();
        let settled_op = operation.clone();
        tokio::spawn(async move {
            // Clear on BOTH settlement and rejection (the TS `.then` pair).
            let _ = std::panic::AssertUnwindSafe(async { settled_op.done().await })
                .catch_unwind()
                .await;
            *record.active.lock() = None;
        });
        Ok(operation)
    }

    /// Read one bounded scrollback page from an owned session (TS `read`).
    pub fn read(
        &self,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        request: TerminalReadRequest,
    ) -> Result<TerminalReadResult, TerminalFailure> {
        Ok(self.expect_owned(owner, id)?.session.read(&request))
    }

    /// Deliver an allowed signal through an owned backend session (TS
    /// `signal`). The synchronous ownership fence surfaces at the call; the
    /// backend delivery rides the returned future.
    pub fn signal(
        self: &Arc<Self>,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        signal: TerminalSignal,
    ) -> Result<BoxFuture<'static, Result<TerminalSignalResult, TerminalFailure>>, TerminalFailure>
    {
        let session = self.expect_owned(owner, id)?.session.clone();
        Ok(Box::pin(async move {
            session
                .signal(signal)
                .await
                .map_err(TerminalFailure::Plain)
        }))
    }

    /// Close one owned session and remove it only after quiescent backend
    /// cleanup. The TS synchronous prefix (ownership fence + closing
    /// publication) runs at the call; the returned future awaits closure.
    pub fn kill(
        self: &Arc<Self>,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        reason: String,
    ) -> Result<BoxFuture<'static, Result<bool, TerminalFailure>>, TerminalFailure> {
        let record = self.expect_owned(owner, id)?;
        // Take the fence OUTSIDE the match: a scrutinee temporary guard
        // would stay held across the `None` arm and deadlock
        // `install_closing`'s re-lock (parking_lot is not reentrant).
        let existing = { record.closing.lock().clone() };
        let closing = match existing {
            Some(existing) => {
                // Join the already-running close; the outcome is `false`.
                return Ok(Box::pin(async move {
                    existing
                        .await
                        .map(|()| false)
                        .map_err(TerminalFailure::Plain)
                }));
            }
            None => {
                let fence = record.session.close(&reason).boxed().shared();
                let generation = record.install_closing(fence.clone());
                (fence, generation)
            }
        };
        let service = self.clone();
        let record = record.clone();
        let id = id.clone();
        Ok(Box::pin(async move {
            let (closing, generation) = closing;
            match closing.await {
                Ok(()) => {
                    service.sessions.lock().remove(&id);
                    Ok(true)
                }
                Err(error) => {
                    // A concurrent retry may already own a newer fence; never
                    // clear it.
                    record.clear_closing_if_current(generation);
                    Err(TerminalFailure::Plain(error))
                }
            }
        }))
    }

    /// List fresh snapshots for exactly one owner (TS `list`).
    pub fn list(&self, owner: &Arc<dyn Agent>) -> Vec<TerminalSessionSnapshot> {
        self.sessions
            .lock()
            .values()
            .filter(|record| Arc::ptr_eq(&record.owner, owner))
            .map(|record| TerminalSessionSnapshot {
                session_id: record.id.clone(),
                name: record.name.clone(),
                type_: record.type_.clone(),
                pid: record.session.pid(),
                status: record.session.status(),
            })
            .collect()
    }

    // ---- disposal ----

    /// Synchronously fire the reservation aborts for `target_key` (`None` =
    /// every owner) and snapshot the pending spawns (the TS
    /// `abortPendingSpawns` sync prefix).
    fn abort_pending(
        &self,
        target_key: Option<usize>,
        reason: &TerminalError,
    ) -> Vec<Arc<PendingSpawn>> {
        let pendings: Vec<Arc<PendingSpawn>> = match target_key {
            None => self.pending.lock().values().flatten().cloned().collect(),
            Some(key) => self.pending.lock().get(&key).cloned().unwrap_or_default(),
        };
        for pending in &pendings {
            pending.abort(reason.clone());
        }
        pendings
    }

    /// Await every aborted spawn's settlement and aggregate cleanup failures
    /// (TS `abortPendingSpawns` tail).
    async fn await_pending_cleanup(
        self: &Arc<Self>,
        pendings: Vec<Arc<PendingSpawn>>,
    ) -> Result<(), TerminalFailure> {
        for pending in &pendings {
            pending.notify.notified().await;
        }
        let failures: Vec<TerminalFailure> = pendings
            .iter()
            .filter_map(|pending| pending.cleanup_failure.lock().clone())
            .collect();
        for pending in &pendings {
            self.remove_pending(pending);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(TerminalFailure::Aggregate {
                message: "failed to roll back unpublished PTY setup".to_string(),
                failures,
            })
        }
    }

    /// Close every session of one owner (TS `disposeOwned`). The
    /// synchronous abort of unpublished setup happens at the call, mirroring
    /// the TS async function's sync prefix.
    #[doc(hidden)]
    pub fn dispose_owned(
        self: &Arc<Self>,
        owner: &Arc<dyn Agent>,
    ) -> BoxFuture<'static, Result<(), TerminalFailure>> {
        let key = owner_key(owner);
        let reason = owner_not_live(owner);
        let pendings = self.abort_pending(Some(key), &reason);
        let service = self.clone();
        Box::pin(async move {
            let result = service
                .abort_and_close_after_abort(Some(key), pendings, "PTY owner disposed")
                .await;
            service.owners.lock().remove(&key);
            result
        })
    }

    /// The continuation of `abortAndClose` once the sync abort prefix ran
    /// (shared by the disposal paths).
    async fn abort_and_close_after_abort(
        self: &Arc<Self>,
        target_key: Option<usize>,
        pendings: Vec<Arc<PendingSpawn>>,
        close_reason: &str,
    ) -> Result<(), TerminalFailure> {
        let mut failures: Vec<TerminalFailure> = Vec::new();
        if let Err(error) = self.await_pending_cleanup(pendings).await {
            failures.push(error);
        }
        let records: Vec<Arc<SessionRecord>> = {
            let sessions = self.sessions.lock();
            sessions
                .values()
                .filter(|record| match target_key {
                    None => true,
                    Some(key) => owner_key(&record.owner) == key,
                })
                .cloned()
                .collect()
        };
        if let Err(error) = self.close_records(records, close_reason.to_string()).await {
            failures.push(error);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(TerminalFailure::Aggregate {
                message: "failed to clean up PTY lifecycle".to_string(),
                failures,
            })
        }
    }

    /// Dispose the whole service. The `disposing` flag and the unpublished
    /// aborts fire at the call (the TS sync prefix); the future awaits the
    /// teardown chain and always clears the registries.
    #[doc(hidden)]
    pub fn dispose_all(self: &Arc<Self>) -> BoxFuture<'static, Result<(), TerminalFailure>> {
        self.disposing.store(true, SeqCst);
        let reason = service_disposing();
        let pendings = self.abort_pending(None, &reason);
        let service = self.clone();
        Box::pin(async move {
            let result = service
                .abort_and_close_after_abort(None, pendings, "PTY service disposed")
                .await;
            // Teardown is best-effort: a close failure still clears
            // registries and runs owner cleanups before the aggregated error
            // propagates, so one stuck session cannot orphan backends,
            // reservations, or owner detachers.
            let cleanups: Vec<Disposer> = {
                let mut owners = service.owners.lock();
                owners
                    .drain()
                    .filter_map(|(_, state)| state.disposer)
                    .collect()
            };
            service.backends.lock().clear();
            service.pending.lock().clear();
            for cleanup in cleanups {
                (cleanup)().await;
            }
            result
        })
    }

    /// Close a batch of records with all-settled aggregation (TS
    /// `closeRecords`). The TS map callback installs every close fence
    /// synchronously at the call, so the fence installation rides the call
    /// (the sync prefix) and only the awaits ride the returned future.
    #[doc(hidden)]
    pub fn close_records(
        self: &Arc<Self>,
        records: Vec<Arc<SessionRecord>>,
        reason: String,
    ) -> BoxFuture<'static, Result<(), TerminalFailure>> {
        let installs: Vec<(Arc<SessionRecord>, Shared<BoxFuture<'static, Result<(), String>>>, u64)> =
            records
                .iter()
                .map(|record| {
                    // Take the fence OUTSIDE the match (guard-lifetime
                    // deadlock on `install_closing`'s re-lock — see `kill`).
                    let existing = { record.closing.lock().clone() };
                    match existing {
                        Some(fence) => {
                            (record.clone(), fence, record.close_generation.load(SeqCst))
                        }
                        None => {
                            let fence = record.session.close(&reason).boxed().shared();
                            let generation = record.install_closing(fence.clone());
                            (record.clone(), fence, generation)
                        }
                    }
                })
                .collect();
        let service = self.clone();
        Box::pin(async move {
            let futures: Vec<_> = installs
                .into_iter()
                .map(|(record, fence, generation)| {
                    let service = service.clone();
                    async move {
                        match fence.await {
                            Ok(()) => {
                                service.sessions.lock().remove(&record.id);
                                Ok(())
                            }
                            Err(error) => {
                                // A concurrent retry may already own a newer
                                // fence; never clear it.
                                record.clear_closing_if_current(generation);
                                Err(TerminalFailure::Plain(error))
                            }
                        }
                    }
                })
                .collect();
            let results = futures::future::join_all(futures).await;
            let failures: Vec<TerminalFailure> =
                results.into_iter().filter_map(Result::err).collect();
            if failures.is_empty() {
                Ok(())
            } else {
                Err(TerminalFailure::Aggregate {
                    message: format!("failed to close {} PTY session(s)", failures.len()),
                    failures,
                })
            }
        })
    }

    // ---- test seams (the TS suite reaches through `as unknown as …`) ----

    /// The registered backend list (the TS `backends` map).
    #[doc(hidden)]
    pub fn backends(&self) -> &Mutex<Vec<(String, Arc<dyn TerminalBackend>)>> {
        &self.backends
    }

    /// Mark an exact owner as disposed (the TS `disposedOwners` weak set).
    #[doc(hidden)]
    pub fn mark_owner_disposed(&self, owner: &Arc<dyn Agent>) {
        self.disposed.lock().insert(owner_key(owner));
    }

    /// Live session records (the TS `sessions` map values).
    #[doc(hidden)]
    pub fn session_records(&self) -> Vec<Arc<SessionRecord>> {
        self.sessions.lock().values().cloned().collect()
    }

    /// Registered backend count (the TS `backends` map size).
    #[doc(hidden)]
    pub fn backends_len(&self) -> usize {
        self.backends.lock().len()
    }

    /// Owner-cleanup registration count (the TS `ownerCleanups` map size).
    #[doc(hidden)]
    pub fn owner_cleanup_len(&self) -> usize {
        self.owners.lock().len()
    }

    /// Whether any pending spawn of the owner observed its abort (the TS
    /// `backendSignal.aborted` observation).
    #[doc(hidden)]
    pub fn pending_aborted(&self, owner: &Arc<dyn Agent>) -> bool {
        let key = owner_key(owner);
        self.pending
            .lock()
            .get(&key)
            .is_some_and(|list| list.iter().any(|pending| pending.aborted()))
    }

    /// The abort reason carried by the owner's pending spawn (the TS
    /// `backendSignal.reason` observation).
    #[doc(hidden)]
    pub fn pending_abort_error(&self, owner: &Arc<dyn Agent>) -> Option<TerminalError> {
        let key = owner_key(owner);
        self.pending
            .lock()
            .get(&key)
            .and_then(|list| list.iter().find_map(|pending| pending.abort_error.lock().clone()))
    }
}

impl Service for TerminalSessionService {
    fn service_name(&self) -> &'static str {
        "terminals"
    }
}
