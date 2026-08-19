//! Rust port of the core `packages/jobs/tool-jobs/tests/tool-jobs.spec.ts`
//! behaviors: controller lifecycle, config validation, the three control
//! tools with producer output bounds, policy-stage bounding, wait clamping,
//! completion notices with wake budgets, and scoped mounts over one shared
//! registry.
//!
//! # Deviations
//!
//! - JS `Infinity` config budgets are not representable in JSON; the
//!   fractional-budget rejection covers `2.5` and `1e300` instead.
//! - `agent/inbox/claimed` is emitted through `emit_agent_event` with the
//!   typed `AgentInboxClaimedPayload`.
//! - The "exact owner after the registry is gone" case detaches the owner
//!   instead: the Rust agent registry has no service-unload seam, and the
//!   settlement path never consults the registry anyway.
//! - Listener containment warnings arrive through the `logger` service
//!   (captured by a test exporter), matching the TS `logger.warn`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::Duration;

use cordis::{
    Context, Exporter, FiberCore, Listener, LoggerLevel, NextFn, Plugin, PluginError, arc,
    downcast, downcast_arc,
};
use parking_lot::Mutex;
use tokio::sync::watch;

use dsh_agent::{
    Agent, AgentInboxClaimedPayload, AgentOptions, AgentRegistry, AgentStatus, Inbox,
    emit_agent_event,
};
use dsh_jobs::{
    JobHooks, JobOutcome, JobOutcomeStatus, JobRegistry, JobSnapshot, JobStart, JobStatus, job_id,
};
use dsh_jobs_local::LocalJobRegistry;
use dsh_llm::{ContentBlock, MessageSource, UserMessage, call_id, create_user_message};
use dsh_scope::{ScopeKey, create_scope, scope_of};
use dsh_session::{Session, SessionId, session_id};
use dsh_system_prompt::SystemPrompt;
use dsh_tool_jobs::{CompletionDelivery, Config, ToolJobsPlugin, apply, status_line};
use dsh_tools::{
    PostToolDecision, PreToolDecision, ToolCallKind, ToolCallView, ToolExecution,
    ToolExecutionInput, ToolExecutionResult, ToolRuntime,
};

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

/// Scripted producer hooks: a watch-gated outcome, a cancellation log, and an
/// optional consuming stream.
struct TestHooks {
    cancel_log: Arc<Mutex<Vec<Option<String>>>>,
    outcome: watch::Sender<Option<JobOutcome>>,
    stream: Mutex<VecDeque<String>>,
    cancel_panics: AtomicBool,
    read_panics: AtomicBool,
    /// Whether `cancel` settles the job as killed (the TS teardown test
    /// producer).
    cancel_settles: AtomicBool,
    /// Stream mode: `read_output` consumes the queue; off, it returns `None`
    /// (a final-output job whose terminal output lives on the outcome).
    stream_mode: AtomicBool,
}

impl TestHooks {
    fn new() -> (Arc<TestHooks>, watch::Receiver<Option<JobOutcome>>) {
        let (outcome, rx) = watch::channel(None);
        (
            Arc::new(TestHooks {
                cancel_log: Arc::new(Mutex::new(Vec::new())),
                outcome,
                stream: Mutex::new(VecDeque::new()),
                cancel_panics: AtomicBool::new(false),
                read_panics: AtomicBool::new(false),
                cancel_settles: AtomicBool::new(false),
                stream_mode: AtomicBool::new(false),
            }),
            rx,
        )
    }
}

impl JobHooks for TestHooks {
    fn cancel(&self, reason: Option<String>) {
        if self.cancel_panics.load(SeqCst) {
            panic!("cancel failed: {}", "cancel failed: ".repeat(100));
        }
        self.cancel_log.lock().push(reason);
        if self.cancel_settles.load(SeqCst) {
            self.outcome.send_replace(Some(JobOutcome {
                status: JobOutcomeStatus::Killed,
                detail: None,
                output: None,
            }));
        }
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
        if self.read_panics.load(SeqCst) {
            panic!("read failed: {}", "read failed: ".repeat(100));
        }
        if !self.stream_mode.load(SeqCst) {
            return None;
        }
        Some(self.stream.lock().pop_front().unwrap_or_default())
    }
}

/// A controllable producer: spec parts + hooks + settle/cancel observation.
struct Producer {
    kind: String,
    label: String,
    output_limit_bytes: Option<u64>,
    owner: Option<Arc<dyn Agent>>,
    hooks: Arc<TestHooks>,
    outcome: watch::Sender<Option<JobOutcome>>,
}

impl Producer {
    fn new(
        kind: &str,
        label: &str,
        owner: Option<Arc<dyn Agent>>,
        output_limit_bytes: Option<u64>,
        overrides: impl FnOnce(&Arc<TestHooks>),
    ) -> Self {
        let (hooks, _rx) = TestHooks::new();
        overrides(&hooks);
        let outcome = hooks.outcome.clone();
        Producer {
            kind: kind.to_string(),
            label: label.to_string(),
            output_limit_bytes,
            owner,
            hooks,
            outcome,
        }
    }

    fn start(&self, jobs: &dyn JobRegistry) -> Result<dsh_jobs::JobId, String> {
        let hooks = self.hooks.clone();
        let spec = JobStart {
            kind: self.kind.clone(),
            label: self.label.clone(),
            output_limit_bytes: self.output_limit_bytes,
            owner: self.owner.clone(),
            run: Arc::new(move || hooks.clone()),
        };
        jobs.start(spec)
    }

    fn settle(&self, outcome: JobOutcome) {
        // `send_replace` stores the value even before the settlement driver
        // subscribes (the spawned `done` task may not have polled yet).
        self.outcome.send_replace(Some(outcome));
    }

    fn cancels(&self) -> Vec<Option<String>> {
        self.hooks.cancel_log.lock().clone()
    }
}

fn completed(detail: Option<&str>, output: Option<&str>) -> JobOutcome {
    JobOutcome {
        status: JobOutcomeStatus::Completed,
        detail: detail.map(|d| d.to_string()),
        output: output.map(|o| o.to_string()),
    }
}

/// An inert agent scope fiber (the owner-cleanup effect anchor).
struct NoopPlugin;

#[async_trait::async_trait]
impl Plugin for NoopPlugin {
    async fn apply(&self, _ctx: &Context, _config: cordis::ArcValue) -> Result<(), PluginError> {
        Ok(())
    }
}

/// A fake owner agent with scriptable status and recorded delivery.
struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
    options: AgentOptions,
    status: Mutex<AgentStatus>,
    injects: Mutex<Vec<UserMessage>>,
    followups: Mutex<Vec<UserMessage>>,
    inject_panics: AtomicBool,
}

impl StubAgent {
    fn new(raw_id: &str, agent_ctx: Context, status: AgentStatus) -> Arc<Self> {
        let id = session_id(raw_id);
        let session = Session::create(id.clone(), None, None).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
            ctx: agent_ctx,
            scope_key: ScopeKey::new(),
            options: AgentOptions::default(),
            status: Mutex::new(status),
            injects: Mutex::new(Vec::new()),
            followups: Mutex::new(Vec::new()),
            inject_panics: AtomicBool::new(false),
        })
    }

    fn injects(&self) -> Vec<UserMessage> {
        self.injects.lock().clone()
    }

    fn followups(&self) -> Vec<UserMessage> {
        self.followups.lock().clone()
    }
}

impl Agent for StubAgent {
    fn id(&self) -> &SessionId {
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
        *self.status.lock()
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
    }

    fn cancel(
        &self,
        _cause: dsh_session::AgentCancelCause,
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

    fn send(&self, _message: UserMessage, _target: dsh_agent::InboxTarget, _wakeup: bool) {}

    fn followup(&self, message: UserMessage) {
        self.followups.lock().push(message);
    }

    fn steer(&self, message: UserMessage) {
        self.followups.lock().push(message);
    }

    fn inject(&self, message: UserMessage) {
        if self.inject_panics.load(SeqCst) {
            panic!("unexpected inject bug");
        }
        self.injects.lock().push(message);
    }
}

/// A fake agent with its own lifecycle fiber, registered in the registry.
async fn fake_agent(
    ctx: &Context,
    registry: &Arc<AgentRegistry>,
    raw_id: &str,
    status: AgentStatus,
) -> (
    Arc<dyn Agent>,
    Arc<StubAgent>,
    Arc<FiberCore>,
    cordis::Disposer,
) {
    let fiber = ctx.plugin(Arc::new(NoopPlugin), arc(()));
    let agent_ctx = fiber.ctx().expect("plugin ctx bound at load");
    let stub = StubAgent::new(raw_id, agent_ctx, status);
    let agent: Arc<dyn Agent> = stub.clone();
    let detach = registry.enter(agent.clone(), None).expect("enter owner");
    registry.announce(&agent).await.expect("announce owner");
    (agent, stub, fiber, detach)
}

/// The assembled world: context, services, and the tool-jobs plugin fiber.
struct World {
    ctx: Context,
    tools_fiber: Arc<FiberCore>,
    registry: Arc<AgentRegistry>,
    tools: Arc<ToolRuntime>,
    jobs: Arc<dyn JobRegistry>,
}

async fn setup_with(config: serde_json::Value) -> World {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let registry = AgentRegistry::install(&ctx);
    LocalJobRegistry::install(&ctx, dsh_jobs_local::Config::default());
    let fiber = ctx.plugin(Arc::new(ToolJobsPlugin::new()), arc(config));
    fiber
        .settle()
        .await
        .unwrap_or_else(|error| panic!("tool-jobs loads: {}", error.message()));
    let jobs = ctx
        .get_typed::<Arc<dyn JobRegistry>>("jobs", false)
        .map(|slot| slot.as_ref().clone())
        .expect("jobs");
    World {
        ctx,
        tools_fiber: fiber,
        registry,
        tools,
        jobs,
    }
}

async fn setup() -> World {
    setup_with(serde_json::json!({})).await
}

async fn tick() {
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
}

async fn call(
    world: &World,
    name: &str,
    args: serde_json::Value,
    agent: Option<Arc<dyn Agent>>,
) -> Arc<ToolExecutionResult> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call_id = call_id(format!("call-{}", COUNTER.fetch_add(1, SeqCst) + 1));
    world
        .tools
        .execute(ToolExecutionInput {
            call_id,
            root_call_id: None,
            name: name.to_string(),
            arguments: args,
            agent,
            parent: None,
            signal: never_abort(),
        })
        .await
}

fn text(result: &Arc<ToolExecutionResult>) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn texts_of(message: &UserMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

async fn settle_tasks(world: &World, owner: Arc<dyn Agent>, count: u64) {
    for _ in 0..count {
        let producer = Producer::new("bash", "sleep 60", Some(owner.clone()), None, |_| {});
        producer.start(world.jobs.as_ref()).expect("start");
        producer.settle(completed(None, None));
        tick().await;
    }
}

/// A logger exporter capturing warn records for containment assertions.
struct CaptureExporter {
    messages: Arc<Mutex<Vec<String>>>,
}

impl Exporter for CaptureExporter {
    fn default_level(&self) -> LoggerLevel {
        LoggerLevel::Warn
    }

    fn export(&self, message: &cordis::Message) {
        let text = message
            .args
            .iter()
            .filter_map(|arg| downcast::<String>(arg).cloned())
            .collect::<Vec<_>>()
            .join(" ");
        self.messages.lock().push(text);
    }
}

// ---- setup ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attaches_the_controller_on_load_and_detaches_it_with_the_fiber() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");
    world.tools_fiber.dispose().await;
    let error = producer
        .start(world.jobs.as_ref())
        .err()
        .expect("no controller");
    assert!(error.contains("no job controller"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_a_config_whose_default_wait_exceeds_the_cap() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    LocalJobRegistry::install(&ctx, dsh_jobs_local::Config::default());
    let fiber = ctx.plugin(
        Arc::new(ToolJobsPlugin::new()),
        arc(serde_json::json!({ "waitTimeoutMs": 100, "maxWaitTimeoutMs": 50 })),
    );
    let error = fiber.settle().await.err().expect("config rejected");
    assert!(
        error.to_string().contains("exceeds maxWaitTimeoutMs"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defaults_delivery_to_wakeup_and_rejects_unknown_lanes() {
    let resolved = Config::default().resolve().expect("resolve");
    assert_eq!(resolved.completion_delivery, CompletionDelivery::Wakeup);
    assert_eq!(resolved.max_consecutive_wakes, 3);
    assert_eq!(resolved.wait_timeout_ms, 30_000);
    assert_eq!(resolved.max_wait_timeout_ms, 600_000);

    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    LocalJobRegistry::install(&ctx, dsh_jobs_local::Config::default());
    let loud = ctx.plugin(
        Arc::new(ToolJobsPlugin::new()),
        arc(serde_json::json!({ "completionDelivery": "loud" })),
    );
    assert!(loud.settle().await.is_err());
    let zero = ctx.plugin(
        Arc::new(ToolJobsPlugin::new()),
        arc(serde_json::json!({ "maxConsecutiveWakes": 0 })),
    );
    assert!(zero.settle().await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_a_wake_budget_that_cannot_bound_anything() {
    for budget in [2.5, 1e300] {
        let ctx = Context::root();
        SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
        ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
        LocalJobRegistry::install(&ctx, dsh_jobs_local::Config::default());
        let fiber = ctx.plugin(
            Arc::new(ToolJobsPlugin::new()),
            arc(serde_json::json!({ "maxConsecutiveWakes": budget })),
        );
        let error = fiber.settle().await.err().expect("budget rejected");
        assert!(error.to_string().contains("maxConsecutiveWakes"), "{error}");
    }
    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    LocalJobRegistry::install(&ctx, dsh_jobs_local::Config::default());
    let fiber = ctx.plugin(
        Arc::new(ToolJobsPlugin::new()),
        arc(serde_json::json!({ "maxConsecutiveWakes": 1 })),
    );
    fiber.settle().await.expect("whole budget loads");
}

#[test]
fn renders_status_lines_with_and_without_producer_detail() {
    let base = JobSnapshot {
        id: job_id("bash-1"),
        kind: "bash".to_string(),
        label: "x".to_string(),
        output_limit_bytes: None,
        owner_session: None,
        status: JobStatus::Running,
        detail: None,
        started_at: 0,
        finished_at: None,
        reported: false,
    };
    assert_eq!(status_line(&base), "[status: running]");
    assert_eq!(
        status_line(&JobSnapshot {
            status: JobStatus::Completed,
            detail: Some("exit code: 0".to_string()),
            ..base.clone()
        }),
        "[status: completed, exit code: 0]"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applies_the_builtin_wait_bounds_with_a_bare_config() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    LocalJobRegistry::install(&ctx, dsh_jobs_local::Config::default());
    apply(&ctx, Config::default()).await.expect("apply");
    assert!(tools.get("job_output", None).is_some());
    let jobs = ctx
        .get_typed::<Arc<dyn JobRegistry>>("jobs", false)
        .map(|slot| slot.as_ref().clone())
        .expect("jobs");
    let producer = Producer::new("bash", "sleep 60", None, None, |_| {});
    producer.start(jobs.as_ref()).expect("controller attached");
}

// ---- job_output ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_a_consuming_delta_with_a_trailing_status_line() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, None, |hooks| {
        hooks.stream_mode.store(true, SeqCst);
        hooks.stream.lock().push_back("line one\n".to_string());
    });
    producer.start(world.jobs.as_ref()).expect("start");

    let first = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert!(!first.is_error);
    let first_value = first.value.as_ref().expect("value");
    assert_eq!(first_value["text"], "line one\n");
    assert_eq!(first_value["job"]["id"], "bash-1");
    assert_eq!(first_value["job"]["kind"], "bash");
    assert_eq!(first_value["job"]["label"], "sleep 60");
    assert_eq!(first_value["job"]["status"], "running");
    assert!(first_value["job"].get("ownerSession").is_none());
    assert!(first_value["job"].get("reported").is_none());
    assert_eq!(text(&first), "line one\n[status: running]");

    let second = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert_eq!(text(&second), "(no new output)\n[status: running]");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_the_final_output_of_a_settled_final_output_job() {
    let world = setup().await;
    let producer = Producer::new("subagent", "research", None, None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");
    let pending = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "subagent-1" }),
        None,
    )
    .await;
    assert_eq!(text(&pending), "(no new output)\n[status: running]");

    producer.settle(completed(Some("completed"), Some("the answer")));
    tick().await;
    let result = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "subagent-1" }),
        None,
    )
    .await;
    assert_eq!(text(&result), "the answer\n[status: completed, completed]");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applies_a_producer_limit_to_the_complete_body_and_status_result() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, Some(48), |hooks| {
        hooks.stream_mode.store(true, SeqCst);
        hooks.stream.lock().push_back("界".repeat(100));
    });
    producer.start(world.jobs.as_ref()).expect("start");

    let output = text(
        &call(
            &world,
            "job_output",
            serde_json::json!({ "job_id": "bash-1" }),
            None,
        )
        .await,
    );
    assert!(output.as_bytes().len() <= 48, "{output}");
    assert!(output.contains("[status: running]"), "{output}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preserves_empty_and_newline_terminated_output_under_a_producer_limit() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, Some(64), |hooks| {
        hooks.stream_mode.store(true, SeqCst);
        hooks.stream.lock().push_back(String::new());
        hooks.stream.lock().push_back("line\n".to_string());
    });
    producer.start(world.jobs.as_ref()).expect("start");

    let first = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert_eq!(text(&first), "(no new output)\n[status: running]");
    let second = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert_eq!(text(&second), "line\n[status: running]");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounds_post_policy_output_without_restoring_the_canonical_status_rendering() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, Some(64), |hooks| {
        hooks.stream_mode.store(true, SeqCst);
        hooks
            .stream
            .lock()
            .push_back("canonical output".to_string());
    });
    producer.start(world.jobs.as_ref()).expect("start");

    let listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let job_id = args
            .first()
            .and_then(|value| downcast::<Arc<ToolExecution>>(value))
            .and_then(|exec| exec.arguments.get("job_id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let next = downcast_arc::<NextFn>(args.last().expect("next")).expect("next");
        Box::pin(async move {
            if job_id.as_deref() == Some("bash-1") {
                return Some(arc(PostToolDecision::Accept {
                    content: Some(vec![ContentBlock::Text {
                        text: "p".repeat(1_000),
                    }]),
                    value: None,
                    additional_contexts: None,
                }));
            }
            let value = next.call().await;
            Some(value)
        })
    });
    world
        .ctx
        .on("tools/post-execute", listener, Default::default())
        .await;

    let result = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert!(text(&result).as_bytes().len() <= 64);
    assert!(text(&result).contains("[result truncated]"));
    assert!(!text(&result).contains("[status: running]"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applies_a_producer_limit_to_a_normalized_read_failure() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, Some(64), |hooks| {
        hooks.read_panics.store(true, SeqCst);
    });
    producer.start(world.jobs.as_ref()).expect("start");

    let result = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert!(result.is_error);
    assert!(text(&result).as_bytes().len() <= 64, "{}", text(&result));
    assert!(text(&result).contains("[result truncated]"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounds_pre_around_and_post_execute_policy_outcomes_and_failures() {
    let world = setup().await;
    for index in 0..5 {
        let producer = Producer::new("bash", &format!("sleep {index}"), None, Some(64), |_| {});
        producer.start(world.jobs.as_ref()).expect("start");
    }

    let pre: Arc<Listener> = Arc::new(move |_ctx, args| {
        let job_id = args
            .first()
            .and_then(|value| downcast::<Arc<ToolExecution>>(value))
            .and_then(|exec| exec.arguments.get("job_id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let next = downcast_arc::<NextFn>(args.last().expect("next")).expect("next");
        Box::pin(async move {
            match job_id.as_deref() {
                Some("bash-1") => Some(arc(PreToolDecision::Deny {
                    reason: "d".repeat(1_000),
                })),
                Some("bash-3") => panic!("pre failed: {}", "p".repeat(1_000)),
                _ => {
                    let value = next.call().await;
                    Some(value)
                }
            }
        })
    });
    world
        .ctx
        .on("tools/pre-execute", pre, Default::default())
        .await;

    let around: Arc<Listener> = Arc::new(move |_ctx, args| {
        let job_id = args
            .first()
            .and_then(|value| downcast::<Arc<ToolExecution>>(value))
            .and_then(|exec| exec.arguments.get("job_id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let next = downcast_arc::<NextFn>(args.last().expect("next")).expect("next");
        Box::pin(async move {
            match job_id.as_deref() {
                Some("bash-2") => {
                    let result = Arc::new(ToolExecutionResult {
                        content: vec![],
                        is_error: false,
                        error: None,
                        value: Some(serde_json::json!({
                            "text": "a".repeat(1_000),
                            "job": {
                                "id": "bash-2", "kind": "bash", "label": "sleep 1",
                                "status": "running", "startedAt": 0,
                            },
                        })),
                        meta: None,
                        additional_contexts: vec![],
                        concludes_turn: false,
                        canonical_token: 0,
                    });
                    Some(arc(result))
                }
                Some("bash-4") => panic!("around failed: {}", "e".repeat(1_000)),
                _ => {
                    let value = next.call().await;
                    Some(value)
                }
            }
        })
    });
    world
        .ctx
        .on("tools/execute", around, Default::default())
        .await;

    let post: Arc<Listener> = Arc::new(move |_ctx, args| {
        let job_id = args
            .first()
            .and_then(|value| downcast::<Arc<ToolExecution>>(value))
            .and_then(|exec| exec.arguments.get("job_id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let next = downcast_arc::<NextFn>(args.last().expect("next")).expect("next");
        Box::pin(async move {
            if job_id.as_deref() == Some("bash-5") {
                panic!("post failed: {}", "o".repeat(1_000));
            }
            let value = next.call().await;
            Some(value)
        })
    });
    world
        .ctx
        .on("tools/post-execute", post, Default::default())
        .await;

    let denied = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert!(denied.is_error);
    assert!(text(&denied).as_bytes().len() <= 64);
    assert!(text(&denied).contains("[result truncated]"));

    let short_circuited = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-2" }),
        None,
    )
    .await;
    assert!(!short_circuited.is_error);
    assert!(text(&short_circuited).as_bytes().len() <= 64);
    assert!(text(&short_circuited).contains("[output truncated]"));

    let failures = [
        call(
            &world,
            "job_output",
            serde_json::json!({ "job_id": "bash-3" }),
            None,
        )
        .await,
        call(
            &world,
            "job_output",
            serde_json::json!({ "job_id": "bash-4" }),
            None,
        )
        .await,
        call(
            &world,
            "job_output",
            serde_json::json!({ "job_id": "bash-5" }),
            None,
        )
        .await,
    ];
    for failure in failures {
        assert!(failure.is_error);
        assert!(text(&failure).as_bytes().len() <= 64, "{}", text(&failure));
        assert!(text(&failure).contains("[result truncated]"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_true_blocks_until_settlement_and_reports_the_terminal_state() {
    let world = setup().await;
    let producer = Producer::new("subagent", "research", None, None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");

    let pending = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "subagent-1", "wait": true }),
        None,
    );
    // Drive the wait's synchronous prefix (waiter registration) before the
    // settlement, like the TS async call reaching its first await.
    tokio::pin!(pending);
    assert!(futures::poll!(&mut pending).is_pending());
    producer.settle(completed(None, Some("done deal")));
    let result = pending.await;
    assert_eq!(text(&result), "done deal\n[status: completed]");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_true_times_out_against_the_configured_cap_and_leaves_the_job_alive() {
    let world =
        setup_with(serde_json::json!({ "waitTimeoutMs": 10, "maxWaitTimeoutMs": 20 })).await;
    let producer = Producer::new("bash", "sleep 60", None, None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");

    let result = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-1", "wait": true, "timeout_ms": 600_000 }),
        None,
    )
    .await;
    assert_eq!(text(&result), "(no new output)\n[status: running]");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_an_empty_or_unknown_job_id_as_an_errored_result() {
    let world = setup().await;
    let empty = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "" }),
        None,
    )
    .await;
    assert!(empty.is_error);
    let unknown = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-99" }),
        None,
    )
    .await;
    assert!(unknown.is_error);
    assert!(
        text(&unknown).contains("unknown job bash-99"),
        "{}",
        text(&unknown)
    );
}

// ---- job_list ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_caller_visible_jobs_and_renders_the_empty_case() {
    let world = setup().await;
    let empty = call(&world, "job_list", serde_json::json!({}), None).await;
    assert_eq!(text(&empty), "(no background jobs)");

    let (alice, _stub, _fiber, _detach) = fake_agent(
        &world.ctx,
        &world.registry,
        "sess-alice",
        AgentStatus::Running,
    )
    .await;
    let alice_job = Producer::new("bash", "pnpm test", Some(alice.clone()), None, |_| {});
    alice_job.start(world.jobs.as_ref()).expect("start");
    let unowned = Producer::new("subagent", "open research", None, None, |_| {});
    unowned.start(world.jobs.as_ref()).expect("start");
    let build = Producer::new("bash", "build", Some(alice.clone()), None, |_| {});
    build.start(world.jobs.as_ref()).expect("start");
    build.settle(completed(Some("exit code: 0"), None));
    tick().await;

    let listed = call(
        &world,
        "job_list",
        serde_json::json!({}),
        Some(alice.clone()),
    )
    .await;
    assert!(!listed.is_error);
    let listed_value = listed
        .value
        .as_ref()
        .expect("value")
        .as_array()
        .expect("array");
    assert_eq!(listed_value.len(), 3);
    assert_eq!(listed_value[0]["id"], "bash-1");
    assert_eq!(listed_value[0]["kind"], "bash");
    assert_eq!(listed_value[0]["label"], "pnpm test");
    assert_eq!(listed_value[0]["status"], "running");
    assert_eq!(listed_value[2]["id"], "bash-2");
    assert_eq!(listed_value[2]["label"], "build");
    assert_eq!(listed_value[2]["status"], "completed");
    assert_eq!(listed_value[2]["detail"], "exit code: 0");
    for job in listed_value {
        assert!(job.get("ownerSession").is_none());
        assert!(job.get("reported").is_none());
    }
    assert_eq!(
        text(&listed),
        [
            "bash-1 [bash] running — pnpm test",
            "subagent-1 [subagent] running — open research",
            "bash-2 [bash] completed — build",
        ]
        .join("\n")
    );

    let (bob, _stub, _fiber, _detach) = fake_agent(
        &world.ctx,
        &world.registry,
        "sess-bob",
        AgentStatus::Running,
    )
    .await;
    let listed = call(&world, "job_list", serde_json::json!({}), Some(bob)).await;
    assert_eq!(
        text(&listed),
        "subagent-1 [subagent] running — open research"
    );
}

// ---- job_kill ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_cancellation_with_the_forwarded_reason() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");

    let result = call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "bash-1", "reason": "superseded" }),
        None,
    )
    .await;
    assert!(!result.is_error);
    let value = result.value.as_ref().expect("value");
    assert_eq!(value["outcome"], "cancellation-requested");
    assert_eq!(value["job"]["id"], "bash-1");
    assert_eq!(value["job"]["status"], "stopping");
    assert!(value["job"].get("ownerSession").is_none());
    assert!(value["job"].get("reported").is_none());
    assert_eq!(text(&result), "requested cancellation of job bash-1");
    assert_eq!(producer.cancels(), vec![Some("superseded".to_string())]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applies_the_producer_output_limit_to_a_cancellation_acknowledgement() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, Some(8), |_| {});
    producer.start(world.jobs.as_ref()).expect("start");

    let result = call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert!(text(&result).as_bytes().len() <= 8, "{}", text(&result));
    assert_eq!(producer.cancels(), vec![None]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applies_the_producer_output_limit_to_a_normalized_cancellation_failure() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, Some(64), |hooks| {
        hooks.cancel_panics.store(true, SeqCst);
    });
    producer.start(world.jobs.as_ref()).expect("start");

    let result = call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert!(result.is_error);
    assert!(text(&result).as_bytes().len() <= 64, "{}", text(&result));
    assert!(text(&result).contains("[result truncated]"));
    let snapshot = world.jobs.get(&job_id("bash-1"), None).expect("get");
    assert_eq!(snapshot.status, JobStatus::Running);
    assert!(!snapshot.reported);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounds_single_text_post_policy_while_preserving_structured_policy_results() {
    let world = setup().await;
    for index in 0..4 {
        let producer = Producer::new("bash", &format!("sleep {index}"), None, Some(64), |_| {});
        producer.start(world.jobs.as_ref()).expect("start");
    }

    let listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let reason = args
            .first()
            .and_then(|value| downcast::<Arc<ToolExecution>>(value))
            .and_then(|exec| exec.arguments.get("reason"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let next = downcast_arc::<NextFn>(args.last().expect("next")).expect("next");
        Box::pin(async move {
            match reason.as_deref() {
                Some("replace") => Some(arc(PostToolDecision::Accept {
                    content: Some(vec![ContentBlock::Text {
                        text: "r".repeat(1_000),
                    }]),
                    value: None,
                    additional_contexts: None,
                })),
                Some("block") => Some(arc(PostToolDecision::Block {
                    feedback: vec![ContentBlock::Text {
                        text: "b".repeat(1_000),
                    }],
                    additional_contexts: None,
                })),
                Some("multi") => Some(arc(PostToolDecision::Block {
                    feedback: vec![
                        ContentBlock::Text {
                            text: "first".to_string(),
                        },
                        ContentBlock::Text {
                            text: "second".to_string(),
                        },
                    ],
                    additional_contexts: None,
                })),
                Some("reasoning") => Some(arc(PostToolDecision::Block {
                    feedback: vec![ContentBlock::Reasoning {
                        text: "policy detail".to_string(),
                    }],
                    additional_contexts: None,
                })),
                _ => {
                    let value = next.call().await;
                    Some(value)
                }
            }
        })
    });
    world
        .ctx
        .on("tools/post-execute", listener, Default::default())
        .await;

    let replaced = call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "bash-1", "reason": "replace" }),
        None,
    )
    .await;
    assert!(!replaced.is_error);
    assert!(text(&replaced).as_bytes().len() <= 64);
    assert!(text(&replaced).contains("[result truncated]"));

    let blocked = call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "bash-2", "reason": "block" }),
        None,
    )
    .await;
    assert!(blocked.is_error);
    assert!(text(&blocked).as_bytes().len() <= 64);
    assert!(text(&blocked).contains("[result truncated]"));

    let multi = call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "bash-3", "reason": "multi" }),
        None,
    )
    .await;
    assert_eq!(
        multi.content,
        vec![
            ContentBlock::Text {
                text: "first".to_string()
            },
            ContentBlock::Text {
                text: "second".to_string()
            },
        ]
    );

    let reasoning = call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "bash-4", "reason": "reasoning" }),
        None,
    )
    .await;
    assert_eq!(
        reasoning.content,
        vec![ContentBlock::Reasoning {
            text: "policy detail".to_string()
        }]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_an_already_finished_job_without_consuming_its_pending_delta() {
    let world = setup().await;
    let producer = Producer::new("bash", "sleep 60", None, None, |hooks| {
        hooks.stream_mode.store(true, SeqCst);
        hooks.stream.lock().push_back("unread tail".to_string());
    });
    producer.start(world.jobs.as_ref()).expect("start");
    producer.settle(completed(Some("exit code: 0"), None));
    tick().await;

    let killed = call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert!(!killed.is_error);
    let value = killed.value.as_ref().expect("value");
    assert_eq!(value["outcome"], "already-finished");
    assert_eq!(value["job"]["status"], "completed");
    assert_eq!(value["job"]["detail"], "exit code: 0");
    assert_eq!(
        text(&killed),
        "job bash-1 had already finished [status: completed, exit code: 0]"
    );

    let read = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "bash-1" }),
        None,
    )
    .await;
    assert_eq!(
        text(&read),
        "unread tail\n[status: completed, exit code: 0]"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_an_empty_job_id_as_an_errored_result() {
    let world = setup().await;
    let empty = call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "" }),
        None,
    )
    .await;
    assert!(empty.is_error);
}

// ---- tool-owned UI presentation ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn renders_generic_cards_for_all_three_control_tools() {
    let world = setup().await;
    let output = world.tools.get("job_output", None).expect("job_output");
    let view = (output.present_call.as_ref().expect("present"))(
        &serde_json::json!({ "job_id": "bash-1" }),
    );
    assert_eq!(
        view,
        Some(ToolCallView::Generic {
            title: "Read output from background job bash-1".to_string(),
            kind: Some(ToolCallKind::Read),
            raw_input: Some(serde_json::json!("bash-1")),
            content: None,
            locations: None,
        })
    );
    let list = world.tools.get("job_list", None).expect("job_list");
    let view = (list.present_call.as_ref().expect("present"))(&serde_json::json!({}));
    assert_eq!(
        view,
        Some(ToolCallView::Generic {
            title: "List background jobs".to_string(),
            kind: Some(ToolCallKind::Read),
            raw_input: None,
            content: None,
            locations: None,
        })
    );
    let kill = world.tools.get("job_kill", None).expect("job_kill");
    let view = (kill.present_call.as_ref().expect("present"))(
        &serde_json::json!({ "job_id": "subagent-2" }),
    );
    assert_eq!(
        view,
        Some(ToolCallView::Generic {
            title: "Kill background job subagent-2".to_string(),
            kind: Some(ToolCallKind::Execute),
            raw_input: Some(serde_json::json!("subagent-2")),
            content: None,
            locations: None,
        })
    );
}

// ---- completion notices across scoped mounts ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delivers_one_notice_from_the_owning_scope_when_two_mounts_share_the_registry() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let registry = AgentRegistry::install(&ctx);
    let jobs_holder = LocalJobRegistry::install(&ctx, dsh_jobs_local::Config::default());

    let standing_a = create_scope(&ctx, ScopeKey::new(), &Default::default());
    let standing_b = create_scope(&ctx, ScopeKey::new(), &Default::default());
    let fiber_a = standing_a
        .ctx
        .plugin(Arc::new(ToolJobsPlugin::new()), arc(serde_json::json!({})));
    fiber_a.settle().await.expect("mount a");
    let fiber_b = standing_b
        .ctx
        .plugin(Arc::new(ToolJobsPlugin::new()), arc(serde_json::json!({})));
    fiber_b.settle().await.expect("mount b");

    // The agent joins preset A exactly as `agentPresets.compose` binds it.
    let agent_key = ScopeKey::new();
    let agent_scope = create_scope(&ctx, agent_key.clone(), &Default::default());
    dsh_scope::bind_scope_parent(&agent_key, &scope_of(&standing_a.ctx).expect("scope a"));

    let stub = StubAgent::new("sess-scoped", agent_scope.ctx.clone(), AgentStatus::Running);
    let owner: Arc<dyn Agent> = stub.clone();
    registry.enter(owner.clone(), None).expect("enter owner");
    registry.announce(&owner).await.expect("announce owner");

    let producer = Producer::new("bash", "pnpm test", Some(owner), None, |_| {});
    producer.start(jobs_holder.as_ref()).expect("start");
    producer.settle(completed(Some("exit code: 0"), None));
    tick().await;

    assert_eq!(stub.injects().len(), 1);
}

// ---- completion notice delivery ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opens_a_turn_on_an_idle_owner_when_a_job_settles() {
    let world = setup().await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Idle).await;

    let producer = Producer::new("bash", "pnpm test", Some(owner), None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");
    producer.settle(completed(Some("exit code: 0"), None));
    tick().await;

    assert_eq!(stub.followups().len(), 1);
    assert!(stub.injects().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn never_wakes_an_idle_owner_under_quiet_delivery() {
    let world = setup_with(serde_json::json!({ "completionDelivery": "quiet" })).await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Idle).await;

    let producer = Producer::new("bash", "sleep 60", Some(owner), None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");
    producer.settle(completed(None, None));
    tick().await;

    assert_eq!(stub.injects().len(), 1);
    assert!(stub.followups().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn degrades_to_injection_once_the_consecutive_wake_budget_is_spent() {
    let world = setup_with(serde_json::json!({ "maxConsecutiveWakes": 2 })).await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Idle).await;

    settle_tasks(&world, owner, 3).await;
    assert_eq!(stub.followups().len(), 2);
    assert_eq!(stub.injects().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restores_the_wake_budget_when_the_owner_claims_a_user_message() {
    let world = setup_with(serde_json::json!({ "maxConsecutiveWakes": 1 })).await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Idle).await;

    settle_tasks(&world, owner.clone(), 2).await;
    assert_eq!(stub.followups().len(), 1);

    emit_agent_event(&world.ctx, &owner, "agent/inbox/claimed", |_| {
        arc(AgentInboxClaimedPayload {
            agent: owner.clone(),
            message: create_user_message(
                vec![ContentBlock::Text {
                    text: "carry on".to_string(),
                }],
                MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
            ),
            turn: 1,
        })
    });
    tick().await;
    settle_tasks(&world, owner, 1).await;
    assert_eq!(stub.followups().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn neither_wakes_nor_injects_into_an_owner_its_own_teardown_is_draining() {
    let world = setup().await;
    let (owner, stub, fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Idle).await;

    let producer = Producer::new("bash", "sleep 60", Some(owner), None, |hooks| {
        hooks.cancel_settles.store(true, SeqCst);
    });
    producer.start(world.jobs.as_ref()).expect("start");
    // Let the spawned owner-cleanup effect body register its disposer.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    // Disposal cancels and settles the owned job; waking here would spend a
    // model request on an agent the host is destroying.
    fiber.dispose().await;
    tick().await;
    assert!(stub.followups().is_empty());
    assert!(stub.injects().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn neither_wakes_nor_injects_when_the_teardown_cancel_itself_threw() {
    let world = setup().await;
    let exporter = Arc::new(CaptureExporter {
        messages: Arc::new(Mutex::new(Vec::new())),
    });
    let messages = exporter.messages.clone();
    world.ctx.logger.exporter(&world.ctx, exporter);

    let (owner, stub, fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Idle).await;
    let producer = Producer::new("bash", "broken producer", Some(owner), None, |hooks| {
        hooks.cancel_panics.store(true, SeqCst);
    });
    producer.start(world.jobs.as_ref()).expect("start");

    // The owner-cleanup disposer registers through a spawned effect body;
    // let it land before disposal (the TS ctx.effect runs its body inline).
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    // The registry force-fails the record instead of deadlocking; that path
    // settles the job too, so it must claim the report as the ordinary
    // teardown cancel does.
    fiber.dispose().await;
    tick().await;
    assert!(
        messages
            .lock()
            .iter()
            .any(|message| message.contains("work may be orphaned")),
        "{:?}",
        *messages.lock()
    );
    assert!(stub.followups().is_empty());
    assert!(stub.injects().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_the_budget_spent_when_the_owner_only_claims_plugin_notices() {
    let world = setup_with(serde_json::json!({ "maxConsecutiveWakes": 1 })).await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Idle).await;

    settle_tasks(&world, owner.clone(), 1).await;
    emit_agent_event(&world.ctx, &owner, "agent/inbox/claimed", |_| {
        arc(AgentInboxClaimedPayload {
            agent: owner.clone(),
            message: create_user_message(
                vec![ContentBlock::Text {
                    text: "background job bash-1 finished".to_string(),
                }],
                MessageSource::Plugin {
                    plugin: "tool-jobs".to_string(),
                    form: Some(dsh_llm::ContextForm::Notice),
                    sections: None,
                    summary: Some("bash".to_string()),
                    compaction_id: None,
                    source_command_id: None,
                },
            ),
            turn: 1,
        })
    });
    tick().await;
    settle_tasks(&world, owner, 1).await;
    assert_eq!(stub.followups().len(), 1);
}

// ---- completion notices ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn injects_a_notice_into_the_owning_agent_when_an_unreported_job_settles() {
    let world = setup().await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Running).await;

    let producer = Producer::new("bash", "pnpm test", Some(owner), None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");
    producer.settle(completed(Some("exit code: 0"), None));
    tick().await;

    let delivered = stub.injects();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].role, dsh_llm::Role::User);
    assert_eq!(
        delivered[0].content,
        vec![ContentBlock::Text {
            text: "background job bash-1 (bash: pnpm test) finished [status: completed, exit code: 0]. Read its output with job_output.".to_string(),
        }]
    );
    assert_eq!(
        delivered[0].source,
        MessageSource::Plugin {
            plugin: "tool-jobs".to_string(),
            form: Some(dsh_llm::ContextForm::Notice),
            sections: None,
            summary: Some("bash pnpm test [status: completed, exit code: 0]".to_string()),
            compaction_id: None,
            source_command_id: None,
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preserves_job_ids_and_collection_guidance_in_bounded_completion_notices() {
    let world = setup().await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Running).await;

    let first = Producer::new(
        "subagent",
        &"x".repeat(1_000),
        Some(owner.clone()),
        Some(61),
        |_| {},
    );
    first.start(world.jobs.as_ref()).expect("start");
    first.settle(completed(Some(&"d".repeat(1_000)), None));
    tick().await;

    let second = Producer::new(
        "subagent",
        &"x".repeat(1_000),
        Some(owner),
        Some(80),
        |_| {},
    );
    second.start(world.jobs.as_ref()).expect("start");
    second.settle(completed(Some(&"d".repeat(1_000)), None));
    tick().await;

    let delivered = stub.injects();
    assert_eq!(delivered.len(), 2);
    assert_eq!(
        texts_of(&delivered[0]),
        "background job subagent-1\nDone; job_output."
    );
    assert_eq!(
        delivered[0].source,
        MessageSource::Plugin {
            plugin: "tool-jobs".to_string(),
            form: Some(dsh_llm::ContextForm::Notice),
            sections: None,
            summary: Some(format!("subagent {}…", "x".repeat(110))),
            compaction_id: None,
            source_command_id: None,
        }
    );

    let notice = texts_of(&delivered[1]);
    assert!(notice.as_bytes().len() <= 80, "{notice}");
    assert!(
        notice.contains("background job subagent-2 (subagent: xxxx"),
        "{notice}"
    );
    assert!(
        notice.contains("[notice truncated]\nDone; job_output."),
        "{notice}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_the_complete_pty_job_id_and_collection_action_at_the_minimum_pty_limit() {
    let world = setup().await;
    for _ in 0..99 {
        let prior = Producer::new("pty-send", "x", None, None, |_| {});
        prior.start(world.jobs.as_ref()).expect("start");
        prior.settle(completed(None, None));
    }
    tick().await;

    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Running).await;

    let target = Producer::new(
        "pty-send",
        &"x".repeat(1_000),
        Some(owner),
        Some(64),
        |_| {},
    );
    target.start(world.jobs.as_ref()).expect("start");
    target.settle(completed(Some(&"d".repeat(1_000)), None));
    tick().await;

    let delivered = stub.injects();
    assert_eq!(delivered.len(), 1);
    let notice = texts_of(&delivered[0]);
    assert!(notice.as_bytes().len() <= 64, "{notice}");
    assert_eq!(
        notice,
        "background job pty-send-100\n[notice truncated]\nDone; job_output."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserves_the_collection_action_tail_when_a_producer_supplies_a_smaller_budget() {
    let world = setup().await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Running).await;

    let tiny = Producer::new(
        "pty-send",
        &"x".repeat(100),
        Some(owner.clone()),
        Some(8),
        |_| {},
    );
    tiny.start(world.jobs.as_ref()).expect("start");
    tiny.settle(completed(None, None));
    tick().await;

    let short = Producer::new("pty-send", &"x".repeat(100), Some(owner), Some(32), |_| {});
    short.start(world.jobs.as_ref()).expect("start");
    short.settle(completed(None, None));
    tick().await;

    let delivered = stub.injects();
    assert_eq!(delivered.len(), 2);
    let tiny_notice = texts_of(&delivered[0]);
    let short_notice = texts_of(&delivered[1]);
    assert!(tiny_notice.as_bytes().len() <= 8, "{tiny_notice}");
    assert_eq!(tiny_notice, "_output.");
    assert!(short_notice.as_bytes().len() <= 32, "{short_notice}");
    assert_eq!(short_notice, "background job\nDone; job_output.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suppresses_the_notice_for_a_job_the_model_already_killed() {
    let world = setup().await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Running).await;

    let producer = Producer::new("bash", "sleep 60", Some(owner.clone()), None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");
    call(
        &world,
        "job_kill",
        serde_json::json!({ "job_id": "bash-1" }),
        Some(owner),
    )
    .await;
    producer.settle(JobOutcome {
        status: JobOutcomeStatus::Killed,
        detail: None,
        output: None,
    });
    tick().await;
    assert!(stub.injects().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suppresses_the_notice_when_a_wait_returned_the_terminal_state() {
    let world = setup().await;
    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Running).await;

    let producer = Producer::new("subagent", "research", Some(owner.clone()), None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");

    let pending = call(
        &world,
        "job_output",
        serde_json::json!({ "job_id": "subagent-1", "wait": true }),
        Some(owner),
    );
    // Drive the wait's synchronous prefix so the settlement sees a waiter
    // and marks the job reported.
    tokio::pin!(pending);
    assert!(futures::poll!(&mut pending).is_pending());
    producer.settle(completed(None, Some("answer")));
    let result = pending.await;
    assert!(text(&result).contains("answer"));
    assert!(stub.injects().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drops_the_notice_for_unowned_jobs_without_throwing() {
    let world = setup().await;
    let unowned = Producer::new("bash", "sleep 60", None, None, |_| {});
    unowned.start(world.jobs.as_ref()).expect("start");
    unowned.settle(completed(None, None));
    tick().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn does_not_route_an_old_owner_completion_notice_to_a_same_session_replacement() {
    let world = setup().await;
    let (old_owner, old_stub, _fiber, detach) =
        fake_agent(&world.ctx, &world.registry, "shared", AgentStatus::Running).await;

    let producer = Producer::new("bash", "sleep 60", Some(old_owner), None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");

    // Detach the old owner; a same-id replacement is a different instance.
    (detach)().await;
    let replacement = StubAgent::new("shared", world.ctx.clone(), AgentStatus::Running);
    let replacement_arc: Arc<dyn Agent> = replacement.clone();
    world
        .registry
        .enter(replacement_arc.clone(), None)
        .expect("enter replacement");
    world
        .registry
        .announce(&replacement_arc)
        .await
        .expect("announce replacement");

    producer.settle(completed(None, None));
    tick().await;

    assert_eq!(old_stub.injects().len(), 1);
    assert!(replacement.injects().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surfaces_an_inject_failure_through_listener_containment() {
    let world = setup().await;
    let exporter = Arc::new(CaptureExporter {
        messages: Arc::new(Mutex::new(Vec::new())),
    });
    let messages = exporter.messages.clone();
    world.ctx.logger.exporter(&world.ctx, exporter);

    let (owner, stub, _fiber, _detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Running).await;
    stub.inject_panics.store(true, SeqCst);

    let producer = Producer::new("bash", "sleep 60", Some(owner), None, |_| {});
    producer.start(world.jobs.as_ref()).expect("start");
    producer.settle(completed(None, None));
    tick().await;

    assert!(
        messages
            .lock()
            .iter()
            .any(|message| message.contains("unexpected inject bug")),
        "{:?}",
        *messages.lock()
    );
    assert!(stub.injects().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_using_the_exact_owner_without_a_later_registry_lookup() {
    let world = setup().await;
    let (owner, stub, _fiber, detach) =
        fake_agent(&world.ctx, &world.registry, "sess-1", AgentStatus::Running).await;

    let p1 = Producer::new("bash", "first", Some(owner.clone()), None, |_| {});
    p1.start(world.jobs.as_ref()).expect("start");
    let p2 = Producer::new("bash", "second", Some(owner.clone()), None, |_| {});
    p2.start(world.jobs.as_ref()).expect("start");

    // Settlement must not depend on a later registry lookup: the exact owner
    // supplied at start remains the destination. (The Rust registry has no
    // service-unload seam, so the owner is detached instead.)
    (detach)().await;
    p1.settle(completed(None, None));
    p2.settle(JobOutcome {
        status: JobOutcomeStatus::Failed,
        detail: None,
        output: None,
    });
    tick().await;

    assert_eq!(stub.injects().len(), 2);
}
