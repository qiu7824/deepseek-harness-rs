//! Rust port of the core `packages/subagent/subagent/tests/service.spec.ts`
//! behaviors: provider registry lifecycle, capability-validating start,
//! start/end event pairing, one-shot run settlement, and assistant-output
//! selection.
//!
//! # Deviations
//!
//! - The `AbortSignal` seam is the shared abort predicate.
//! - Continuation operations and listings are not ported yet.

use std::sync::Arc;

use cordis::Context;
use dsh_llm::ContentBlock;
use dsh_session::{Session, SessionEvent, SessionId, session_id};
use dsh_subagent::{
    AssistantOutputFold, ResolvedSubagentStartRequest, SubagentCapabilities, SubagentError,
    SubagentProvider, SubagentResult, SubagentRun, SubagentRuntime, SubagentStartRequest,
    SubagentStopReason, final_assistant_output, settle_run,
};

/// A probe provider that returns scripted runs.
struct ProbeProvider {
    name: &'static str,
    capabilities: SubagentCapabilities,
    inherits: bool,
    start_result: parking_lot::Mutex<Option<Result<Arc<dyn SubagentRun>, SubagentError>>>,
    starts: parking_lot::Mutex<Vec<SubagentStartRequest>>,
}

impl ProbeProvider {
    fn new(name: &'static str, capabilities: SubagentCapabilities) -> Arc<Self> {
        Arc::new(Self {
            name,
            capabilities,
            inherits: false,
            start_result: parking_lot::Mutex::new(None),
            starts: parking_lot::Mutex::new(Vec::new()),
        })
    }

    fn starts(&self) -> Vec<SubagentStartRequest> {
        self.starts.lock().clone()
    }
}

/// A scripted one-shot run.
struct ProbeRun {
    id: SessionId,
    local: Option<Arc<dyn dsh_agent::Agent>>,
    result: SubagentResult,
    disposed: parking_lot::Mutex<bool>,
}

#[async_trait::async_trait]
impl SubagentRun for ProbeRun {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn local_agent(&self) -> Option<Arc<dyn dsh_agent::Agent>> {
        self.local.clone()
    }

    async fn result(&self) -> Result<SubagentResult, String> {
        Ok(self.result.clone())
    }

    async fn dispose(&self) -> Result<(), String> {
        *self.disposed.lock() = true;
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
        match self.start_result.lock().take() {
            Some(result) => result,
            None => Ok(Arc::new(ProbeRun {
                id: session_id("child"),
                local: None,
                result: SubagentResult {
                    output: vec![ContentBlock::Text {
                        text: "done".to_string(),
                    }],
                    structured: None,
                    stop_reason: SubagentStopReason::Completed,
                },
                disposed: parking_lot::Mutex::new(false),
            })),
        }
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

    fn options(&self) -> &dsh_agent::AgentOptions {
        static OPTIONS: std::sync::OnceLock<dsh_agent::AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(dsh_agent::AgentOptions::default)
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

fn parent() -> Arc<dyn dsh_agent::Agent> {
    let session = Session::create(session_id("parent"), None, None).expect("session");
    ProbeAgent::new("parent", session)
}

fn start_request() -> SubagentStartRequest {
    SubagentStartRequest {
        label: Some("check".to_string()),
        prompt: vec![ContentBlock::Text {
            text: "go".to_string(),
        }],
        parent: parent(),
        signal: Arc::new(|| false),
        agent_options: None,
        output_schema: None,
        max_depth: None,
        tool_filter: None,
        persona: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registers_providers_and_rejects_duplicates() {
    let ctx = Context::root();
    let runtime = SubagentRuntime::install(&ctx);
    let provider = ProbeProvider::new("fork", SubagentCapabilities::default());
    let disposer = runtime
        .register_provider(&ctx, provider.clone())
        .expect("register");
    assert_eq!(runtime.list(), vec!["fork"]);
    assert!(runtime.get_provider("fork").is_some());

    let duplicate = runtime.register_provider(&ctx, provider);
    let error = duplicate.err().expect("duplicate");
    assert_eq!(error.code, "DUPLICATE_PROVIDER");

    // Disposing removes the provider and blocks new starts.
    disposer().await;
    assert!(runtime.get_provider("fork").is_none());
    let error = runtime
        .start("fork", start_request())
        .await
        .err()
        .expect("no provider");
    assert_eq!(error.code, "NO_PROVIDER");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validates_capabilities_before_delegation() {
    let ctx = Context::root();
    let runtime = SubagentRuntime::install(&ctx);
    let provider = ProbeProvider::new(
        "spawn",
        SubagentCapabilities {
            output_schema: false,
            depth_limit: false,
            tool_filter: false,
            persona: false,
        },
    );
    runtime
        .register_provider(&ctx, provider.clone())
        .expect("register");

    let mut request = start_request();
    request.max_depth = Some(2);
    let error = runtime
        .start("spawn", request)
        .await
        .err()
        .expect("capability");
    assert_eq!(error.code, "UNSUPPORTED_CAPABILITY");
    assert!(provider.starts().is_empty());

    let mut request = start_request();
    request.persona = Some("strict".to_string());
    let error = runtime
        .start("spawn", request)
        .await
        .err()
        .expect("capability");
    assert_eq!(error.code, "UNSUPPORTED_CAPABILITY");
    assert!(provider.starts().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starts_a_published_run_and_emits_the_lifecycle_pair() {
    let ctx = Context::root();
    let runtime = SubagentRuntime::install(&ctx);
    let provider = ProbeProvider::new(
        "fork",
        SubagentCapabilities {
            output_schema: true,
            depth_limit: true,
            tool_filter: true,
            persona: true,
        },
    );
    runtime
        .register_provider(&ctx, provider.clone())
        .expect("register");

    // Observe start/end on the parent-scoped carrier.
    let started: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let started_for_listener = started.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let started = started_for_listener.clone();
        Box::pin(async move {
            if let Some(info) = args
                .first()
                .and_then(|value| cordis::downcast::<dsh_subagent::SubagentRunInfo>(value))
            {
                started
                    .lock()
                    .push(format!("start:{}:{}", info.provider, info.id.as_str()));
            }
            if let Some(info) = args
                .first()
                .and_then(|value| cordis::downcast::<dsh_subagent::SubagentRunEndInfo>(value))
            {
                let mut started = started.lock();
                started.push(format!("end:{:?}", info.stop_reason));
            }
            None
        })
    });
    ctx.on("subagent/start", listener.clone(), Default::default())
        .await;
    ctx.on("subagent/end", listener, Default::default()).await;

    let run = runtime.start("fork", start_request()).await.expect("start");
    assert_eq!(run.id(), &session_id("child"));
    assert_eq!(provider.starts().len(), 1);
    let delegated = &provider.starts()[0];
    assert_eq!(delegated.label.as_deref(), Some("check"));

    // The descriptor was resolved and snapshotted for the provider.
    let result = run.result().await.expect("result");
    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    let replay = run
        .result()
        .await
        .expect("lifecycle observation must not consume the business result");
    assert_eq!(replay.stop_reason, SubagentStopReason::Completed);

    // Let the terminal observer publish the end edge.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let events = started.lock().clone();
    assert!(
        events
            .iter()
            .any(|event| event.starts_with("start:fork:child")),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| event.starts_with("end:")),
        "{events:?}"
    );
}

#[test]
fn selects_the_last_non_empty_assistant_message_then_streamed_text() {
    let session = Session::create(session_id("output"), None, None).expect("session");
    let mut fold = AssistantOutputFold::default();
    assert_eq!(fold.collect(), None);
    fold.push_text("");
    assert_eq!(fold.collect(), None);
    fold.push_text("streamed");
    fold.push_text(" tail");
    assert_eq!(
        fold.collect(),
        Some(vec![ContentBlock::Text {
            text: "streamed tail".to_string()
        }])
    );
    // A non-empty assistant message replaces the streamed fallback.
    let _ = session;
    let event = SessionEvent {
        type_: "assistant/message".to_string(),
        seq: 0,
        time: 1,
        data: serde_json::json!({
            "turn": 1, "step": 1,
            "message": { "role": "assistant", "content": [{ "type": "text", "text": "final answer" }] }
        }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    };
    fold.push(&event);
    assert_eq!(
        fold.collect(),
        Some(vec![ContentBlock::Text {
            text: "final answer".to_string()
        }])
    );
    assert_eq!(
        final_assistant_output(&[event]),
        Some(vec![ContentBlock::Text {
            text: "final answer".to_string()
        }])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settles_a_run_into_the_task_outcome() {
    let completed: Arc<dyn SubagentRun> = Arc::new(ProbeRun {
        id: session_id("completed"),
        local: None,
        result: SubagentResult {
            output: vec![ContentBlock::Text {
                text: "all good".to_string(),
            }],
            structured: None,
            stop_reason: SubagentStopReason::Completed,
        },
        disposed: parking_lot::Mutex::new(false),
    });
    let outcome = settle_run(&completed).await;
    assert_eq!(outcome.status, dsh_jobs::JobOutcomeStatus::Completed);
    assert_eq!(outcome.output.as_deref(), Some("all good"));

    let aborted: Arc<dyn SubagentRun> = Arc::new(ProbeRun {
        id: session_id("aborted"),
        local: None,
        result: SubagentResult {
            output: vec![],
            structured: None,
            stop_reason: SubagentStopReason::Aborted,
        },
        disposed: parking_lot::Mutex::new(false),
    });
    let outcome = settle_run(&aborted).await;
    assert_eq!(outcome.status, dsh_jobs::JobOutcomeStatus::Killed);

    let refusal: Arc<dyn SubagentRun> = Arc::new(ProbeRun {
        id: session_id("refusal"),
        local: None,
        result: SubagentResult {
            output: vec![],
            structured: None,
            stop_reason: SubagentStopReason::Refusal,
        },
        disposed: parking_lot::Mutex::new(false),
    });
    let outcome = settle_run(&refusal).await;
    assert_eq!(outcome.status, dsh_jobs::JobOutcomeStatus::Failed);
    assert_eq!(outcome.detail.as_deref(), Some("refusal"));
}
