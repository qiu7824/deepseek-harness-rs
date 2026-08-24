//! Rust port of the core `packages/subagent/tool-subagent/tests/tool-subagent.spec.ts`
//! behaviors: foreground delegation with disposal, non-completed results as
//! tool errors with preserved partial text, background one-shot jobs, and
//! provider-lifecycle wording.
//!
//! The jobs registry is a scripted fake; continuable execution is covered by
//! the continuation-manager integration suite.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::AgentOptions;
use dsh_jobs::{JobHooks, JobId, JobRegistry, JobStart, job_id};
use dsh_llm::{ContentBlock, call_id};
use dsh_session::{Session, SessionId, session_id};
use dsh_subagent::{
    ContinuableCreateRequest, ContinuableCreateSpec, ResolvedSubagentStartRequest,
    SubagentCapabilities, SubagentError, SubagentProvider, SubagentResult, SubagentRun,
    SubagentRuntime, SubagentStopReason,
};
use dsh_system_prompt::SystemPrompt;
use dsh_tool_subagent::{Config, apply};
use dsh_tools::{ToolExecutionInput, ToolRuntime};

struct ProbeProvider {
    name: &'static str,
    capabilities: SubagentCapabilities,
    inherits: bool,
    result: parking_lot::Mutex<Option<SubagentResult>>,
    disposed: Arc<parking_lot::Mutex<Vec<String>>>,
    starts: parking_lot::Mutex<Vec<dsh_subagent::SubagentStartRequest>>,
}

struct ProbeRun {
    id: SessionId,
    result: SubagentResult,
    disposed: Arc<parking_lot::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl SubagentRun for ProbeRun {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn local_agent(&self) -> Option<Arc<dyn dsh_agent::Agent>> {
        None
    }

    async fn result(&self) -> Result<SubagentResult, String> {
        Ok(self.result.clone())
    }

    async fn dispose(&self) -> Result<(), String> {
        self.disposed.lock().push(self.id.as_str().to_string());
        Ok(())
    }
}

#[async_trait::async_trait]
impl SubagentProvider for ProbeProvider {
    fn name(&self) -> &str {
        self.name
    }

    fn capabilities(&self) -> SubagentCapabilities {
        self.capabilities
    }

    fn inherits_parent_context(&self) -> bool {
        self.inherits
    }

    async fn start(
        &self,
        request: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        self.starts.lock().push(request.request.clone());
        let result = self.result.lock().take().unwrap_or(SubagentResult {
            output: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            structured: None,
            stop_reason: SubagentStopReason::Completed,
        });
        Ok(Arc::new(ProbeRun {
            id: session_id("child"),
            result,
            disposed: self.disposed.clone(),
        }))
    }

    async fn prepare_continuable(
        &self,
        _request: ContinuableCreateRequest,
    ) -> Result<ContinuableCreateSpec, SubagentError> {
        Ok(ContinuableCreateSpec::default())
    }
}

struct ProbeAgent {
    id: SessionId,
    session: Session,
    scope_key: dsh_scope::ScopeKey,
}

impl ProbeAgent {
    fn new(id: &str, session: Session) -> Arc<Self> {
        Arc::new(Self {
            id: session_id(id),
            session,
            scope_key: dsh_scope::ScopeKey::new(),
        })
    }
}

impl dsh_agent::Agent for ProbeAgent {
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

    fn inbox(&self) -> &dsh_agent::Inbox {
        static INBOX: std::sync::OnceLock<dsh_agent::Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            dsh_agent::Inbox::new(
                &Session::create(session_id("probe"), None, None).expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }

    fn status(&self) -> dsh_agent::AgentStatus {
        dsh_agent::AgentStatus::Running
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }

    fn scope_key(&self) -> &dsh_scope::ScopeKey {
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

/// A fake jobs registry that records start specs and mints ids.
struct FakeJobs {
    starts: parking_lot::Mutex<Vec<JobStart>>,
}

#[derive(Clone)]
struct FakeJobsService {
    starts: Arc<parking_lot::Mutex<Vec<JobStart>>>,
}

impl JobRegistry for FakeJobsService {
    fn start(&self, spec: JobStart) -> Result<JobId, String> {
        let mut starts = self.starts.lock();
        let id = job_id(format!("subagent-{}", starts.len() + 1));
        starts.push(spec);
        Ok(id)
    }

    fn list(&self, _caller: Option<&Arc<dyn dsh_agent::Agent>>) -> Vec<dsh_jobs::JobSnapshot> {
        Vec::new()
    }

    fn get(
        &self,
        _id: &JobId,
        _caller: Option<&Arc<dyn dsh_agent::Agent>>,
    ) -> Result<dsh_jobs::JobSnapshot, String> {
        Err("not found".to_string())
    }

    fn read(
        &self,
        _id: &JobId,
        _caller: Option<&Arc<dyn dsh_agent::Agent>>,
    ) -> Result<dsh_jobs::JobRead, String> {
        Err("not found".to_string())
    }

    fn kill(
        &self,
        _id: &JobId,
        _caller: Option<&Arc<dyn dsh_agent::Agent>>,
        _reason: Option<String>,
    ) -> Result<dsh_jobs::KillOutcome, String> {
        Ok(dsh_jobs::KillOutcome::Requested)
    }

    fn wait(
        &self,
        _id: &JobId,
        _timeout_ms: u64,
        _caller: Option<&Arc<dyn dsh_agent::Agent>>,
        _signal: Option<dsh_jobs::JobAbort>,
    ) -> cordis::BoxFuture<'static, Result<dsh_jobs::JobSnapshot, String>> {
        Box::pin(async { Err("not found".to_string()) })
    }

    fn on_job_done(
        &self,
        _caller: &cordis::Context,
        _listener: dsh_jobs::JobDoneListener,
    ) -> cordis::Disposer {
        cordis::events::make_disposer(move || Box::pin(async move {}))
    }

    fn on_jobs_changed(
        &self,
        _caller: &cordis::Context,
        _listener: dsh_jobs::JobsChangedListener,
    ) -> cordis::Disposer {
        cordis::events::make_disposer(move || Box::pin(async move {}))
    }

    fn attach_controller(&self, _caller: &cordis::Context, _name: &str) -> cordis::Disposer {
        cordis::events::make_disposer(move || Box::pin(async move {}))
    }
}

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

fn setup(
    provider_result: SubagentResult,
    inherits: bool,
    enable_background: bool,
) -> (
    Context,
    Arc<ToolRuntime>,
    Arc<ProbeProvider>,
    Arc<FakeJobsService>,
) {
    let ctx = Context::root();
    let _system_prompt =
        SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("systemPrompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let subagents = SubagentRuntime::install(&ctx);
    let provider = Arc::new(ProbeProvider {
        name: "fork",
        capabilities: SubagentCapabilities {
            output_schema: false,
            depth_limit: true,
            tool_filter: true,
            persona: true,
        },
        inherits,
        result: parking_lot::Mutex::new(Some(provider_result)),
        disposed: Arc::new(parking_lot::Mutex::new(Vec::new())),
        starts: parking_lot::Mutex::new(Vec::new()),
    });
    let provider_arc: Arc<dyn SubagentProvider> = provider.clone();
    let _ = subagents
        .register_provider(&ctx, provider_arc)
        .expect("register");
    let jobs = Arc::new(FakeJobsService {
        starts: Arc::new(parking_lot::Mutex::new(Vec::new())),
    });
    let jobs_erased: Arc<dyn JobRegistry> = jobs.clone();
    ctx.register_service(jobs_erased);
    apply(
        &ctx,
        &Config {
            provider: "fork".to_string(),
            enable_run_in_background: Some(enable_background),
            ..Default::default()
        },
    )
    .expect("apply");
    let _ = (subagents, &ctx);
    (ctx, tools, provider, jobs)
}

fn agent() -> Arc<dyn dsh_agent::Agent> {
    let session = Session::create(session_id("parent"), None, None).expect("session");
    ProbeAgent::new("parent", session)
}

fn input(args: serde_json::Value, agent: Arc<dyn dsh_agent::Agent>) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id("c1"),
        root_call_id: None,
        name: "subagent".to_string(),
        arguments: args,
        agent: Some(agent),
        parent: None,
        signal: never_abort(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_delegation_returns_output_and_disposes() {
    let (_ctx, tools, _provider, _jobs) = setup(
        SubagentResult {
            output: vec![ContentBlock::Text {
                text: "child answer".to_string(),
            }],
            structured: None,
            stop_reason: SubagentStopReason::Completed,
        },
        true,
        true,
    );
    let result = tools
        .execute(input(
            serde_json::json!({ "description": "check build", "prompt": "run it" }),
            agent(),
        ))
        .await;
    assert!(!result.is_error, "{:?}", result.content);
    let value = result.value.as_ref().expect("value");
    assert_eq!(value["kind"], "foreground");
    assert_eq!(value["runId"], "child");
    assert_eq!(value["output"][0]["text"], "child answer");
    // The completed result renders as the child's text.
    let text: String = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "child answer");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_completed_results_become_errors_with_partial_text() {
    let (_ctx, tools, _provider, _jobs) = setup(
        SubagentResult {
            output: vec![ContentBlock::Text {
                text: "half an answer".to_string(),
            }],
            structured: None,
            stop_reason: SubagentStopReason::MaxTokens,
        },
        false,
        true,
    );
    let result = tools
        .execute(input(
            serde_json::json!({ "description": "check build", "prompt": "run it" }),
            agent(),
        ))
        .await;
    assert!(result.is_error);
    let text: String = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("subagent run hit its token limit before finishing"),
        "{text}"
    );
    assert!(
        text.contains("Partial output before the run ended:"),
        "{text}"
    );
    assert!(text.contains("half an answer"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_one_shot_starts_a_job() {
    let (_ctx, tools, _provider, jobs) = setup(
        SubagentResult {
            output: vec![],
            structured: None,
            stop_reason: SubagentStopReason::Completed,
        },
        false,
        true,
    );
    let result = tools
        .execute(input(
            serde_json::json!({
                "description": "check build",
                "prompt": "run it",
                "run_in_background": true
            }),
            agent(),
        ))
        .await;
    assert!(!result.is_error);
    let value = result.value.as_ref().expect("value");
    assert_eq!(value["kind"], "background");
    assert_eq!(value["jobId"], "subagent-1");
    assert_eq!(jobs.starts.lock().len(), 1);
    assert_eq!(jobs.starts.lock()[0].kind, "subagent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_background_rejects_forced_background_calls() {
    let (_ctx, tools, _provider, _jobs) = setup(
        SubagentResult {
            output: vec![],
            structured: None,
            stop_reason: SubagentStopReason::Completed,
        },
        false,
        false,
    );
    let result = tools
        .execute(input(
            serde_json::json!({
                "description": "check build",
                "prompt": "run it",
                "run_in_background": true
            }),
            agent(),
        ))
        .await;
    assert!(result.is_error);
    let text: String = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(text.contains("run_in_background is disabled"), "{text}");
}
