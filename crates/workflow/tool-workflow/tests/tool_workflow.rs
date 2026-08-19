use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::Context;
use dsh_agent::{Agent, AgentOptions, AgentStatus, Inbox};
use dsh_llm::{ContentBlock, call_id};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, session_id};
use dsh_system_prompt::SystemPrompt;
use dsh_tools::{ToolExecutionInput, ToolRuntime};
use dsh_workflow::{
    WorkflowEngine, WorkflowMeta, WorkflowResult, WorkflowRun, WorkflowRunId, WorkflowStartRequest,
    workflow_run_id,
};
use parking_lot::Mutex;
use serde_json::json;

struct TestAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope: ScopeKey,
}

impl TestAgent {
    fn boxed() -> Arc<dyn Agent> {
        let session = Session::create(session_id("workflow-parent"), None, None).expect("session");
        Arc::new(Self {
            id: session.id().clone(),
            inbox: Inbox::new(&session, Default::default()).expect("inbox"),
            session,
            ctx: Context::root(),
            scope: ScopeKey::new(),
        })
    }
}

impl Agent for TestAgent {
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
    fn scope_key(&self) -> &ScopeKey {
        &self.scope
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
    fn send(&self, _: dsh_session::UserMessage, _: dsh_agent::InboxTarget, _: bool) {}
    fn followup(&self, _: dsh_session::UserMessage) {}
    fn steer(&self, _: dsh_session::UserMessage) {}
    fn inject(&self, _: dsh_session::UserMessage) {}
}

struct StubRun {
    id: WorkflowRunId,
    meta: WorkflowMeta,
    disposed: Arc<AtomicBool>,
}

impl WorkflowRun for StubRun {
    fn id(&self) -> &WorkflowRunId {
        &self.id
    }
    fn meta(&self) -> &WorkflowMeta {
        &self.meta
    }
    fn result(&self) -> cordis::BoxFuture<'static, WorkflowResult> {
        Box::pin(async { WorkflowResult::completed(json!({ "answer": 42 }), 2) })
    }
    fn cancel(&self, _: Option<String>) {}
    fn dispose(&self) -> cordis::BoxFuture<'static, ()> {
        let disposed = self.disposed.clone();
        Box::pin(async move {
            disposed.store(true, Ordering::Release);
        })
    }
}

struct StubEngine {
    ctx: Context,
    requests: Mutex<Vec<WorkflowStartRequest>>,
    disposed: Arc<AtomicBool>,
}

impl WorkflowEngine for StubEngine {
    fn context(&self) -> &Context {
        &self.ctx
    }
    fn start(
        &self,
        request: WorkflowStartRequest,
    ) -> Result<Arc<dyn WorkflowRun>, dsh_workflow::WorkflowError> {
        let meta = request.meta.clone();
        self.requests.lock().push(request);
        Ok(Arc::new(StubRun {
            id: workflow_run_id("run-1"),
            meta,
            disposed: self.disposed.clone(),
        }))
    }
}

#[tokio::test]
async fn workflow_tool_starts_records_returns_and_disposes_the_run() {
    let ctx = Context::root();
    let _prompt = SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let engine = Arc::new(StubEngine {
        ctx: ctx.clone(),
        requests: Mutex::new(Vec::new()),
        disposed: Arc::new(AtomicBool::new(false)),
    });
    let service: Arc<dyn WorkflowEngine> = engine.clone();
    ctx.register_service(service);
    dsh_tool_workflow::apply(&ctx).expect("install workflow tool");
    let agent = TestAgent::boxed();

    let result = tools
        .execute(ToolExecutionInput {
            call_id: call_id("workflow-call"),
            root_call_id: None,
            name: "workflow".to_string(),
            arguments: json!({
                "script": "return { answer: 42 }",
                "meta": { "name": "answer", "description": "Return answer" },
                "args": { "input": 21 }
            }),
            agent: Some(agent.clone()),
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;

    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(
        result.value,
        Some(json!({
            "runId": "run-1",
            "agentsStarted": 2,
            "result": { "answer": 42 }
        }))
    );
    let requests = engine.requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].script, "return { answer: 42 }");
    assert_eq!(requests[0].args, Some(json!({ "input": 21 })));
    assert_eq!(requests[0].parent.id(), agent.id());
    drop(requests);
    assert!(engine.disposed.load(Ordering::Acquire));
    let events = agent.session().events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "tool-workflow/run-start")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "tool-workflow/run-end")
            .count(),
        1
    );
    assert!(
        result
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if text.contains("42")))
    );
}
