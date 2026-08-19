use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::Context;
use dsh_agent::{Agent, AgentOptions, AgentStatus, Inbox};
use dsh_llm::call_id;
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

struct ParentAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope: ScopeKey,
}

impl ParentAgent {
    fn boxed() -> Arc<dyn Agent> {
        let session = Session::create(session_id("ralph-parent"), None, None).expect("session");
        Arc::new(Self {
            id: session.id().clone(),
            inbox: Inbox::new(&session, Default::default()).expect("inbox"),
            session,
            ctx: Context::root(),
            scope: ScopeKey::new(),
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
        Box::pin(async {
            WorkflowResult::completed(
                json!({
                    "status": "complete",
                    "roundsStarted": 2,
                    "report": {
                        "status": "complete",
                        "summary": "done",
                        "evidence": ["verified"],
                        "nextSteps": [],
                        "blocker": ""
                    }
                }),
                2,
            )
        })
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
            id: workflow_run_id("ralph-run"),
            meta,
            disposed: self.disposed.clone(),
        }))
    }
}

#[tokio::test]
async fn ralph_tool_uses_fixed_workflow_policy_and_disposes_the_run() {
    let ctx = Context::root();
    let _prompt = SystemPrompt::install(&ctx, Default::default()).expect("prompt");
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let engine = Arc::new(StubEngine {
        ctx: ctx.clone(),
        requests: Mutex::new(Vec::new()),
        disposed: Arc::new(AtomicBool::new(false)),
    });
    let service: Arc<dyn WorkflowEngine> = engine.clone();
    ctx.register_service(service);
    dsh_tool_ralph::apply(
        &ctx,
        &dsh_tool_ralph::Config {
            subagent_provider: Some("spawn".to_string()),
            max_rounds: Some(4),
            max_handoff_chars: Some(1024),
            max_result_chars: Some(4096),
        },
    )
    .expect("install ralph");
    let parent = ParentAgent::boxed();

    let result = tools
        .execute(ToolExecutionInput {
            call_id: call_id("ralph-call"),
            root_call_id: None,
            name: "ralph".to_string(),
            arguments: json!({ "objective": "Finish the migration", "maxRounds": 2 }),
            agent: Some(parent.clone()),
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;

    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(result.value.as_ref().unwrap()["runId"], "ralph-run");
    assert_eq!(result.value.as_ref().unwrap()["agentsStarted"], 2);
    assert!(
        result.content[0]
            .as_text()
            .unwrap_or_default()
            .contains("2 rounds")
    );
    let requests = engine.requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].subagent_provider.as_deref(), Some("spawn"));
    assert_eq!(requests[0].max_total_agents, Some(2));
    assert_eq!(
        requests[0].args.as_ref().unwrap()["objective"],
        "Finish the migration"
    );
    assert_eq!(requests[0].args.as_ref().unwrap()["maxRounds"], 2);
    assert!(requests[0].script.contains("Fresh-agent rounds"));
    assert_eq!(requests[0].parent.id(), parent.id());
    drop(requests);
    assert!(engine.disposed.load(Ordering::Acquire));
}
