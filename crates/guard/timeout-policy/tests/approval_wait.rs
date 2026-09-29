use cordis::{BoxFuture, Context, arc};
use dsh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxNotifications, InboxTarget,
};
use dsh_scope::ScopeKey;
use dsh_session::{AgentCancelCause, Session, SessionId, SessionStore, UserMessage, session_id};
use dsh_tools::{
    ToolBodyError, ToolDefinition, ToolExecutionInput, ToolOutputDefinition, ToolRuntime,
};
use dsh_user_approval::{ApprovalOutcome, ApprovalRequest, ApprovalService};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Owner {
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
}
impl Agent for Owner {
    fn id(&self) -> &SessionId {
        self.session.id()
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
        &self.scope
    }
    fn cancel(&self, _: AgentCancelCause, _: Option<&CancelOptions>) {}
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        _: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}
async fn fixture(
    approval_ms: u64,
    delay_ms: u64,
) -> (
    Context,
    Arc<ToolRuntime>,
    Arc<dyn Agent>,
    Arc<AtomicUsize>,
    Arc<AtomicBool>,
) {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    dsh_timeout_policy::apply(&ctx)().await;
    let approval = ApprovalService::install(
        &ctx,
        dsh_user_approval::Config {
            timeout_ms: Some(approval_ms),
            ..Default::default()
        },
    );
    let session = SessionStore::install(&ctx)
        .create(&ctx, Some(session_id("approval-tool")), None)
        .await
        .unwrap();
    session
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    let owner: Arc<dyn Agent> = Arc::new(Owner {
        inbox: Inbox::new(&session, InboxNotifications::default()).unwrap(),
        session,
        ctx: ctx.clone(),
        scope: ScopeKey::new(),
        options: Default::default(),
    });
    let asks = Arc::new(AtomicUsize::new(0));
    let seen = asks.clone();
    ctx.on(
        "approval/request",
        Arc::new(move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Some(arc(ApprovalOutcome::AllowedOnce))
            })
        }),
        Default::default(),
    )
    .await;
    let started = Arc::new(AtomicBool::new(false));
    let began = started.clone();
    tools
        .register(
            &ctx,
            ToolDefinition {
                name: "uu_terminal".into(),
                description: "Approval and execution clock fixture".into(),
                parameters: json!({"type":"object"}),
                output: ToolOutputDefinition {
                    schema: json!({"type":"null"}),
                    render: Arc::new(|_, _| Ok(vec![])),
                    presentation_meta: None,
                },
                timeout_ms: Some(120),
                is_concurrency_safe: None,
                finalize_content: None,
                present_call: None,
                present_result: None,
                execute: Arc::new(move |args, run| {
                    let signal = run.signal.lock().clone();
                    let approval = approval.clone();
                    let agent = run.agent.clone().unwrap();
                    let call_id = run.call_id.to_string();
                    let began = began.clone();
                    let work = args["workMs"].as_u64().unwrap_or(30);
                    Box::pin(async move {
                        let decision = approval
                            .request(&ApprovalRequest {
                                agent,
                                tool_name: "uu_terminal".into(),
                                call_id: Some(call_id),
                                reason: Some("Remote command fixture".into()),
                                grant_key: None,
                                rememberable: false,
                                signal: Some(signal.clone()),
                            })
                            .await
                            .map_err(ToolBodyError::plain)?;
                        match decision {
                            ApprovalOutcome::AllowedOnce | ApprovalOutcome::AllowedAlways => {}
                            ApprovalOutcome::TimedOut => {
                                return Err(ToolBodyError::coded(
                                    "approval ended",
                                    "ApprovalError",
                                    "USER_APPROVAL_TIMEOUT",
                                ));
                            }
                            _ => {
                                return Err(ToolBodyError::coded(
                                    "approval ended",
                                    "ApprovalError",
                                    "USER_APPROVAL_CANCELLED",
                                ));
                            }
                        }
                        began.store(true, Ordering::SeqCst);
                        let start = tokio::time::Instant::now();
                        while start.elapsed() < Duration::from_millis(work) {
                            if signal() {
                                return Err(ToolBodyError::coded(
                                    "execution cancelled",
                                    "Cancelled",
                                    "TOOL_ABORTED",
                                ));
                            }
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                        Ok(Value::Null)
                    })
                }),
            },
        )
        .unwrap();
    (ctx, tools, owner, asks, started)
}
async fn run(
    tools: &Arc<ToolRuntime>,
    owner: &Arc<dyn Agent>,
    args: Value,
    signal: dsh_tools::AbortPredicate,
) -> Arc<dsh_tools::ToolExecutionResult> {
    tools
        .execute(ToolExecutionInput {
            name: "uu_terminal".into(),
            call_id: dsh_llm::call_id("call"),
            root_call_id: None,
            arguments: args,
            agent: Some(owner.clone()),
            parent: None,
            signal,
        })
        .await
}
fn code(result: &dsh_tools::ToolExecutionResult) -> Option<&str> {
    result
        .error
        .as_ref()
        .and_then(|e| e.info.as_ref())
        .map(|info| info.code.as_str())
}

#[tokio::test(start_paused = true)]
async fn approval_wait_longer_than_tool_budget_does_not_consume_execution_time() {
    let (_ctx, tools, owner, asks, began) = fixture(1000, 300).await;
    let start = tokio::time::Instant::now();
    let result = run(&tools, &owner, json!({"workMs":60}), Arc::new(|| false)).await;
    assert!(!result.is_error, "{:?}", result.error);
    assert!(start.elapsed() >= Duration::from_millis(360));
    assert_eq!(asks.load(Ordering::SeqCst), 1);
    assert!(began.load(Ordering::SeqCst));
}
#[tokio::test(start_paused = true)]
async fn original_short_execution_budget_still_expires_after_approval() {
    let (_ctx, tools, owner, _, began) = fixture(1000, 300).await;
    let start = tokio::time::Instant::now();
    let result = run(&tools, &owner, json!({"workMs":1000}), Arc::new(|| false)).await;
    assert_eq!(code(&result), Some("TOOL_TIMEOUT"));
    assert!(began.load(Ordering::SeqCst));
    assert!(start.elapsed() >= Duration::from_millis(420));
    assert!(start.elapsed() < Duration::from_millis(450));
}
#[tokio::test(start_paused = true)]
async fn caller_cancellation_withdraws_slow_approval_without_starting_the_operation() {
    let (_ctx, tools, owner, _, began) = fixture(1000, 900).await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        flag.store(true, Ordering::SeqCst);
    });
    let result = run(
        &tools,
        &owner,
        json!({}),
        Arc::new(move || cancelled.load(Ordering::SeqCst)),
    )
    .await;
    assert_ne!(code(&result), Some("TOOL_TIMEOUT"));
    assert!(result.is_error);
    assert!(!began.load(Ordering::SeqCst));
    assert_eq!(
        owner
            .session()
            .find_event_rev(|e| e.type_ == "approval/decided")
            .unwrap()
            .unwrap()
            .data["outcome"],
        "cancelled"
    );
}
#[tokio::test(start_paused = true)]
async fn approval_deadline_survives_and_same_turn_retry_never_opens_a_second_question() {
    let (_ctx, tools, owner, asks, began) = fixture(250, 900).await;
    let first = run(
        &tools,
        &owner,
        json!({"command":"one","workdir":"first"}),
        Arc::new(|| false),
    )
    .await;
    assert_eq!(code(&first), Some("USER_APPROVAL_TIMEOUT"));
    assert!(!began.load(Ordering::SeqCst));
    let start = tokio::time::Instant::now();
    let second = run(
        &tools,
        &owner,
        json!({"command":"different","workdir":"second"}),
        Arc::new(|| false),
    )
    .await;
    assert_eq!(code(&second), Some("USER_APPROVAL_TIMEOUT"));
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert_eq!(asks.load(Ordering::SeqCst), 1);
}
