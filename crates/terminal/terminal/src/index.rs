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
    /// Serializes the synchronous count-to-reservation handoff for all spawn
    /// callers. Generic callers are not limited, but participate in the gate
    /// so a limited GUI admission cannot race their pending publication.
    spawn_admission: Mutex<()>,
    disposed: Mutex<HashSet<usize>>,
    next_id: AtomicU64,
    disposing: AtomicBool,
}

fn coded(message: impl Into<String>, code: TerminalErrorCode) -> TerminalFailure {
    TerminalFailure::Coded(TerminalError::new(message, code))
}

fn service_disposing() -> TerminalError {
    TerminalError::new(
        "PTY service is disposing",
        TerminalErrorCode::ServiceDisposing,
    )
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
            spawn_admission: Mutex::new(()),
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

    fn release_spawn(&self, pending: &Arc<PendingSpawn>, cleanup_failure: Option<TerminalFailure>) {
        if let Some(failure) = cleanup_failure {
            *pending.cleanup_failure.lock() = Some(failure);
        }
        // Disposal that already snapshotted this reservation retains its Arc
        // and will observe the cleanup failure after settlement. Keeping a
        // settled failure in the live pending map would permanently pin the
        // owner because there is no cleanup operation left to retry.
        self.remove_pending(pending);
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

    fn owner_spawn_count(&self, owner: &Arc<dyn Agent>) -> usize {
        let key = owner_key(owner);
        let published = self
            .sessions
            .lock()
            .values()
            .filter(|record| Arc::ptr_eq(&record.owner, owner))
            .count();
        let pending = self.pending.lock().get(&key).map(Vec::len).unwrap_or(0);
        published.saturating_add(pending)
    }

    fn snapshot(&self, record: &Arc<SessionRecord>, motd: bool) -> TerminalSpawnResult {
        TerminalSpawnResult {
            session_id: record.id.clone(),
            name: record.name.clone(),
            type_: record.type_.clone(),
            pid: record.session.pid(),
            status: record.session.status(),
            motd: if motd {
                record.session.motd()
            } else {
                String::new()
            },
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
                    format!(
                        "a PTY backend named \"{}\" is already registered",
                        backend.type_()
                    ),
                    TerminalErrorCode::DuplicateBackend,
                ));
            }
        }
        // The TS set rides the effect setup (observable immediately); the
        // Rust effect executes asynchronously, so publish here and let the
        // disposer remove exactly this contribution.
        self.backends
            .lock()
            .push((backend.type_(), backend.clone()));
        let backends = self.backends.clone();
        let disposer = self.ctx.effect(
            "pty.registerBackend()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let backends = backends.clone();
                    let backend = backend.clone();
                    Box::pin(async move {
                        let mut list = backends.lock();
                        if let Some(index) = list.iter().position(|(type_, registered)| {
                            type_ == &backend.type_() && Arc::ptr_eq(registered, &backend)
                        }) {
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
        self.backends
            .lock()
            .iter()
            .map(|(type_, _)| type_.clone())
            .collect()
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
        let admission = self.spawn_admission.lock();
        self.spawn_unlocked(owner, request, signal, admission)
    }

    /// Create a PTY under an atomic exact-owner limit. The generic
    /// [`Self::spawn`] surface remains unlimited; both paths share the same
    /// count-to-pending gate so they cannot race the limited admission.
    pub fn spawn_limited(
        self: &Arc<Self>,
        owner: Arc<dyn Agent>,
        request: TerminalSpawnRequest,
        signal: Option<TerminalAbort>,
        max_per_owner: usize,
    ) -> Result<BoxFuture<'static, Result<TerminalSpawnResult, TerminalFailure>>, TerminalFailure>
    {
        let admission = self.spawn_admission.lock();
        if max_per_owner == 0 || self.owner_spawn_count(&owner) >= max_per_owner {
            return Err(coded(
                format!("PTY session limit reached for this owner (limit: {max_per_owner})"),
                TerminalErrorCode::SessionLimit,
            ));
        }
        self.spawn_unlocked(owner, request, signal, admission)
    }

    fn spawn_unlocked(
        self: &Arc<Self>,
        owner: Arc<dyn Agent>,
        request: TerminalSpawnRequest,
        signal: Option<TerminalAbort>,
        admission: parking_lot::MutexGuard<'_, ()>,
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
        drop(admission);
        let backend_signal: TerminalAbort = {
            let caller = signal.clone();
            let reservation = reservation.clone();
            Arc::new(move || {
                caller.as_ref().is_some_and(|abort| abort()) || reservation.aborted.load(SeqCst)
            })
        };
        let session_id =
            terminal_session_id(format!("pty-{}", self.next_id.fetch_add(1, SeqCst) + 1));
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
                    failure = Some(match error.code {
                        Some(TerminalErrorCode::Aborted) => TerminalFailure::Aborted,
                        Some(code) => {
                            TerminalFailure::Coded(TerminalError::new(error.spawn_error, code))
                        }
                        None => TerminalFailure::Plain(error.spawn_error),
                    });
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
                {
                    let _admission = service.spawn_admission.lock();
                    service
                        .sessions
                        .lock()
                        .insert(session_id.clone(), record.clone());
                    service.release_spawn(&reservation, None);
                }
                let result = service.snapshot(&record, true);
                if let Some(release_name) = release_name {
                    release_name.release();
                }
                return Ok(result);
            };

            // Roll back an unpublished session.
            let mut rollback_failure = cleanup_failure.clone();
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
            let failure =
                if rollback_failure.is_some() && !signal.as_ref().is_some_and(|signal| signal()) {
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
            service.notify_owner_idle(&owner);
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

    fn notify_owner_idle(&self, owner: &Arc<dyn Agent>) {
        if !self.disposing.load(SeqCst)
            && !self.disposed.lock().contains(&owner_key(owner))
            && !self.has_owner_activity(owner)
        {
            self.ctx
                .emit("terminal/owner-idle", vec![cordis::arc(owner.clone())]);
        }
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
            return Err(TerminalFailure::Plain(format!(
                "PTY session {id} is closing"
            )));
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

    /// Write raw interactive input to one owned session. Unlike
    /// [`Self::start_send`], this does not wait for an inferred command
    /// boundary and can therefore carry individual key sequences.
    pub fn write_input(
        &self,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        data: &str,
    ) -> Result<BoxFuture<'static, Result<(), TerminalFailure>>, TerminalFailure> {
        let record = self.expect_owned(owner, id)?;
        if record.closing.lock().is_some() {
            return Err(TerminalFailure::Plain(format!(
                "PTY session {id} is closing"
            )));
        }
        let future = record.session.write_input(data);
        Ok(Box::pin(async move {
            future.await.map_err(TerminalFailure::Plain)
        }))
    }

    /// Resize one owned native PTY.
    pub fn resize(
        &self,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        rows: u16,
        cols: u16,
    ) -> Result<BoxFuture<'static, Result<(), TerminalFailure>>, TerminalFailure> {
        let record = self.expect_owned(owner, id)?;
        if record.closing.lock().is_some() {
            return Err(TerminalFailure::Plain(format!(
                "PTY session {id} is closing"
            )));
        }
        let future = record.session.resize(rows, cols);
        Ok(Box::pin(async move {
            future.await.map_err(TerminalFailure::Plain)
        }))
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
            session.signal(signal).await.map_err(TerminalFailure::Plain)
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
                    service.notify_owner_idle(&record.owner);
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
        let installs: Vec<(
            Arc<SessionRecord>,
            Shared<BoxFuture<'static, Result<(), String>>>,
            u64,
        )> = records
            .iter()
            .map(|record| {
                // Take the fence OUTSIDE the match (guard-lifetime
                // deadlock on `install_closing`'s re-lock — see `kill`).
                let existing = { record.closing.lock().clone() };
                match existing {
                    Some(fence) => (record.clone(), fence, record.close_generation.load(SeqCst)),
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
        self.pending.lock().get(&key).and_then(|list| {
            list.iter()
                .find_map(|pending| pending.abort_error.lock().clone())
        })
    }
}

impl Service for TerminalSessionService {
    fn service_name(&self) -> &'static str {
        "terminals"
    }
}

#[cfg(test)]
mod limit_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    use cordis::Context;
    use dsh_agent::{
        Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
        InboxNotifications, InboxTarget,
    };
    use dsh_scope::ScopeKey;
    use dsh_session::{Session, SessionStore, UserMessage, session_id};
    use futures::future::BoxFuture;

    use crate::types::{
        TerminalBackend, TerminalBackendSession, TerminalBackendSpawnError,
        TerminalBackendSpawnSpec, TerminalErrorCode, TerminalReadRequest, TerminalReadResult,
        TerminalSendOperation, TerminalSendRequest, TerminalSessionStatus, TerminalSignal,
        TerminalSignalResult, TerminalSpawnRequest,
    };

    use super::{TerminalSessionService, owner_key};

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

    async fn agent(ctx: &Context) -> Arc<dyn Agent> {
        let sessions = SessionStore::install(ctx);
        let id = session_id("terminal-limit-owner");
        let session = sessions
            .create(ctx, Some(id.clone()), None)
            .await
            .expect("create owner session");
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

    struct FakeSession {
        closed: Arc<AtomicUsize>,
    }

    impl TerminalBackendSession for FakeSession {
        fn motd(&self) -> String {
            String::new()
        }
        fn pid(&self) -> Option<u32> {
            None
        }
        fn start_send(&self, _request: &TerminalSendRequest) -> Arc<dyn TerminalSendOperation> {
            panic!("send is outside the admission fixture")
        }
        fn write_input(&self, _data: &str) -> BoxFuture<'static, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn resize(&self, _rows: u16, _cols: u16) -> BoxFuture<'static, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn read(&self, _request: &TerminalReadRequest) -> TerminalReadResult {
            TerminalReadResult {
                text: String::new(),
                total_lines: 0,
                line_begin: 0,
                line_end: 0,
                truncated: false,
            }
        }
        fn signal(
            &self,
            _signal: TerminalSignal,
        ) -> BoxFuture<'static, Result<TerminalSignalResult, String>> {
            Box::pin(async {
                Ok(TerminalSignalResult {
                    delivered: true,
                    target_pgid: 0,
                })
            })
        }
        fn status(&self) -> TerminalSessionStatus {
            TerminalSessionStatus::Running
        }
        fn close(&self, _reason: &str) -> BoxFuture<'static, Result<(), String>> {
            let closed = self.closed.clone();
            Box::pin(async move {
                closed.fetch_add(1, SeqCst);
                Ok(())
            })
        }
    }

    struct FakeBackend {
        spawned: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
    }

    impl TerminalBackend for FakeBackend {
        fn type_(&self) -> String {
            "limit-fixture".to_string()
        }
        fn spawn(
            &self,
            _spec: TerminalBackendSpawnSpec,
        ) -> BoxFuture<'static, Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>>
        {
            let spawned = self.spawned.clone();
            let closed = self.closed.clone();
            Box::pin(async move {
                spawned.fetch_add(1, SeqCst);
                Ok(Arc::new(FakeSession { closed }) as Arc<dyn TerminalBackendSession>)
            })
        }
    }

    struct FailingBackend;

    impl TerminalBackend for FailingBackend {
        fn type_(&self) -> String {
            "failure-fixture".to_string()
        }
        fn spawn(
            &self,
            _spec: TerminalBackendSpawnSpec,
        ) -> BoxFuture<'static, Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>>
        {
            Box::pin(async {
                Err(TerminalBackendSpawnError::cleanup_failed(
                    "fixture setup failed",
                    "fixture cleanup failed",
                ))
            })
        }
    }

    fn request() -> TerminalSpawnRequest {
        TerminalSpawnRequest {
            type_: "limit-fixture".to_string(),
            name: None,
            cwd: None,
        }
    }

    fn failing_request() -> TerminalSpawnRequest {
        TerminalSpawnRequest {
            type_: "failure-fixture".to_string(),
            name: None,
            cwd: None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn limited_spawn_atomically_counts_pending_without_limiting_generic_callers() {
        let ctx = Context::root();
        let agents = AgentRegistry::install(&ctx);
        let owner = agent(&ctx).await;
        let _owner_entry = agents.enter(owner.clone(), None).expect("register owner");
        let service = TerminalSessionService::install(&ctx);
        let spawned = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let _backend = service
            .register_backend(Arc::new(FakeBackend {
                spawned: spawned.clone(),
                closed: closed.clone(),
            }))
            .expect("register fixture backend");
        let _failing_backend = service
            .register_backend(Arc::new(FailingBackend))
            .expect("register failing backend");

        let barrier = Arc::new(tokio::sync::Barrier::new(9));
        let mut calls = Vec::new();
        for _ in 0..8 {
            let service = service.clone();
            let owner = owner.clone();
            let barrier = barrier.clone();
            calls.push(tokio::spawn(async move {
                barrier.wait().await;
                service.spawn_limited(owner, request(), None, 3)
            }));
        }
        barrier.wait().await;
        let mut accepted = Vec::new();
        let mut rejected = 0;
        for call in calls {
            match call.await.expect("join admission caller") {
                Ok(future) => accepted.push(future),
                Err(failure) => {
                    assert_eq!(failure.code(), Some(TerminalErrorCode::SessionLimit));
                    rejected += 1;
                }
            }
        }
        assert_eq!(accepted.len(), 3);
        assert_eq!(rejected, 5);
        assert_eq!(
            service.pending.lock().get(&owner_key(&owner)).map(Vec::len),
            Some(3)
        );

        let generic = service
            .spawn(owner.clone(), request(), None)
            .expect("generic model-facing spawn remains unlimited");
        accepted.push(generic);
        for future in accepted {
            future.await.expect("publish admitted session");
        }
        assert_eq!(spawned.load(SeqCst), 4);
        assert_eq!(service.list(&owner).len(), 4);
        assert!(!service.pending.lock().contains_key(&owner_key(&owner)));

        service
            .close_records(
                service.session_records(),
                "quota fixture cleanup".to_string(),
            )
            .await
            .expect("close all fixture sessions");
        assert!(service.list(&owner).is_empty());
        assert!(!service.has_owner_activity(&owner));
        assert_eq!(closed.load(SeqCst), 4);

        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal: crate::types::TerminalAbort = {
            let cancelled = cancelled.clone();
            Arc::new(move || cancelled.load(SeqCst))
        };
        let cancelled_spawn = service
            .spawn_limited(owner.clone(), request(), Some(signal), 3)
            .expect("reserve cancellable spawn");
        cancelled.store(true, SeqCst);
        let failure = cancelled_spawn
            .await
            .expect_err("post-setup cancellation rolls back publication");
        assert!(matches!(failure, crate::types::TerminalFailure::Aborted));
        assert!(service.list(&owner).is_empty());
        assert!(!service.pending.lock().contains_key(&owner_key(&owner)));
        assert!(!service.has_owner_activity(&owner));
        assert_eq!(closed.load(SeqCst), 5);

        let recovered = service
            .spawn_limited(owner.clone(), request(), None, 3)
            .expect("cancelled admission releases the quota")
            .await
            .expect("publish recovery session");
        service
            .kill(
                &owner,
                &recovered.session_id,
                "fixture complete".to_string(),
            )
            .expect("begin recovery cleanup")
            .await
            .expect("finish recovery cleanup");
        assert!(!service.has_owner_activity(&owner));
        assert_eq!(closed.load(SeqCst), 6);

        let failure = service
            .spawn_limited(owner.clone(), failing_request(), None, 3)
            .expect("failing backend still reserves synchronously")
            .await
            .expect_err("backend failure propagates");
        assert!(matches!(
            failure,
            crate::types::TerminalFailure::Aggregate { .. }
        ));
        assert!(!service.pending.lock().contains_key(&owner_key(&owner)));
        assert!(!service.has_owner_activity(&owner));
    }
}
