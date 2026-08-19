use std::sync::Arc;

use cordis::{Context, EventOptions};
use dsh_agent::{Agent, AgentOptions, AgentStatus, Inbox, InboxTarget};
use dsh_code_runtime_node::{Config as NodeConfig, NodeCodeRuntime};
use dsh_llm::ContentBlock;
use dsh_session::{Session, SessionId, UserMessage, session_id};
use dsh_subagent::{
    ResolvedSubagentStartRequest, SubagentCapabilities, SubagentError, SubagentProvider,
    SubagentResult, SubagentRun, SubagentRuntime, SubagentStartRequest, SubagentStopReason,
};
use dsh_subprocess_local::LocalSubprocessRuntime;
use dsh_workflow::{WorkflowEngine, WorkflowMeta, WorkflowStartRequest, WorkflowStopReason};
use dsh_workflow_node::{Config, NodeWorkflowEngine};
use parking_lot::Mutex;
use serde_json::json;

struct ParentAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: dsh_scope::ScopeKey,
}

impl ParentAgent {
    fn new(ctx: &Context) -> Arc<Self> {
        let id = session_id("workflow-parent");
        let session = Session::create(id.clone(), None, None).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
            ctx: ctx.clone(),
            scope_key: dsh_scope::ScopeKey::new(),
        })
    }
}

impl Agent for ParentAgent {
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
        AgentStatus::Running
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        &self.scope_key
    }
    fn cancel(&self, _: dsh_session::AgentCancelCause, _: Option<&dsh_agent::CancelOptions>) {}
    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        _: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}

struct StubRun {
    id: SessionId,
    disposed: Arc<std::sync::atomic::AtomicBool>,
}

struct BlockingRun {
    id: SessionId,
    disposed: Arc<std::sync::atomic::AtomicBool>,
    done: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl SubagentRun for BlockingRun {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn local_agent(&self) -> Option<Arc<dyn Agent>> {
        None
    }
    async fn result(&self) -> Result<SubagentResult, String> {
        while !self.disposed.load(std::sync::atomic::Ordering::Acquire) {
            self.done.notified().await;
        }
        Ok(SubagentResult {
            output: vec![],
            structured: None,
            stop_reason: SubagentStopReason::Aborted,
        })
    }
    async fn dispose(&self) -> Result<(), String> {
        self.disposed
            .store(true, std::sync::atomic::Ordering::Release);
        self.done.notify_waiters();
        Ok(())
    }
}

struct HangingDisposeRun {
    id: SessionId,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl SubagentRun for HangingDisposeRun {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn local_agent(&self) -> Option<Arc<dyn Agent>> {
        None
    }
    async fn result(&self) -> Result<SubagentResult, String> {
        self.started.notify_waiters();
        std::future::pending().await
    }
    async fn dispose(&self) -> Result<(), String> {
        std::future::pending().await
    }
}

struct HangingDisposeProvider {
    started: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl SubagentProvider for HangingDisposeProvider {
    fn name(&self) -> &str {
        "hanging-dispose"
    }
    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities::default()
    }
    fn inherits_parent_context(&self) -> bool {
        false
    }
    async fn start(
        &self,
        _: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        Ok(Arc::new(HangingDisposeRun {
            id: session_id("hanging-dispose-child"),
            started: self.started.clone(),
        }))
    }
}

#[derive(Default)]
struct BlockingProvider {
    disposed: Arc<std::sync::atomic::AtomicBool>,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl SubagentProvider for BlockingProvider {
    fn name(&self) -> &str {
        "blocking"
    }
    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities::default()
    }
    fn inherits_parent_context(&self) -> bool {
        false
    }
    async fn start(
        &self,
        _: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        self.started.notify_waiters();
        Ok(Arc::new(BlockingRun {
            id: session_id("blocking-child"),
            disposed: self.disposed.clone(),
            done: Arc::new(tokio::sync::Notify::new()),
        }))
    }
}

#[async_trait::async_trait]
impl SubagentRun for StubRun {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn local_agent(&self) -> Option<Arc<dyn Agent>> {
        None
    }
    async fn result(&self) -> Result<SubagentResult, String> {
        Ok(SubagentResult {
            output: vec![],
            structured: Some(json!({ "answer": 42 })),
            stop_reason: SubagentStopReason::Completed,
        })
    }
    async fn dispose(&self) -> Result<(), String> {
        self.disposed
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
}

#[derive(Default)]
struct StubProvider {
    starts: Mutex<Vec<SubagentStartRequest>>,
    disposed: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl SubagentProvider for StubProvider {
    fn name(&self) -> &str {
        "stub"
    }
    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities {
            output_schema: true,
            ..Default::default()
        }
    }
    fn inherits_parent_context(&self) -> bool {
        false
    }
    async fn start(
        &self,
        request: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        self.starts.lock().push(request.request);
        Ok(Arc::new(StubRun {
            id: session_id("stub-child"),
            disposed: self.disposed.clone(),
        }))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_code_runtime_calls_subagent_and_orders_lifecycle() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let _code = NodeCodeRuntime::install(&ctx, NodeConfig::default()).expect("node runtime");
    let subagents = SubagentRuntime::install(&ctx);
    let provider = Arc::new(StubProvider::default());
    subagents
        .register_provider(&ctx, provider.clone())
        .expect("provider");
    let engine = NodeWorkflowEngine::install(
        &ctx,
        Config {
            provider: "stub".to_string(),
            ..Config::default()
        },
    )
    .expect("workflow engine");

    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    for name in [
        "workflow/start",
        "workflow/phase",
        "workflow/agent-start",
        "workflow/agent-end",
        "workflow/end",
    ] {
        let events = events.clone();
        let event_name = name.to_string();
        ctx.on(
            name,
            Arc::new(move |_, _| {
                let events = events.clone();
                let event_name = event_name.clone();
                Box::pin(async move {
                    events.lock().push(event_name);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
    }

    let run = engine
        .start(WorkflowStartRequest {
            meta: WorkflowMeta {
                name: "answer".to_string(),
                description: "delegate once".to_string(),
                when_to_use: None,
                phases: Vec::new(),
            },
            script: "phase('Answering'); const child = await agent(args); return { answer: child.answer };"
                .to_string(),
            args: Some(json!({
                "prompt": "answer the question",
                "schema": {
                    "type": "object",
                    "properties": { "answer": { "type": "number" } },
                    "required": ["answer"]
                }
            })),
            parent: ParentAgent::new(&ctx),
            signal: None,
            subagent_provider: None,
            max_total_agents: None,
        })
        .expect("start");

    let result = run.result().await;
    tokio::task::yield_now().await;

    assert_eq!(result.stop_reason, WorkflowStopReason::Completed);
    assert_eq!(result.value, json!({ "answer": 42 }));
    assert_eq!(result.agents_started, 1);
    {
        let starts = provider.starts.lock();
        assert_eq!(starts.len(), 1);
        assert_eq!(
            starts[0].prompt,
            vec![ContentBlock::Text {
                text: "answer the question".to_string()
            }]
        );
        assert_eq!(
            starts[0].output_schema,
            Some(json!({
                "type": "object",
                "properties": { "answer": { "type": "number" } },
                "required": ["answer"]
            }))
        );
    }
    assert_eq!(
        events.lock().as_slice(),
        [
            "workflow/start",
            "workflow/phase",
            "workflow/agent-start",
            "workflow/agent-end",
            "workflow/end",
        ]
    );
    assert!(provider.disposed.load(std::sync::atomic::Ordering::Acquire));
    run.dispose().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispose_cancels_and_drains_an_active_child_before_returning() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let _code = NodeCodeRuntime::install(&ctx, NodeConfig::default()).expect("node runtime");
    let subagents = SubagentRuntime::install(&ctx);
    let provider = Arc::new(BlockingProvider::default());
    subagents
        .register_provider(&ctx, provider.clone())
        .expect("provider");
    let engine = NodeWorkflowEngine::install(
        &ctx,
        Config {
            provider: "blocking".to_string(),
            dispose_grace_ms: 1_000,
            ..Config::default()
        },
    )
    .expect("engine");
    let run = engine
        .start(WorkflowStartRequest {
            script: "return await agent('wait');".to_string(),
            meta: WorkflowMeta {
                name: "blocking".into(),
                description: "blocking".into(),
                when_to_use: None,
                phases: vec![],
            },
            args: None,
            subagent_provider: None,
            max_total_agents: None,
            parent: ParentAgent::new(&ctx),
            signal: None,
        })
        .expect("run");
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        provider.started.notified(),
    )
    .await
    .expect("child starts");
    tokio::time::timeout(std::time::Duration::from_secs(2), run.dispose())
        .await
        .expect("dispose bounded");
    assert!(provider.disposed.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(
        run.result().await.stop_reason,
        WorkflowStopReason::Cancelled
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        run.result().await.stop_reason,
        WorkflowStopReason::Cancelled,
        "a late workflow settlement must not overwrite dispose"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_agent_cap_is_enforced_before_starting_an_extra_child() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let _code = NodeCodeRuntime::install(&ctx, NodeConfig::default()).expect("node runtime");
    let subagents = SubagentRuntime::install(&ctx);
    let provider = Arc::new(StubProvider::default());
    subagents
        .register_provider(&ctx, provider.clone())
        .expect("provider");
    let engine = NodeWorkflowEngine::install(
        &ctx,
        Config {
            provider: "stub".into(),
            ..Config::default()
        },
    )
    .expect("engine");
    let run = engine
        .start(WorkflowStartRequest {
            script: "await agent('one'); await agent('two'); return true;".into(),
            meta: WorkflowMeta {
                name: "cap".into(),
                description: "cap".into(),
                when_to_use: None,
                phases: vec![],
            },
            args: None,
            subagent_provider: None,
            max_total_agents: Some(1),
            parent: ParentAgent::new(&ctx),
            signal: None,
        })
        .expect("run");
    let result = run.result().await;
    assert_eq!(provider.starts.lock().len(), 1);
    assert_eq!(result.stop_reason, WorkflowStopReason::Error);
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("agent cap")),
        "{result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispose_grace_bounds_a_child_whose_disposer_never_settles() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let _code = NodeCodeRuntime::install(&ctx, NodeConfig::default()).expect("node runtime");
    let subagents = SubagentRuntime::install(&ctx);
    let started = Arc::new(tokio::sync::Notify::new());
    subagents
        .register_provider(
            &ctx,
            Arc::new(HangingDisposeProvider {
                started: started.clone(),
            }),
        )
        .expect("provider");
    let engine = NodeWorkflowEngine::install(
        &ctx,
        Config {
            provider: "hanging-dispose".into(),
            dispose_grace_ms: 50,
            ..Config::default()
        },
    )
    .expect("engine");
    let run = engine
        .start(WorkflowStartRequest {
            script: "return await agent('wait');".into(),
            meta: WorkflowMeta {
                name: "bounded".into(),
                description: "bounded".into(),
                when_to_use: None,
                phases: vec![],
            },
            args: None,
            subagent_provider: None,
            max_total_agents: None,
            parent: ParentAgent::new(&ctx),
            signal: None,
        })
        .expect("run");
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .expect("child active");
    tokio::time::timeout(std::time::Duration::from_millis(750), run.dispose())
        .await
        .expect("dispose obeys grace");
    let result = tokio::time::timeout(std::time::Duration::from_millis(250), run.result())
        .await
        .expect("dispose settles result");
    assert_eq!(result.stop_reason, WorkflowStopReason::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_start_request_is_rejected_before_publication() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let _code = NodeCodeRuntime::install(&ctx, NodeConfig::default()).expect("node runtime");
    let subagents = SubagentRuntime::install(&ctx);
    subagents
        .register_provider(&ctx, Arc::new(StubProvider::default()))
        .expect("provider");
    let engine = NodeWorkflowEngine::install(
        &ctx,
        Config {
            provider: "stub".into(),
            ..Config::default()
        },
    )
    .expect("engine");
    let error = engine
        .start(WorkflowStartRequest {
            script: " ".into(),
            meta: WorkflowMeta {
                name: "invalid".into(),
                description: "invalid".into(),
                when_to_use: None,
                phases: vec![],
            },
            args: None,
            subagent_provider: None,
            max_total_agents: Some(0),
            parent: ParentAgent::new(&ctx),
            signal: None,
        })
        .err()
        .expect("invalid start rejected");
    assert!(matches!(
        error.code,
        dsh_workflow::WorkflowErrorCode::ScriptParse
            | dsh_workflow::WorkflowErrorCode::InvalidArgument
    ));
}
