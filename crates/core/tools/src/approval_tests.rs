use super::*;
use dsh_agent::{
    AgentCancelCause, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxNotifications,
    InboxTarget,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionStore, UserMessage, session_id};
use dsh_user_approval::{ApprovalOutcome, ApprovalService};
use std::sync::atomic::{AtomicUsize, Ordering};
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

async fn agent(ctx: &Context, name: &str) -> Arc<dyn Agent> {
    let store = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .map(|slot| slot.as_ref().clone())
        .unwrap_or_else(|| SessionStore::install(ctx));
    let id = session_id(name);
    let session = store
        .create(ctx, Some(id.clone()), None)
        .await
        .expect("session");
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("open turn");
    let inbox = Inbox::new(&session, InboxNotifications::default()).expect("inbox");
    Arc::new(TestAgent {
        id,
        options: AgentOptions::default(),
        session,
        inbox,
        ctx: ctx.clone(),
        scope_key: ScopeKey::new(),
    })
}

async fn execute_approval(
    outcome: Option<ApprovalOutcome>,
) -> (Arc<ToolExecutionResult>, usize, Vec<String>) {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    ApprovalService::install(&ctx, Default::default());
    let tools = ToolRuntime::install(&ctx, Config::default()).unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let body_runs = runs.clone();
    tools.register(&ctx, ToolDefinition {
        name: "approval-probe".into(), description: "Record a single approved execution".into(),
        parameters: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        output: ToolOutputDefinition { schema: serde_json::json!({"type":"boolean"}), render: Arc::new(|_,_| Ok(vec![])), presentation_meta: None },
        timeout_ms: None, is_concurrency_safe: None,
        execute: Arc::new(move |_,_| { body_runs.fetch_add(1, Ordering::SeqCst); Box::pin(async { Ok(serde_json::json!(true)) }) }),
        finalize_content: None, present_call: None, present_result: None,
    }).unwrap();
    let gate: Arc<cordis::Listener> = Arc::new(|_, _| {
        Box::pin(async {
            Some(arc(PreToolDecision::Ask {
                reason: Some("Computer Use 将操作隔离浏览器或桌面，需要用户确认".into()),
                grant_key: None,
                rememberable: false,
            }))
        })
    });
    ctx.events.register(
        &ctx,
        "approval probe policy",
        "tools/pre-execute",
        gate,
        &cordis::EventOptions::default().global(true),
    );
    let answer: Arc<cordis::Listener> = Arc::new(move |_, _| {
        Box::pin(async move {
            match outcome {
                Some(outcome) => Some(arc(outcome)),
                None => futures::future::pending().await,
            }
        })
    });
    ctx.events.register(
        &ctx,
        "approval probe answer",
        "approval/request",
        answer,
        &cordis::EventOptions::default().global(true),
    );
    let owner = agent(&ctx, "approval-tool-probe").await;
    let result = tools
        .execute(ToolExecutionInput {
            call_id: dsh_llm::call_id("approval-probe-call"),
            root_call_id: None,
            name: "approval-probe".into(),
            arguments: serde_json::json!({}),
            agent: Some(owner.clone()),
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;
    let outcomes = owner
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "approval/decided")
        .map(|event| event.data["outcome"].as_str().unwrap().to_string())
        .collect();
    (result, runs.load(Ordering::SeqCst), outcomes)
}

#[tokio::test(start_paused = true)]
async fn approval_deadline_returns_expired_request_instructions_without_executing_the_tool() {
    let started = tokio::time::Instant::now();
    let (result, runs, outcomes) = execute_approval(None).await;
    assert!(
        tokio::time::Instant::now().duration_since(started) >= std::time::Duration::from_secs(30)
    );
    assert_eq!(runs, 0);
    assert_eq!(outcomes, ["timed-out"]);
    let failure = result.error.as_ref().unwrap();
    assert_eq!(
        failure.info.as_ref().unwrap().code,
        "USER_APPROVAL_TIMED_OUT"
    );
    assert!(failure.message.contains("request has ended"));
    assert!(failure.message.contains("new tool call"));
    assert!(!failure.message.contains("需要用户确认"));
    assert!(
        matches!(&result.content[0], ContentBlock::Text { text } if text.contains("controls are no longer active"))
    );
}

#[tokio::test]
async fn approval_settlement_preserves_denial_cancellation_and_unavailability() {
    for (outcome, code) in [
        (ApprovalOutcome::Rejected, "USER_APPROVAL_DENIED"),
        (ApprovalOutcome::Cancelled, "USER_APPROVAL_CANCELLED"),
        (ApprovalOutcome::Unavailable, "USER_APPROVAL_UNAVAILABLE"),
    ] {
        let (result, runs, outcomes) = execute_approval(Some(outcome)).await;
        assert_eq!(runs, 0);
        assert_eq!(outcomes, [outcome.as_str()]);
        let failure = result.error.as_ref().unwrap();
        assert_eq!(failure.info.as_ref().unwrap().code, code);
        assert!(failure.message.contains("request has ended"));
        assert!(!failure.message.contains("需要用户确认"));
    }
}

#[tokio::test]
async fn approval_single_grant_still_dispatches_once() {
    let (result, runs, outcomes) = execute_approval(Some(ApprovalOutcome::AllowedOnce)).await;
    assert!(!result.is_error);
    assert_eq!(runs, 1);
    assert_eq!(outcomes, ["allowed-once"]);
}
