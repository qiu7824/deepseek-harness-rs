//! Rust port of the TS `service.spec.ts` suite for `dsh-terminal`: backend
//! registry disposal, owner fencing, spawn publication/rollback, caller
//! cancellation, unpublished-setup aborts through owner and service disposal,
//! and idempotent closes.
//!
//! # Deviations
//!
//! - Rust futures are lazy: the synchronous prefix of `spawn`/`kill`/
//!   `dispose_owned`/`dispose_all` runs at the call (mirroring the TS async
//!   function's sync prefix), and tests drive in-flight spawn futures with
//!   `tokio::spawn` + a `yield_until` helper where the TS suite relies on
//!   sync aborts.
//! - Caller-cancellation reason objects collapse into
//!   [`TerminalFailure::Aborted`]; a rejecting send `done` collapses into a
//!   panic caught with `catch_unwind`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::Duration;

use cordis::{Context, FiberCore, Plugin};
use futures::FutureExt;
use parking_lot::Mutex;
use tokio::sync::{oneshot, watch};

use dsh_agent::{Agent, AgentOptions, AgentRegistry, AgentStatus, Inbox};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, session_id};use dsh_terminal::{
    TerminalAbort, TerminalBackend, TerminalBackendSession, TerminalBackendSpawnError,
    TerminalBackendSpawnSpec, TerminalErrorCode, TerminalFailure, TerminalReadRequest,
    TerminalReadResult, TerminalSendOperation, TerminalSendRequest, TerminalSessionStatus,
    TerminalSignal, TerminalSignalResult, TerminalSpawnRequest, TerminalWaitReason,
    terminal_session_id,
};

/// An inert plugin fiber: gives each stub agent its own scope context (the
/// TS `ctx.plugin(() => {})`).
struct NoopPlugin;

#[async_trait::async_trait]
impl Plugin for NoopPlugin {
    async fn apply(&self, _ctx: &Context, _config: cordis::ArcValue) -> Result<(), cordis::PluginError> {
        Ok(())
    }
}

/// The terminals-service plugin form (the TS `ctx.plugin(TerminalSessionService)`).
struct TerminalsPlugin;

#[async_trait::async_trait]
impl Plugin for TerminalsPlugin {
    async fn apply(&self, ctx: &Context, _config: cordis::ArcValue) -> Result<(), cordis::PluginError> {
        let _ = dsh_terminal::TerminalSessionService::install(ctx);
        Ok(())
    }
}

struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl StubAgent {
    fn new(ctx: &Context, raw_id: &str) -> (Arc<dyn Agent>, Arc<FiberCore>) {
        let fiber = ctx.plugin(Arc::new(NoopPlugin), cordis::arc(()));
        let agent_ctx = fiber.ctx().expect("plugin ctx is bound at load");
        let id = session_id(raw_id);
        let session = Session::create(id.clone(), None, None).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        let agent: Arc<dyn Agent> = Arc::new(Self {
            id,
            session,
            inbox,
            ctx: agent_ctx,
            scope_key: ScopeKey::new(),
        });
        (agent, fiber)
    }
}

impl Agent for StubAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn options(&self) -> &AgentOptions {
        static OPTIONS: std::sync::OnceLock<AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(AgentOptions::default)
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
    }

    fn cancel(&self, _cause: dsh_agent::AgentCancelCause, _options: Option<&dsh_agent::CancelOptions>) {}

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: dsh_session::UserMessage, _target: dsh_agent::InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}

/// A hand-built send operation with a resolvable settlement (the TS
/// `StubSession.startSend` shape).
struct StubSendOperation {
    reject: bool,
    done: futures::future::Shared<futures::future::BoxFuture<'static, dsh_terminal::TerminalSendResult>>,
    settled: AtomicBool,
    cancel_tx: watch::Sender<bool>,
}

impl StubSendOperation {
    fn new(reject: bool, status: Arc<Mutex<TerminalSessionStatus>>) -> Arc<dyn TerminalSendOperation> {
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let done = async move {
            if reject {
                panic!("send failed");
            }
            loop {
                if *cancel_rx.borrow() {
                    break;
                }
                if cancel_rx.changed().await.is_err() {
                    break;
                }
            }
            dsh_terminal::TerminalSendResult {
                viewport: "done".to_string(),
                wait_reason: TerminalWaitReason::StdinRead,
                session_status: status.lock().clone(),
                truncated: false,
            }
        }
        .boxed()
        .shared();
        Arc::new(Self {
            reject,
            done,
            settled: AtomicBool::new(false),
            cancel_tx,
        })
    }
}

impl TerminalSendOperation for StubSendOperation {
    fn done(&self) -> futures::future::BoxFuture<'static, dsh_terminal::TerminalSendResult> {
        self.done.clone().boxed()
    }

    fn read_output(&self) -> dsh_terminal::TerminalSendRead {
        dsh_terminal::TerminalSendRead {
            delta: "delta".to_string(),
            truncated: false,
        }
    }

    fn cancel(&self) -> bool {
        if self.settled.swap(true, SeqCst) {
            return false;
        }
        let _ = self.cancel_tx.send(true);
        true
    }
}

/// The TS `StubSession`. State is Arc'd internally so the `'static` futures
/// of the trait methods never capture a borrow.
pub struct StubSession {
    pub pid_value: Arc<Mutex<Option<u32>>>,
    pub closed: Arc<Mutex<Vec<String>>>,
    pub status_value: Arc<Mutex<TerminalSessionStatus>>,
    pub operation: Arc<Mutex<Option<Arc<dyn TerminalSendOperation>>>>,
    pub reject_send: Arc<AtomicBool>,
    pub reject_close: Arc<AtomicBool>,
    pub close_gate: Arc<Mutex<Option<watch::Sender<bool>>>>,
}

impl Default for StubSession {
    fn default() -> Self {
        Self {
            pid_value: Arc::new(Mutex::new(Some(123))),
            closed: Arc::new(Mutex::new(Vec::new())),
            status_value: Arc::new(Mutex::new(TerminalSessionStatus::Running)),
            operation: Arc::new(Mutex::new(None)),
            reject_send: Arc::new(AtomicBool::new(false)),
            reject_close: Arc::new(AtomicBool::new(false)),
            close_gate: Arc::new(Mutex::new(None)),
        }
    }
}

impl TerminalBackendSession for StubSession {
    fn motd(&self) -> String {
        "stub ready".to_string()
    }

    fn pid(&self) -> Option<u32> {
        *self.pid_value.lock()
    }

    fn start_send(&self, _request: &TerminalSendRequest) -> Arc<dyn TerminalSendOperation> {
        let operation = StubSendOperation::new(self.reject_send.load(SeqCst), self.status_value.clone());
        *self.operation.lock() = Some(operation.clone());
        operation
    }

    fn read(&self, request: &TerminalReadRequest) -> TerminalReadResult {
        TerminalReadResult {
            text: format!("{}:{}", request.offset.unwrap_or(0), request.count.unwrap_or(0)),
            total_lines: 1,
            line_begin: 0,
            line_end: 1,
            truncated: false,
        }
    }

    fn signal(&self, signal: TerminalSignal) -> futures::future::BoxFuture<'static, Result<TerminalSignalResult, String>> {
        Box::pin(async move {
            Ok(TerminalSignalResult {
                delivered: true,
                target_pgid: if signal == TerminalSignal::SigInt { 12 } else { 13 },
            })
        })
    }

    fn status(&self) -> TerminalSessionStatus {
        self.status_value.lock().clone()
    }

    fn close(&self, reason: &str) -> futures::future::BoxFuture<'static, Result<(), String>> {
        let reason = reason.to_string();
        let closed = self.closed.clone();
        let reject_close = self.reject_close.clone();
        let gate = { self.close_gate.lock().clone() };
        let status_value = self.status_value.clone();
        let operation = self.operation.clone();
        Box::pin(async move {
            closed.lock().push(reason);
            if reject_close.load(SeqCst) {
                return Err("close failed".to_string());
            }
            if let Some(gate) = gate {
                let mut rx = gate.subscribe();
                loop {
                    if *rx.borrow() {
                        break;
                    }
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            }
            *status_value.lock() = TerminalSessionStatus::Exited {
                exit_code: Some(0),
                signal: None,
            };
            if let Some(operation) = operation.lock().clone() {
                operation.cancel();
            }
            Ok(())
        })
    }
}

/// The TS `backend()` helper: a typed stub provider tracking its sessions.
pub struct StubBackend {
    type_: String,
    pub sessions: Mutex<Vec<Arc<StubSession>>>,
}

impl StubBackend {
    pub fn new(type_: &str) -> Arc<Self> {
        Arc::new(Self {
            type_: type_.to_string(),
            sessions: Mutex::new(Vec::new()),
        })
    }
}

impl TerminalBackend for StubBackend {
    fn type_(&self) -> String {
        self.type_.clone()
    }

    fn spawn(
        &self,
        _spec: TerminalBackendSpawnSpec,
    ) -> futures::future::BoxFuture<
        'static,
        Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>,
    > {
        let session = Arc::new(StubSession::default());
        self.sessions.lock().push(session.clone());
        Box::pin(async move { Ok(session as Arc<dyn TerminalBackendSession>) })
    }
}

/// A closure-backed provider for the inline TS backends.
pub struct FnBackend {
    type_: String,
    spawn: Arc<
        dyn Fn(
                TerminalBackendSpawnSpec,
            )
                -> futures::future::BoxFuture<
                'static,
                Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>,
            > + Send
            + Sync,
    >,
}

impl FnBackend {
    pub fn new(
        type_: &str,
        spawn: impl Fn(
                TerminalBackendSpawnSpec,
            )
                -> futures::future::BoxFuture<
                'static,
                Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>,
            > + Send
            + Sync
            + 'static,
    ) -> Arc<dyn TerminalBackend> {
        Arc::new(Self {
            type_: type_.to_string(),
            spawn: Arc::new(spawn),
        })
    }
}

impl TerminalBackend for FnBackend {
    fn type_(&self) -> String {
        self.type_.clone()
    }

    fn spawn(
        &self,
        spec: TerminalBackendSpawnSpec,
    ) -> futures::future::BoxFuture<
        'static,
        Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>,
    > {
        (self.spawn)(spec)
    }
}

/// A spawn future that waits on a gate before resolving the session (the TS
/// `Promise.withResolvers` gates).
fn gated_session(
    rx: oneshot::Receiver<Arc<StubSession>>,
) -> futures::future::BoxFuture<'static, Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>> {
    Box::pin(async move {
        let session = rx.await.expect("gate resolved");
        Ok(session as Arc<dyn TerminalBackendSession>)
    })
}

/// A spawn future that reports its start, then rejects once the spec signal
/// fires (the TS inline cleanup/abortable backends). `oneshot::Sender` is
/// single-shot, so the slot rides a mutex.
fn observing_backend_spawn(
    started_tx: Option<Arc<Mutex<Option<oneshot::Sender<()>>>>>,
    cleanup_failure: Option<&'static str>,
) -> impl Fn(
    TerminalBackendSpawnSpec,
) -> futures::future::BoxFuture<
    'static,
    Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>,
> {
    move |spec: TerminalBackendSpawnSpec| {
        let started_slot = started_tx.clone();
        Box::pin(async move {
            let Some(signal) = spec.signal else {
                return Err(TerminalBackendSpawnError::spawn("missing spawn signal"));
            };
            if let Some(slot) = started_slot {
                if let Some(started) = slot.lock().take() {
                    let _ = started.send(());
                }
            }
            loop {
                if signal() {
                    return match cleanup_failure {
                        Some(cleanup) => Err(TerminalBackendSpawnError::cleanup_failed(
                            "backend observed cancellation",
                            cleanup,
                        )),
                        None => Err(TerminalBackendSpawnError::spawn(
                            "backend observed cancellation",
                        )),
                    };
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    }
}

/// A single-use gated backend: the first spawn consumes the receiver (the
/// TS `Promise.withResolvers` gate, one per test spawn).
fn gated_backend(
    rx: Mutex<Option<oneshot::Receiver<Arc<StubSession>>>>,
) -> impl Fn(
    TerminalBackendSpawnSpec,
) -> futures::future::BoxFuture<
    'static,
    Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>,
> {
    move |_spec: TerminalBackendSpawnSpec| {
        let rx = rx
            .lock()
            .take()
            .expect("single-use gate consumed exactly once");
        gated_session(rx)
    }
}

struct Harness {
    ctx: Context,
    terminals_fiber: Arc<FiberCore>,
    agents: Arc<AgentRegistry>,
}

impl Harness {
    fn service(&self) -> Arc<dsh_terminal::TerminalSessionService> {
        self.ctx
            .get_typed::<Arc<dsh_terminal::TerminalSessionService>>("terminals", false)
            .map(|slot| slot.as_ref().clone())
            .expect("terminals service registered")
    }
}

async fn harness() -> Harness {
    let ctx = Context::root();
    let agents = AgentRegistry::install(&ctx);
    let terminals_fiber = ctx.plugin(Arc::new(TerminalsPlugin), cordis::arc(()));
    terminals_fiber.settle().await.expect("terminals service loads");
    Harness {
        ctx,
        terminals_fiber,
        agents,
    }
}

async fn dispose_terminal_session_service(harness: &Harness) {
    harness.terminals_fiber.dispose().await;
}

/// Poll `condition` on the current-thread runtime until true (the TS
/// synchronous-abort observations).
async fn yield_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if condition() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition never became true");
}

fn assert_code(failure: &TerminalFailure, code: TerminalErrorCode) {
    assert_eq!(failure.code(), Some(code), "failure: {failure}");
}

fn spawn_request(type_: &str, name: Option<&str>, cwd: Option<&str>) -> TerminalSpawnRequest {
    TerminalSpawnRequest {
        type_: type_.to_string(),
        name: name.map(str::to_string),
        cwd: cwd.map(str::to_string),
    }
}

fn send_request(text: &str, submit: bool) -> TerminalSendRequest {
    TerminalSendRequest {
        text: text.to_string(),
        submit,
        signal: None,
    }
}

/// Start driving a spawn future on the runtime (the TS async function runs
/// at call; the Rust future needs a poller before its backend side effects
/// are observable).
fn drive<T: Send + 'static>(
    future: futures::future::BoxFuture<'static, Result<T, TerminalFailure>>,
) -> tokio::task::JoinHandle<Result<T, TerminalFailure>> {
    tokio::spawn(future)
}

/// Register an agent and wait for the registry's async effect body to make
/// it live (the Rust `agents.register()` enters through a spawned effect).
async fn register_agent(harness: &Harness, agent: &Arc<dyn Agent>) {
    harness.agents.register(&harness.ctx, agent.clone());
    let id = agent.id().clone();
    yield_until(move || harness.agents.get(&id).is_some()).await;
}

fn render_panic(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic>".to_string()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn preserves_the_id_brand_and_disposes_exact_backend_contributions() {
    // The brand is a compile-time cast (the TS `expectTypeOf` assertion).
    let branded: dsh_terminal::TerminalSessionId = terminal_session_id("pty-1");
    assert_eq!(branded.as_str(), "pty-1");

    let harness = harness().await;
    let service = harness.service();
    let stub = StubBackend::new("stub");
    let dispose = service
        .register_backend(stub.clone() as Arc<dyn TerminalBackend>)
        .expect("first registration");
    assert_eq!(service.list_backends(), vec!["stub"]);
    let duplicate = service.register_backend(StubBackend::new("stub"));
    assert_code(duplicate.err().as_ref().expect("duplicate"), TerminalErrorCode::DuplicateBackend);
    // An internal replacement is NOT removed by the original contribution's
    // disposer (exact-contribution cleanup).
    *service.backends().lock() = vec![("stub".to_string(), StubBackend::new("replacement") as Arc<dyn TerminalBackend>)];
    (dispose)().await;
    assert_eq!(service.list_backends(), vec!["stub"]);
    service.backends().lock().clear();
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_empty_backend_types() {
    let harness = harness().await;
    let service = harness.service();
    let error = service
        .register_backend(StubBackend::new(""))
        .err()
        .expect("empty type rejected");
    assert!(error.message().contains("must be non-empty"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn publishes_only_after_spawn_and_fences_every_operation_to_the_exact_owner() {
    let harness = harness().await;
    let service = harness.service();
    service
        .register_backend(StubBackend::new("stub"))
        .expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    let (foreign, _foreign_fiber) = StubAgent::new(&harness.ctx, "foreign");
    register_agent(&harness, &owner).await;
    register_agent(&harness, &foreign).await;

    let created = service
        .spawn(
            owner.clone(),
            spawn_request("stub", Some("main"), Some("/tmp")),
            None,
        )
        .expect("spawn starts")
        .await
        .expect("spawn publishes");
    assert_eq!(created.session_id.as_str(), "pty-1");
    assert_eq!(created.name.as_deref(), Some("main"));
    assert_eq!(created.type_, "stub");
    assert_eq!(created.pid, Some(123));
    assert_eq!(created.motd, "stub ready");
    assert_eq!(created.status, TerminalSessionStatus::Running);
    assert!(service.has_owner_activity(&owner));
    assert_eq!(service.list(&owner).len(), 1);
    assert!(service.list(&foreign).is_empty());

    let read = service
        .read(&foreign, &created.session_id, TerminalReadRequest::default())
        .err()
        .expect("foreign read rejected");
    assert!(read.message().contains("belongs to another agent"), "{read}");
    assert_code(&read, TerminalErrorCode::ForeignSession);
    let signal = service
        .signal(&foreign, &created.session_id, TerminalSignal::SigInt)
        .err()
        .expect("foreign signal rejected");
    assert!(signal.message().contains("belongs to another agent"), "{signal}");
    let kill = service
        .kill(&foreign, &created.session_id, "model request".to_string())
        .err()
        .expect("foreign kill rejected");
    assert!(kill.message().contains("belongs to another agent"), "{kill}");
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_unknown_backends_non_live_owners_duplicate_names_and_active_sends() {
    let harness = harness().await;
    let service = harness.service();
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");

    let error = service
        .spawn(owner.clone(), spawn_request("missing", None, None), None)
        .err()
        .expect("unregistered owner rejected");
    assert_code(&error, TerminalErrorCode::OwnerNotLive);

    register_agent(&harness, &owner).await;
    let error = service
        .spawn(owner.clone(), spawn_request("missing", None, None), None)
        .err()
        .expect("missing backend rejected");
    assert_code(&error, TerminalErrorCode::NoBackend);

    let stub = StubBackend::new("stub");
    service.register_backend(stub.clone()).expect("backend");
    let created = service
        .spawn(owner.clone(), spawn_request("stub", Some("main"), None), None)
        .expect("spawn starts")
        .await
        .expect("published");

    let error = service
        .spawn(owner.clone(), spawn_request("stub", Some(""), None), None)
        .err()
        .expect("empty name rejected");
    assert!(error.message().contains("must be non-empty"), "{error}");

    let caller_aborted: TerminalAbort = Arc::new(|| true);
    let error = service
        .spawn(owner.clone(), spawn_request("stub", None, None), Some(caller_aborted))
        .err()
        .expect("aborted spawn rejected");
    assert!(matches!(error, TerminalFailure::Aborted), "{error}");

    let error = service
        .spawn(owner.clone(), spawn_request("stub", Some("main"), None), None)
        .err()
        .expect("duplicate name rejected");
    assert_code(&error, TerminalErrorCode::DuplicateName);

    let operation = service
        .start_send(&owner, &created.session_id, send_request("echo hi", true))
        .expect("first send starts");
    let error = service
        .start_send(&owner, &created.session_id, send_request("pwd", true))
        .err()
        .expect("second send rejected");
    assert_code(&error, TerminalErrorCode::SendActive);
    let read = operation.read_output();
    assert_eq!(read.delta, "delta");
    assert!(!read.truncated);
    assert!(operation.cancel());
    let settled = operation.done().await;
    assert_eq!(settled.viewport, "done");
    // The TS `.then` clear is a microtask that drains before the await
    // continuation; the Rust clear rides a spawned task, so let it run.
    tokio::task::yield_now().await;
    let next = service
        .start_send(&owner, &created.session_id, send_request("pwd", true))
        .expect("next send starts");
    assert!(next.cancel());
    next.done().await;
    // Let the second operation's clear task run before the failing send.
    tokio::task::yield_now().await;

    stub.sessions.lock()[0].reject_send.store(true, SeqCst);
    let failing = service
        .start_send(&owner, &created.session_id, send_request("bad", true))
        .expect("failing send starts");
    let rejection = std::panic::AssertUnwindSafe(async { failing.done().await })
        .catch_unwind()
        .await;
    let panic = rejection.err().expect("send rejects");
    assert!(render_panic(&panic).contains("send failed"));
    tokio::task::yield_now().await;
}

#[tokio::test(flavor = "current_thread")]
async fn reserves_concurrent_names_and_rolls_back_a_spawn_whose_owner_disappears() {
    let harness = harness().await;
    let service = harness.service();
    let (gate_tx, gate_rx) = oneshot::channel::<Arc<StubSession>>();
    service
        .register_backend(FnBackend::new(
            "slow",
            gated_backend(Mutex::new(Some(gate_rx))),
        ))
        .expect("backend");
    let (owner, owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let pending = service
        .spawn(owner.clone(), spawn_request("slow", Some("main"), None), None)
        .expect("spawn starts");
    let error = service
        .spawn(owner.clone(), spawn_request("slow", Some("main"), None), None)
        .err()
        .expect("concurrent duplicate rejected");
    assert_code(&error, TerminalErrorCode::DuplicateName);

    // Disposing the owner scope must abort the unpublished setup before the
    // backend resolves.
    let disposal = tokio::spawn(async move {
        owner_fiber.dispose().await;
    });
    yield_until(|| service.pending_aborted(&owner)).await;
    let session = Arc::new(StubSession::default());
    assert!(gate_tx.send(session.clone()).is_ok());
    let failure = pending.await.err().expect("pending spawn rejected");
    assert_code(&failure, TerminalErrorCode::OwnerNotLive);
    disposal.await.expect("owner disposal settles");
    assert_eq!(*session.closed.lock(), vec!["PTY spawn rolled back"]);
}

#[tokio::test(flavor = "current_thread")]
async fn preserves_caller_cancellation_when_a_pending_backend_spawn_completes() {
    let harness = harness().await;
    let service = harness.service();
    let (gate_tx, gate_rx) = oneshot::channel::<Arc<StubSession>>();
    service
        .register_backend(FnBackend::new(
            "slow",
            gated_backend(Mutex::new(Some(gate_rx))),
        ))
        .expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let aborted = Arc::new(AtomicBool::new(false));
    let caller: TerminalAbort = Arc::new({
        let aborted = aborted.clone();
        move || aborted.load(SeqCst)
    });
    let pending = service
        .spawn(owner.clone(), spawn_request("slow", None, None), Some(caller))
        .expect("spawn starts");
    aborted.store(true, SeqCst);
    let session = Arc::new(StubSession::default());
    assert!(gate_tx.send(session.clone()).is_ok());

    let failure = pending.await.err().expect("pending rejected");
    assert!(matches!(failure, TerminalFailure::Aborted), "{failure}");
    assert_eq!(*session.closed.lock(), vec!["PTY spawn rolled back"]);
    let registered = harness.agents.get(owner.id()).expect("still registered");
    assert!(Arc::ptr_eq(&registered, &owner));
}

#[tokio::test(flavor = "current_thread")]
async fn preserves_caller_cancellation_when_unpublished_rollback_fails() {
    let harness = harness().await;
    let service = harness.service();
    let (gate_tx, gate_rx) = oneshot::channel::<Arc<StubSession>>();
    service
        .register_backend(FnBackend::new(
            "slow",
            gated_backend(Mutex::new(Some(gate_rx))),
        ))
        .expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let aborted = Arc::new(AtomicBool::new(false));
    let caller: TerminalAbort = Arc::new({
        let aborted = aborted.clone();
        move || aborted.load(SeqCst)
    });
    let pending = service
        .spawn(owner.clone(), spawn_request("slow", None, None), Some(caller))
        .expect("spawn starts");
    aborted.store(true, SeqCst);
    let session = Arc::new(StubSession::default());
    session.reject_close.store(true, SeqCst);
    assert!(gate_tx.send(session.clone()).is_ok());

    let failure = pending.await.err().expect("pending rejected");
    assert!(matches!(failure, TerminalFailure::Aborted), "{failure}");
    assert!(service.has_owner_activity(&owner));
    let disposal = service.dispose_all();
    let error = disposal.await.err().expect("disposal reports the retained cleanup failure");
    assert_eq!(error.message(), "failed to clean up PTY lifecycle");
    assert!(!service.has_owner_activity(&owner));
    assert_eq!(*session.closed.lock(), vec!["PTY spawn rolled back"]);
}

#[tokio::test(flavor = "current_thread")]
async fn preserves_caller_cancellation_when_a_backend_rejects_in_response_to_it() {
    let harness = harness().await;
    let service = harness.service();
    let (started_tx, started_rx) = oneshot::channel::<()>();
    service
        .register_backend(FnBackend::new(
            "abortable",
            observing_backend_spawn(Some(Arc::new(Mutex::new(Some(started_tx)))), None),
        ))
        .expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let aborted = Arc::new(AtomicBool::new(false));
    let caller: TerminalAbort = Arc::new({
        let aborted = aborted.clone();
        move || aborted.load(SeqCst)
    });
    let pending = drive(service
        .spawn(owner.clone(), spawn_request("abortable", None, None), Some(caller))
        .expect("spawn starts"));
    started_rx.await.expect("backend started");
    aborted.store(true, SeqCst);

    let failure = pending.await.expect("spawn task").err().expect("pending rejected");
    assert!(matches!(failure, TerminalFailure::Aborted), "{failure}");
}

#[tokio::test(flavor = "current_thread")]
async fn retains_caller_triggered_backend_cleanup_failure_until_owner_disposal() {
    retains_caller_triggered_cleanup_failure_until_disposal("owner").await;
}

#[tokio::test(flavor = "current_thread")]
async fn retains_caller_triggered_backend_cleanup_failure_until_service_disposal() {
    retains_caller_triggered_cleanup_failure_until_disposal("service").await;
}

async fn retains_caller_triggered_cleanup_failure_until_disposal(scope: &str) {
    let harness = harness().await;
    let service = harness.service();
    let (started_tx, started_rx) = oneshot::channel::<()>();
    service
        .register_backend(FnBackend::new(
            "cleanup-failing",
            observing_backend_spawn(Some(Arc::new(Mutex::new(Some(started_tx)))), Some("backend cleanup failed")),
        ))
        .expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let aborted = Arc::new(AtomicBool::new(false));
    let caller: TerminalAbort = Arc::new({
        let aborted = aborted.clone();
        move || aborted.load(SeqCst)
    });
    let pending = drive(service
        .spawn(
            owner.clone(),
            spawn_request("cleanup-failing", None, None),
            Some(caller),
        )
        .expect("spawn starts"));
    started_rx.await.expect("backend started");
    aborted.store(true, SeqCst);

    let failure = pending.await.expect("spawn task").err().expect("pending rejected");
    assert!(matches!(failure, TerminalFailure::Aborted), "{failure}");
    assert!(service.has_owner_activity(&owner));

    let disposal = if scope == "owner" {
        service.dispose_owned(&owner)
    } else {
        service.dispose_all()
    };
    let error = disposal.await.err().expect("disposal reports the retained cleanup failure");
    assert_eq!(error.message(), "failed to clean up PTY lifecycle");
    assert!(!service.has_owner_activity(&owner));
}

#[tokio::test(flavor = "current_thread")]
async fn owner_disposal_aborts_and_awaits_unpublished_backend_setup() {
    disposal_aborts_and_awaits_unpublished_setup("owner").await;
}

#[tokio::test(flavor = "current_thread")]
async fn service_disposal_aborts_and_awaits_unpublished_backend_setup() {
    disposal_aborts_and_awaits_unpublished_setup("service").await;
}

async fn disposal_aborts_and_awaits_unpublished_setup(scope: &str) {
    let harness = harness().await;
    let service = harness.service();
    let (gate_tx, gate_rx) = oneshot::channel::<Arc<StubSession>>();
    let (started_tx, started_rx) = oneshot::channel::<()>();
    let gates = Arc::new(Mutex::new(vec![gate_rx]));
    let started = Arc::new(Mutex::new(Some(started_tx)));
    service
        .register_backend(FnBackend::new("slow", move |spec| {
            let gates = gates.clone();
            let started = started.clone();
            Box::pin(async move {
                let Some(signal) = spec.signal else {
                    return Err(TerminalBackendSpawnError::spawn("missing spawn signal"));
                };
                if let Some(tx) = started.lock().take() {
                    let _ = tx.send(());
                }
                let _ = signal;
                let rx = gates.lock().remove(0);
                gated_session(rx).await
            })
        }))
        .expect("backend");
    let (owner, owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let pending = drive(service
        .spawn(owner.clone(), spawn_request("slow", None, None), None)
        .expect("spawn starts"));
    started_rx.await.expect("backend started");

    let expected_code = if scope == "owner" {
        TerminalErrorCode::OwnerNotLive
    } else {
        TerminalErrorCode::ServiceDisposing
    };
    let disposal = if scope == "owner" {
        tokio::spawn(async move {
            owner_fiber.dispose().await;
        })
    } else {
        let fiber = harness.terminals_fiber.clone();
        tokio::spawn(async move {
            fiber.dispose().await;
        })
    };
    yield_until(|| service.pending_aborted(&owner)).await;
    assert!(!disposal.is_finished(), "disposal must wait for backend settlement");
    assert_eq!(
        service.pending_abort_error(&owner).map(|error| error.code),
        Some(expected_code)
    );
    let session = Arc::new(StubSession::default());
    assert!(gate_tx.send(session.clone()).is_ok());

    let failure = pending.await.expect("spawn task").err().expect("pending rejected");
    assert_code(&failure, expected_code);
    disposal.await.expect("disposal settles");
    assert_eq!(*session.closed.lock(), vec!["PTY spawn rolled back"]);
}

#[tokio::test(flavor = "current_thread")]
async fn reports_unpublished_rollback_failure_through_service_disposal() {
    let harness = harness().await;
    let service = harness.service();
    let (gate_tx, gate_rx) = oneshot::channel::<Arc<StubSession>>();
    service
        .register_backend(FnBackend::new(
            "slow",
            gated_backend(Mutex::new(Some(gate_rx))),
        ))
        .expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let pending = service
        .spawn(owner.clone(), spawn_request("slow", None, None), None)
        .expect("spawn starts");
    // The disposal call sets `disposing` and aborts the reservation at the
    // call (the TS sync prefix).
    let disposal = service.dispose_all();
    let session = Arc::new(StubSession::default());
    session.reject_close.store(true, SeqCst);
    assert!(gate_tx.send(session.clone()).is_ok());

    let failure = pending.await.err().expect("pending rejected");
    assert_eq!(failure.message(), "PTY spawn and rollback both failed");
    let error = disposal.await.err().expect("disposal reports the cleanup failure");
    assert_eq!(error.message(), "failed to clean up PTY lifecycle");
    assert_eq!(*session.closed.lock(), vec!["PTY spawn rolled back"]);
}

#[tokio::test(flavor = "current_thread")]
async fn owner_disposal_retains_backend_side_startup_cleanup_failure() {
    backend_side_cleanup_failure("owner").await;
}

#[tokio::test(flavor = "current_thread")]
async fn service_disposal_retains_backend_side_startup_cleanup_failure() {
    backend_side_cleanup_failure("service").await;
}

async fn backend_side_cleanup_failure(scope: &str) {
    let harness = harness().await;
    let service = harness.service();
    let (started_tx, started_rx) = oneshot::channel::<()>();
    service
        .register_backend(FnBackend::new(
            "cleanup-failing",
            observing_backend_spawn(Some(Arc::new(Mutex::new(Some(started_tx)))), Some("backend cleanup failed")),
        ))
        .expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let pending = drive(service
        .spawn(owner.clone(), spawn_request("cleanup-failing", None, None), None)
        .expect("spawn starts"));
    started_rx.await.expect("backend started");

    let expected_code = if scope == "owner" {
        TerminalErrorCode::OwnerNotLive
    } else {
        TerminalErrorCode::ServiceDisposing
    };
    // The disposal call fires its synchronous abort at the call (the TS
    // async function's sync prefix).
    let disposal = if scope == "owner" {
        service.dispose_owned(&owner)
    } else {
        service.dispose_all()
    };

    let failure = pending.await.expect("spawn task").err().expect("pending rejected");
    assert_code(&failure, expected_code);
    let error = disposal.await.err().expect("disposal reports the cleanup failure");
    assert_eq!(error.message(), "failed to clean up PTY lifecycle");
    // The aggregate nests: lifecycle 閳?unpublished-setup 閳?cleanup failure.
    let TerminalFailure::Aggregate { failures: lifecycle, .. } = &error else {
        panic!("expected lifecycle aggregate: {error}");
    };
    let TerminalFailure::Aggregate { failures: rollback, .. } = &lifecycle[0] else {
        panic!("expected rollback aggregate: {:?}", lifecycle[0]);
    };
    assert_eq!(rollback.len(), 1);
    assert_eq!(rollback[0].message(), "backend cleanup failed");
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_independent_reservations_and_handles_provider_failure_before_publication() {
    let harness = harness().await;
    let service = harness.service();
    let (first_tx, first_rx) = oneshot::channel::<Arc<StubSession>>();
    let (second_tx, second_rx) = oneshot::channel::<Arc<StubSession>>();
    let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let gates = Arc::new(Mutex::new(vec![first_rx, second_rx]));
    service
        .register_backend(FnBackend::new("slow", move |_spec| {
            let count = count.clone();
            let gates = gates.clone();
            Box::pin(async move {
                let rx = gates.lock().remove(0);
                if count.fetch_add(1, SeqCst) == 0 {
                    gated_session(rx).await
                } else {
                    gated_session(rx).await
                }
            })
        }))
        .expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let first = service
        .spawn(owner.clone(), spawn_request("slow", Some("one"), None), None)
        .expect("spawn starts");
    let second = service
        .spawn(owner.clone(), spawn_request("slow", Some("two"), None), None)
        .expect("spawn starts");
    first_tx
        .send(Arc::new(StubSession::default()))
        .unwrap_or_else(|_| panic!("first gate"));
    first.await.expect("first publishes");
    second_tx
        .send(Arc::new(StubSession::default()))
        .unwrap_or_else(|_| panic!("second gate"));
    second.await.expect("second publishes");

    service
        .register_backend(FnBackend::new("throwing", |_spec| {
            Box::pin(async { Err(TerminalBackendSpawnError::spawn("provider failed")) })
        }))
        .expect("backend");
    let failure = service
        .spawn(owner.clone(), spawn_request("throwing", None, None), None)
        .expect("spawn starts")
        .await
        .err()
        .expect("provider failure rejects");
    assert_eq!(failure.message(), "provider failed");

    let caller: TerminalAbort = Arc::new(|| false);
    service.register_backend(StubBackend::new("signaled")).expect("backend");
    let created = service
        .spawn(owner.clone(), spawn_request("signaled", None, None), Some(caller))
        .expect("spawn starts")
        .await
        .expect("publishes under a live signal");
    assert_eq!(created.type_, "signaled");
}

#[tokio::test(flavor = "current_thread")]
async fn omits_optional_pid_metadata_when_a_backend_has_no_process_id() {
    let harness = harness().await;
    let service = harness.service();
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;
    let session = Arc::new(StubSession::default());
    *session.pid_value.lock() = None;
    service
        .register_backend(FnBackend::new("virtual", move |_spec| {
            let session = session.clone();
            Box::pin(async move { Ok(session as Arc<dyn TerminalBackendSession>) })
        }))
        .expect("backend");
    let created = service
        .spawn(owner.clone(), spawn_request("virtual", None, None), None)
        .expect("spawn starts")
        .await
        .expect("published");
    assert_eq!(created.pid, None);
}

#[tokio::test(flavor = "current_thread")]
async fn reports_rollback_and_close_failures_without_publishing_false_success() {
    let harness = harness().await;
    let service = harness.service();
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let failed_spawn = Arc::new(StubSession::default());
    failed_spawn.reject_close.store(true, SeqCst);
    let (disposal_tx, disposal_rx) = oneshot::channel::<
        futures::future::BoxFuture<'static, Result<(), TerminalFailure>>,
    >();
    let disposal_slot = Arc::new(Mutex::new(Some(disposal_tx)));
    let service_hook = service.clone();
    let owner_hook = owner.clone();
    let failed_spawn_hook = failed_spawn.clone();
    service
        .register_backend(FnBackend::new("bad-spawn", move |spec| {
            let service = service_hook.clone();
            let owner = owner_hook.clone();
            let failed_spawn = failed_spawn_hook.clone();
            let disposal_slot = disposal_slot.clone();
            Box::pin(async move {
                let Some(signal) = spec.signal else {
                    return Err(TerminalBackendSpawnError::spawn("missing spawn signal"));
                };
                // The TS backend reaches into the service: mark the owner
                // disposed and start its cleanup while the spawn is pending.
                service.mark_owner_disposed(&owner);
                if let Some(tx) = disposal_slot.lock().take() {
                    let _ = tx.send(service.dispose_owned(&owner));
                }
                loop {
                    if signal() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Ok(failed_spawn as Arc<dyn TerminalBackendSession>)
            })
        }))
        .expect("backend");

    let failure = service
        .spawn(owner.clone(), spawn_request("bad-spawn", None, None), None)
        .expect("spawn starts")
        .await
        .err()
        .expect("spawn rejects");
    assert_eq!(failure.message(), "PTY spawn and rollback both failed");
    let disposal = disposal_rx.await.expect("owner disposal future");
    let error = tokio::spawn(disposal)
        .await
        .expect("disposal task")
        .err()
        .expect("owner disposal reports the lifecycle failure");
    assert_eq!(error.message(), "failed to clean up PTY lifecycle");

    let (next_owner, _next_fiber) = StubAgent::new(&harness.ctx, "next");
    register_agent(&harness, &next_owner).await;
    let bad_close = StubBackend::new("bad-close");
    service.register_backend(bad_close.clone()).expect("backend");
    let created = service
        .spawn(next_owner.clone(), spawn_request("bad-close", None, None), None)
        .expect("spawn starts")
        .await
        .expect("published");
    bad_close.sessions.lock()[0].reject_close.store(true, SeqCst);
    let failure = service
        .kill(&next_owner, &created.session_id, "model request".to_string())
        .expect("kill starts")
        .await
        .err()
        .expect("close failure rejects");
    assert_eq!(failure.message(), "close failed");
    assert_eq!(service.list(&next_owner).len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn joins_an_already_running_close_and_refuses_new_sends_while_closing() {
    let harness = harness().await;
    let service = harness.service();
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;
    let stub = StubBackend::new("stub");
    service.register_backend(stub.clone()).expect("backend");
    let created = service
        .spawn(owner.clone(), spawn_request("stub", None, None), None)
        .expect("spawn starts")
        .await
        .expect("published");

    let (gate_tx, _gate_rx) = watch::channel(false);
    *stub.sessions.lock()[0].close_gate.lock() = Some(gate_tx.clone());
    let first = service
        .kill(&owner, &created.session_id, "model request".to_string())
        .expect("kill starts");
    let send = service
        .start_send(&owner, &created.session_id, send_request("", false))
        .err()
        .expect("send refused while closing");
    assert!(send.message().contains("closing"), "{send}");
    let second = service
        .kill(&owner, &created.session_id, "model request".to_string())
        .expect("second kill joins");
    let _ = gate_tx.send(true);
    assert_eq!(first.await.expect("first close"), true);
    assert_eq!(second.await.expect("second close"), false);
    let read = service
        .read(&owner, &created.session_id, TerminalReadRequest::default())
        .err()
        .expect("session removed");
    assert!(read.message().contains("unknown PTY"), "{read}");
}

#[tokio::test(flavor = "current_thread")]
async fn awaits_owner_cleanup_and_removes_sessions_while_backend_registration_may_reload() {
    let harness = harness().await;
    let service = harness.service();
    let stub = StubBackend::new("stub");
    let dispose_backend = service.register_backend(stub.clone()).expect("backend");
    let (owner, owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;
    let created = service
        .spawn(owner.clone(), spawn_request("stub", None, None), None)
        .expect("spawn starts")
        .await
        .expect("published");

    (dispose_backend)().await;
    assert!(service.list_backends().is_empty());
    let read = service
        .read(&owner, &created.session_id, TerminalReadRequest::default())
        .expect("sessions outlive backend registration");
    assert_eq!(read.text, "0:0");

    owner_fiber.dispose().await;
    assert_eq!(*stub.sessions.lock()[0].closed.lock(), vec!["PTY owner disposed"]);
    assert!(service.list(&owner).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn kills_idempotently_and_service_disposal_closes_all_owners() {
    let harness = harness().await;
    let service = harness.service();
    let stub = StubBackend::new("stub");
    service.register_backend(stub.clone()).expect("backend");
    let (first_owner, _first_fiber) = StubAgent::new(&harness.ctx, "first");
    let (second_owner, _second_fiber) = StubAgent::new(&harness.ctx, "second");
    register_agent(&harness, &first_owner).await;
    register_agent(&harness, &second_owner).await;
    let a = service
        .spawn(first_owner.clone(), spawn_request("stub", None, None), None)
        .expect("spawn starts")
        .await
        .expect("published");
    service
        .spawn(second_owner.clone(), spawn_request("stub", None, None), None)
        .expect("spawn starts")
        .await
        .expect("published");

    assert_eq!(
        service
            .kill(&first_owner, &a.session_id, "model request".to_string())
            .expect("kill starts")
            .await
            .expect("killed"),
        true
    );
    assert_eq!(*stub.sessions.lock()[0].closed.lock(), vec!["model request"]);

    dispose_terminal_session_service(&harness).await;
    assert_eq!(
        *stub.sessions.lock()[1].closed.lock(),
        vec!["PTY service disposed"]
    );
    let error = service
        .spawn(first_owner.clone(), spawn_request("stub", None, None), None)
        .err()
        .expect("disposed service rejects");
    assert_code(&error, TerminalErrorCode::ServiceDisposing);
}

#[tokio::test(flavor = "current_thread")]
async fn aggregates_service_disposal_close_failures_after_attempting_every_record() {
    let harness = harness().await;
    let service = harness.service();
    let stub = StubBackend::new("stub");
    service.register_backend(stub.clone()).expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;
    service
        .spawn(owner.clone(), spawn_request("stub", None, None), None)
        .expect("spawn starts")
        .await
        .expect("published");

    stub.sessions.lock()[0].reject_close.store(true, SeqCst);
    let records = service.session_records();

    // Both calls install their fences at the call (the TS sync prefix); the
    // second must JOIN the first's fence, not start a second close.
    let first = service.close_records(records.clone(), "test failure".to_string());
    let joined = service.close_records(records.clone(), "joined failure".to_string());
    let first = first.await.err().expect("first close fails");
    assert!(first.message().contains("failed to close 1 PTY session"));
    let joined = joined.await.err().expect("joined close fails");
    assert!(joined.message().contains("failed to close 1 PTY session"));

    stub.sessions.lock()[0].reject_close.store(false, SeqCst);
    service
        .close_records(records.clone(), "retry".to_string())
        .await
        .expect("retry succeeds");
    assert_eq!(
        *stub.sessions.lock()[0].closed.lock(),
        vec!["test failure", "retry"]
    );
    assert!(service.session_records().is_empty());

    dispose_terminal_session_service(&harness).await;
    let error = service
        .spawn(owner.clone(), spawn_request("stub", None, None), None)
        .err()
        .expect("disposed service rejects");
    assert_code(&error, TerminalErrorCode::ServiceDisposing);
}

#[tokio::test(flavor = "current_thread")]
async fn clears_registries_and_runs_owner_cleanups_even_when_a_session_close_fails() {
    let harness = harness().await;
    let service = harness.service();
    let stub = StubBackend::new("stub");
    service.register_backend(stub.clone()).expect("backend");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;
    service
        .spawn(owner.clone(), spawn_request("stub", None, None), None)
        .expect("spawn starts")
        .await
        .expect("published");
    stub.sessions.lock()[0].reject_close.store(true, SeqCst);

    // Teardown surfaces the close failure, but still clears the backend and
    // owner-cleanup registries instead of orphaning them.
    let disposal = service.dispose_all();
    let error = disposal.await.err().expect("disposal reports the close failure");
    assert_eq!(error.message(), "failed to clean up PTY lifecycle");
    assert_eq!(service.backends_len(), 0);
    assert_eq!(service.owner_cleanup_len(), 0);
}
