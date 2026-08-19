//! Rust port of the core `packages/jobs/jobs-local/tests/jobs.spec.ts`
//! behaviors: admission preflight, sequential ids, per-owner concurrency,
//! session fencing, stream/final reads, kill transitions, first-wins
//! settlement with contained listeners, bounded waits, and teardown
//! containment. The loader-composition admission check is covered at the
//! unit level (the Rust loader fixtures are exercised by the loader crate).
//!
//! # Deviations
//!
//! - Producer `done` rejections collapse into panics, contained into a
//!   `failed` settlement.
//! - The abort predicate is polled every 15 ms.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::Duration;

use cordis::{Context, FiberCore, Plugin};
use parking_lot::Mutex;
use tokio::sync::watch;

use dsh_agent::{Agent, AgentOptions, AgentRegistry, AgentStatus, Inbox};
use dsh_jobs::{
    JobHooks, JobOutcome, JobOutcomeStatus, JobRegistry, JobStart, JobStatus, KillOutcome,
};
use dsh_jobs_local::LocalJobRegistry;
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, session_id};

/// Scripted producer hooks: a watch-gated outcome, a cancellation log, and an
/// optional consuming stream.
struct TestHooks {
    cancel_log: Arc<Mutex<Vec<Option<String>>>>,
    outcome: watch::Sender<Option<JobOutcome>>,
    stream: Mutex<VecDeque<String>>,
    read_calls: Arc<Mutex<u64>>,
    cancel_panics: AtomicBool,
}

impl TestHooks {
    fn new() -> (Arc<TestHooks>, watch::Receiver<Option<JobOutcome>>) {
        let (outcome, rx) = watch::channel(None);
        (
            Arc::new(TestHooks {
                cancel_log: Arc::new(Mutex::new(Vec::new())),
                outcome,
                stream: Mutex::new(VecDeque::new()),
                read_calls: Arc::new(Mutex::new(0)),
                cancel_panics: AtomicBool::new(false),
            }),
            rx,
        )
    }
}

impl JobHooks for TestHooks {
    fn cancel(&self, reason: Option<String>) {
        if self.cancel_panics.load(SeqCst) {
            panic!("cancel blew up");
        }
        self.cancel_log.lock().push(reason);
    }

    fn done(&self) -> futures::future::BoxFuture<'static, JobOutcome> {
        let mut rx = self.outcome.subscribe();
        Box::pin(async move {
            loop {
                if let Some(outcome) = rx.borrow().clone() {
                    return outcome;
                }
                if rx.changed().await.is_err() {
                    return JobOutcome {
                        status: JobOutcomeStatus::Failed,
                        detail: Some("outcome sender dropped".to_string()),
                        output: None,
                    };
                }
            }
        })
    }

    fn read_output(&self) -> Option<String> {
        *self.read_calls.lock() += 1;
        self.stream.lock().pop_front()
    }
}

/// An inert agent scope fiber (the owner-cleanup effect anchor).
struct NoopPlugin;

#[async_trait::async_trait]
impl Plugin for NoopPlugin {
    async fn apply(
        &self,
        _ctx: &Context,
        _config: cordis::ArcValue,
    ) -> Result<(), cordis::PluginError> {
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
        let agent_ctx = fiber.ctx().expect("plugin ctx bound at load");
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

    fn cancel(
        &self,
        _cause: dsh_agent::AgentCancelCause,
        _options: Option<&dsh_agent::CancelOptions>,
    ) {
    }

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(
        &self,
        _message: dsh_session::UserMessage,
        _target: dsh_agent::InboxTarget,
        _wakeup: bool,
    ) {
    }

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}

struct Harness {
    ctx: Context,
    registry: Arc<LocalJobRegistry>,
    agents: Arc<AgentRegistry>,
}

async fn setup(config: dsh_jobs_local::Config) -> Harness {
    let ctx = Context::root();
    let agents = AgentRegistry::install(&ctx);
    let registry = LocalJobRegistry::install(&ctx, config);
    Harness {
        ctx,
        registry,
        agents,
    }
}

async fn register_agent(harness: &Harness, agent: &Arc<dyn Agent>) {
    harness.agents.register(&harness.ctx, agent.clone());
    let id = agent.id().clone();
    for _ in 0..10_000 {
        if harness.agents.get(&id).is_some() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("agent never became live");
}

fn start_spec(kind: &str, label: &str, hooks: Arc<TestHooks>) -> JobStart {
    JobStart {
        kind: kind.to_string(),
        label: label.to_string(),
        output_limit_bytes: None,
        owner: None,
        run: Arc::new(move || hooks.clone() as Arc<dyn JobHooks>),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_start_without_a_serving_controller() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    let (hooks, mut _rx) = TestHooks::new();
    let error = harness
        .registry
        .start(start_spec("bash", "sleep 60", hooks.clone()))
        .err()
        .expect("no controller");
    assert!(error.contains("no job controller serves"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn validates_start_inputs() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (hooks, mut _rx) = TestHooks::new();

    let error = harness
        .registry
        .start(start_spec("", "sleep 60", hooks.clone()))
        .err()
        .expect("empty kind");
    assert!(error.contains("non-empty"), "{error}");
    let error = harness
        .registry
        .start(start_spec("bash", "", hooks.clone()))
        .err()
        .expect("empty label");
    assert!(error.contains("non-empty"), "{error}");
    let mut spec = start_spec("bash", "sleep 60", hooks.clone());
    spec.output_limit_bytes = Some(0);
    let error = harness
        .registry
        .start(spec)
        .err()
        .expect("bad output limit");
    assert!(error.contains("outputLimitBytes"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn mints_sequential_ids_per_kind() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (a, _rx_a) = TestHooks::new();
    let (b, _rx_b) = TestHooks::new();
    let (c, _rx_c) = TestHooks::new();
    let id_a = harness
        .registry
        .start(start_spec("bash", "a", a))
        .expect("a");
    let id_b = harness
        .registry
        .start(start_spec("subagent", "b", b))
        .expect("b");
    let id_c = harness
        .registry
        .start(start_spec("bash", "c", c))
        .expect("c");
    assert_eq!(id_a.as_str(), "bash-1");
    assert_eq!(id_b.as_str(), "subagent-1");
    assert_eq!(id_c.as_str(), "bash-2");
}

#[tokio::test(flavor = "current_thread")]
async fn enforces_the_per_owner_concurrency_limit() {
    let harness = setup(dsh_jobs_local::Config {
        max_concurrent_jobs_per_owner: Some(1),
    })
    .await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (first, _rx) = TestHooks::new();
    let (second, _rx2) = TestHooks::new();
    harness
        .registry
        .start(start_spec("bash", "hold slot", first))
        .expect("first");
    let error = harness
        .registry
        .start(start_spec("bash", "blocked", second))
        .err()
        .expect("limit reached");
    assert!(error.contains("(limit: 1)"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn fences_access_by_owner_session() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (owner, _owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    let (foreign, _foreign_fiber) = StubAgent::new(&harness.ctx, "foreign");
    register_agent(&harness, &owner).await;
    register_agent(&harness, &foreign).await;

    let (hooks, mut _rx) = TestHooks::new();
    let mut spec = start_spec("bash", "owned", hooks.clone());
    spec.owner = Some(owner.clone());
    let id = harness.registry.start(spec).expect("owned start");
    // Let the owner-cleanup effect body register its disposer before the     // fiber starts draining (the effect execute task runs asynchronously).     for _ in 0..1_000 {         tokio::task::yield_now().await;     }

    let error = harness
        .registry
        .get(&id, Some(&foreign))
        .err()
        .expect("foreign get");
    assert!(error.contains("belongs to another session"), "{error}");
    let error = harness
        .registry
        .read(&id, Some(&foreign))
        .err()
        .expect("foreign read");
    assert!(error.contains("belongs to another session"), "{error}");
    let error = harness
        .registry
        .kill(&id, Some(&foreign), None)
        .err()
        .expect("foreign kill");
    assert!(error.contains("belongs to another session"), "{error}");
    // The exact owner reaches it.
    let snapshot = harness.registry.get(&id, Some(&owner)).expect("owner get");
    assert_eq!(snapshot.owner_session.as_ref(), Some(owner.id()));
    // An unowned job is open to any caller.
    let (unowned_hooks, _rx) = TestHooks::new();
    let unowned = harness
        .registry
        .start(start_spec("bash", "unowned", unowned_hooks))
        .expect("unowned start");
    assert!(harness.registry.get(&unowned, Some(&foreign)).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn reads_stream_output_and_marks_reported() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (hooks, mut rx) = TestHooks::new();
    hooks.stream.lock().push_back("chunk-a\n".to_string());
    hooks.stream.lock().push_back("chunk-b\n".to_string());
    let id = harness
        .registry
        .start(start_spec("bash", "stream", hooks.clone()))
        .expect("start");

    let first = harness.registry.read(&id, None).expect("read");
    assert_eq!(first.text, "chunk-a\n");
    assert!(!first.snapshot.reported);
    let second = harness.registry.read(&id, None).expect("read");
    assert_eq!(second.text, "chunk-b\n");
    assert!(!second.snapshot.reported);
    // The stream is empty while live; a terminal read marks reported.
    let third = harness.registry.read(&id, None).expect("read");
    assert_eq!(third.text, "");

    let _ = hooks.outcome.send(Some(JobOutcome {
        status: JobOutcomeStatus::Completed,
        detail: None,
        output: None,
    }));
    rx.wait_for(|outcome| outcome.is_some())
        .await
        .expect("settled");
    tokio::task::yield_now().await;
    let final_read = harness.registry.read(&id, None).expect("read");
    assert!(final_read.snapshot.reported);
    assert_eq!(final_read.snapshot.status, JobStatus::Completed);
    assert_eq!(hooks.read_calls.lock().clone(), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn reads_final_output_after_settlement_idempotently() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (hooks, mut rx) = TestHooks::new();
    let id = harness
        .registry
        .start(start_spec("bash", "final", hooks.clone()))
        .expect("start");
    let _ = hooks.outcome.send(Some(JobOutcome {
        status: JobOutcomeStatus::Completed,
        detail: Some("exit code: 3".to_string()),
        output: Some("final-body".to_string()),
    }));
    rx.wait_for(|outcome| outcome.is_some())
        .await
        .expect("settled");
    tokio::task::yield_now().await;
    let first = harness.registry.read(&id, None).expect("read");
    assert_eq!(first.text, "final-body");
    assert_eq!(first.snapshot.detail.as_deref(), Some("exit code: 3"));
    // Idempotent, never consumed.
    let again = harness.registry.read(&id, None).expect("read");
    assert_eq!(again.text, "final-body");
}

#[tokio::test(flavor = "current_thread")]
async fn kill_requests_then_marks_stopping_and_reported() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (hooks, mut _rx) = TestHooks::new();
    let id = harness
        .registry
        .start(start_spec("bash", "kill me", hooks.clone()))
        .expect("start");

    let outcome = harness
        .registry
        .kill(&id, None, Some("user asked".to_string()))
        .expect("kill");
    assert_eq!(outcome, KillOutcome::Requested);
    assert_eq!(
        *hooks.cancel_log.lock(),
        vec![Some("user asked".to_string())]
    );
    let snapshot = harness.registry.get(&id, None).expect("get");
    assert_eq!(snapshot.status, JobStatus::Stopping);
    assert!(snapshot.reported);
}

#[tokio::test(flavor = "current_thread")]
async fn kill_returns_already_finished_after_settlement() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (hooks, mut rx) = TestHooks::new();
    let id = harness
        .registry
        .start(start_spec("bash", "done soon", hooks.clone()))
        .expect("start");
    let _ = hooks.outcome.send(Some(JobOutcome {
        status: JobOutcomeStatus::Completed,
        detail: None,
        output: None,
    }));
    rx.wait_for(|outcome| outcome.is_some())
        .await
        .expect("settled");
    tokio::task::yield_now().await;
    let outcome = harness.registry.kill(&id, None, None).expect("kill");
    assert_eq!(outcome, KillOutcome::AlreadyFinished);
}

#[tokio::test(flavor = "current_thread")]
async fn settlement_is_first_wins_and_notifies_listeners() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let done_events: Arc<Mutex<Vec<(String, Option<String>)>>> = Arc::new(Mutex::new(Vec::new()));
    let changed_events: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let listener_events = done_events.clone();
    harness.registry.on_job_done(
        &harness.ctx,
        Arc::new(move |snapshot, owner| {
            listener_events.lock().push((
                snapshot.status.as_str().to_string(),
                owner.map(|o| o.id().as_str().to_string()),
            ));
        }),
    );
    let changed_events_for_listener = changed_events.clone();
    harness.registry.on_jobs_changed(
        &harness.ctx,
        Arc::new(move |_owner| {
            *changed_events_for_listener.lock() += 1;
        }),
    );

    let (hooks, mut rx) = TestHooks::new();
    let id = harness
        .registry
        .start(start_spec("bash", "settle me", hooks.clone()))
        .expect("start");
    // Registration itself notifies the visible-set observer.
    assert!(*changed_events.lock() >= 1);

    let _ = hooks.outcome.send(Some(JobOutcome {
        status: JobOutcomeStatus::Completed,
        detail: None,
        output: None,
    }));
    rx.wait_for(|outcome| outcome.is_some())
        .await
        .expect("settled");
    tokio::task::yield_now().await;
    let snapshot = harness.registry.get(&id, None).expect("get");
    assert_eq!(snapshot.status, JobStatus::Completed);
    assert!(snapshot.finished_at.is_some());
    // First-wins: a second settlement is ignored.
    let _ = hooks.outcome.send(Some(JobOutcome {
        status: JobOutcomeStatus::Failed,
        detail: Some("late".to_string()),
        output: None,
    }));
    tokio::task::yield_now().await;
    let snapshot = harness.registry.get(&id, None).expect("get");
    assert_eq!(snapshot.status, JobStatus::Completed);
    assert_eq!(snapshot.detail, None);

    assert_eq!(*done_events.lock(), vec![("completed".to_string(), None)]);
}

#[tokio::test(flavor = "current_thread")]
async fn wait_resolves_on_terminal_timeout_and_abort() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");

    // A live job waits until settlement.
    let (hooks, mut rx) = TestHooks::new();
    let id = harness
        .registry
        .start(start_spec("bash", "waiter", hooks.clone()))
        .expect("start");
    let waiting = harness.registry.wait(&id, 5_000, None, None);
    let waiting = tokio::spawn(waiting);
    let _ = hooks.outcome.send(Some(JobOutcome {
        status: JobOutcomeStatus::Killed,
        detail: None,
        output: None,
    }));
    rx.wait_for(|outcome| outcome.is_some())
        .await
        .expect("settled");
    let snapshot = waiting.await.expect("task").expect("wait settles");
    assert_eq!(snapshot.status, JobStatus::Killed);
    assert!(
        snapshot.reported,
        "a live waiter marks the settlement reported"
    );

    // A terminal job returns immediately.
    let immediate = harness.registry.wait(&id, 5_000, None, None);
    let snapshot = immediate.await.expect("immediate");
    assert_eq!(snapshot.status, JobStatus::Killed);

    // A bounded wait times out without cancelling the job.
    let (pending_hooks, _rx) = TestHooks::new();
    let pending = harness
        .registry
        .start(start_spec("bash", "timeout-waiter", pending_hooks))
        .expect("start");
    let started = std::time::Instant::now();
    let snapshot = harness
        .registry
        .wait(&pending, 50, None, None)
        .await
        .expect("timeout resolves with the snapshot");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(snapshot.status, JobStatus::Running);

    // A caller abort rejects while the job is live.
    let aborted: dsh_jobs::JobAbort = Arc::new(|| true);
    let error = harness
        .registry
        .wait(&pending, 5_000, None, Some(aborted))
        .await
        .err()
        .expect("aborted wait");
    assert_eq!(error, "wait aborted");
}

#[tokio::test(flavor = "current_thread")]
async fn teardown_cancels_live_work_and_force_fails_throwing_cancels() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (hooks, mut _rx) = TestHooks::new();
    harness
        .registry
        .start(start_spec("bash", "torn down", hooks.clone()))
        .expect("start");

    // Service disposal cancels and awaits settlement.
    let disposed = harness.registry.clone();
    let teardown = tokio::spawn(async move {
        disposed.dispose_all().await;
    });
    // The cancel lands synchronously in the disposal prefix.
    let _ = teardown;
    tokio::task::yield_now().await;
    let logs = hooks.cancel_log.lock().clone();
    assert_eq!(logs, vec![Some("jobs service disposed".to_string())]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_throwing_teardown_cancel_force_fails_the_record() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    // Owner disposal keeps listeners open, so the force-failed terminal
    // snapshot is observable through the completion channel (service
    // disposal clears the store before any external read).
    let (owner, owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let (hooks, _rx) = TestHooks::new();
    hooks.cancel_panics.store(true, SeqCst);
    let mut spec = start_spec("bash", "bad cancel", hooks.clone());
    spec.owner = Some(owner.clone());
    harness.registry.start(spec).expect("start");

    let done_events: Arc<Mutex<Vec<dsh_jobs::JobSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
    let listener_events = done_events.clone();
    harness.registry.on_job_done(
        &harness.ctx,
        Arc::new(move |snapshot, _owner| {
            listener_events.lock().push(snapshot);
            // Let the owner-cleanup effect body run (its disposer registers     // asynchronously) BEFORE the fiber starts draining.
        }),
    );

    owner_fiber.dispose().await;
    let events = done_events.lock().clone();
    let snapshot = events.last().expect("force-failed settlement announced");
    assert_eq!(snapshot.status, JobStatus::Failed);
    assert!(
        snapshot
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("cancel threw"),
        "{}",
        snapshot.detail.clone().unwrap_or_default()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn owner_disposal_cancels_owned_work() {
    let harness = setup(dsh_jobs_local::Config::default()).await;
    harness.registry.attach_controller(&harness.ctx, "test");
    let (owner, owner_fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &owner).await;

    let (hooks, mut rx) = TestHooks::new();
    let mut spec = start_spec("bash", "owned work", hooks.clone());
    spec.owner = Some(owner.clone());
    let id = harness.registry.start(spec).expect("owned start");

    // Let the owner-cleanup effect body register its disposer before the
    // fiber starts draining (the effect execute task runs asynchronously).
    for _ in 0..1_000 {
        tokio::task::yield_now().await;
    }
    let disposal = tokio::spawn(async move {
        owner_fiber.dispose().await;
    });
    // The owner cleanup cancels first, then awaits the producer settlement:
    // let the disposal chain reach the cancel before releasing.
    for _ in 0..10_000 {
        if !hooks.cancel_log.lock().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        *hooks.cancel_log.lock(),
        vec![Some("owner disposed".to_string())]
    );
    let _ = hooks.outcome.send(Some(JobOutcome {
        status: JobOutcomeStatus::Killed,
        detail: None,
        output: None,
    }));
    rx.wait_for(|outcome| outcome.is_some())
        .await
        .expect("producer settles");
    disposal.await.expect("owner disposal settles");
    // The record was dropped with the owner.
    let error = harness
        .registry
        .get(&id, Some(&owner))
        .err()
        .expect("dropped");
    assert!(error.contains("unknown job"), "{error}");
}
