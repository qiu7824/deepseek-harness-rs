use super::*;
use dsh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxNotifications, InboxTarget,
};
use dsh_scope::ScopeKey;
use dsh_session::{AgentCancelCause, Session, SessionId, SessionStore, UserMessage, session_id};
use dsh_tools::{
    ToolBodyError, ToolDefinition, ToolExecutionInput, ToolOutputDefinition, ToolRuntime,
};
use dsh_user_approval::{ApprovalOutcome, ApprovalRequest, ApprovalService};
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
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
    fn when_idle(&self) -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        _: Arc<dyn Fn() -> futures::future::BoxFuture<'static, ()> + Send + Sync>,
    ) -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}

struct Fixture {
    runtime: Arc<NodeCodeRuntime>,
    tools: Arc<ToolRuntime>,
    owner: Arc<dyn Agent>,
    asked: Arc<tokio::sync::Notify>,
    effects: Arc<AtomicUsize>,
}
async fn fixture() -> Fixture {
    fixture_with(1000, 3000).await
}
async fn fixture_with(delay_ms: u64, approval_ms: u64) -> Fixture {
    let ctx = Context::root();
    dsh_subprocess_local::LocalSubprocessRuntime::install(&ctx);
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let runtime = NodeCodeRuntime::install(&ctx, Config::default()).unwrap();
    let tools = ToolRuntime::install(
        &ctx,
        dsh_tools::Config {
            mode: Some(dsh_tools::ToolPresentationMode::Both),
            ..Default::default()
        },
    )
    .unwrap();
    dsh_timeout_policy::apply(&ctx)().await;
    let approval = ApprovalService::install(
        &ctx,
        dsh_user_approval::Config {
            timeout_ms: Some(approval_ms),
            ..Default::default()
        },
    );
    let session = SessionStore::install(&ctx)
        .create(&ctx, Some(session_id("node-human-approval")), None)
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
    let asked = Arc::new(tokio::sync::Notify::new());
    let notifier = asked.clone();
    ctx.on(
        "approval/human-request",
        Arc::new(move |_, _| {
            let notifier = notifier.clone();
            Box::pin(async move {
                notifier.notify_one();
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Some(cordis::arc(ApprovalOutcome::AllowedOnce))
            })
        }),
        Default::default(),
    )
    .await;
    let effects = Arc::new(AtomicUsize::new(0));
    let count = effects.clone();
    tools
        .register(
            &ctx,
            ToolDefinition {
                name: "file_manage".into(),
                description: "Synthetic human-approved operation".into(),
                parameters: json!({"type":"object"}),
                output: ToolOutputDefinition {
                    schema: json!({"type":"integer"}),
                    render: Arc::new(|_, _| Ok(vec![])),
                    presentation_meta: None,
                },
                timeout_ms: Some(200),
                is_concurrency_safe: None,
                finalize_content: None,
                present_call: None,
                present_result: None,
                execute: Arc::new(move |_, run| {
                    let approval = approval.clone();
                    let count = count.clone();
                    let agent = run.agent.clone().unwrap();
                    let signal = run.signal.lock().clone();
                    let call_id = run.call_id.to_string();
                    Box::pin(async move {
                        let outcome = approval
                            .request_human(&ApprovalRequest {
                                agent,
                                tool_name: "file_manage".into(),
                                call_id: Some(call_id),
                                reason: Some("Synthetic file operation".into()),
                                grant_key: None,
                                rememberable: false,
                                signal: Some(signal.clone()),
                            })
                            .await
                            .map_err(ToolBodyError::plain)?;
                        if !matches!(
                            outcome,
                            ApprovalOutcome::AllowedOnce | ApprovalOutcome::AllowedAlways
                        ) {
                            return Err(ToolBodyError::coded(
                                "approval did not permit execution",
                                "ApprovalError",
                                outcome.as_str(),
                            ));
                        }
                        if signal() {
                            return Err(ToolBodyError::plain("cancelled before operation"));
                        }
                        count.fetch_add(1, Ordering::SeqCst);
                        Ok(json!(7))
                    })
                }),
            },
        )
        .unwrap();
    Fixture {
        runtime,
        tools,
        owner,
        asked,
        effects,
    }
}

async fn execute(
    f: &Fixture,
    code: &str,
    signal: dsh_tools::AbortPredicate,
) -> Arc<dsh_tools::ToolExecutionResult> {
    f.tools.execute(ToolExecutionInput{call_id:"parent-program".into(),root_call_id:None,name:"run_code".into(),arguments:json!({"code":code,"description":"Test approved nested operation","timeoutMs":400}),agent:Some(f.owner.clone()),parent:None,signal}).await
}

#[tokio::test]
async fn node_ptc_slow_human_approval_is_excluded_from_parent_program_budget() {
    let f = fixture().await;
    let enclosing = ApprovalClock::start();
    let wall = std::time::Instant::now();
    let result = enclosing
        .scope(execute(
            &f,
            "return await tools.file_manage({});",
            Arc::new(|| false),
        ))
        .await;
    f.runtime.dispose().await;
    assert!(
        !result.is_error,
        "slow human approval must not exhaust parent budget: {:?}; effects={}",
        result.error,
        f.effects.load(Ordering::SeqCst)
    );
    assert_eq!(result.value.as_ref().unwrap()["result"], 7);
    assert_eq!(f.effects.load(Ordering::SeqCst), 1);
    assert!(
        wall.elapsed().saturating_sub(enclosing.elapsed()) >= Duration::from_millis(900),
        "spawned binding must propagate the enclosing approval clock"
    );
}

#[tokio::test]
async fn node_ptc_stop_while_awaiting_human_never_runs_the_operation() {
    let f = fixture().await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let stop = async {
        tokio::time::timeout(Duration::from_secs(5), f.asked.notified())
            .await
            .unwrap();
        cancelled.store(true, Ordering::SeqCst);
    };
    let run = execute(
        &f,
        "return await tools.file_manage({});",
        Arc::new(move || flag.load(Ordering::SeqCst)),
    );
    let (result, ()) = tokio::join!(run, stop);
    f.runtime.dispose().await;
    assert!(result.is_error);
    assert_eq!(f.effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn node_ptc_compute_budget_still_expires_after_a_slow_approval() {
    let f = fixture().await;
    let start = std::time::Instant::now();
    let result = execute(
        &f,
        "await tools.file_manage({}); while(true) {}",
        Arc::new(|| false),
    )
    .await;
    f.runtime.dispose().await;
    assert!(result.is_error);
    assert!(result.error.as_ref().unwrap().message.contains("timeout"));
    assert_eq!(f.effects.load(Ordering::SeqCst), 1);
    assert!(start.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn node_ptc_busy_loop_cannot_hide_behind_an_unawaited_approval() {
    let f = fixture().await;
    let result = execute(
        &f,
        "tools.file_manage({}); while(true) {}",
        Arc::new(|| false),
    )
    .await;
    f.runtime.dispose().await;
    assert!(result.is_error);
    assert!(result.error.as_ref().unwrap().message.contains("timeout"));
    assert_eq!(f.effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn node_ptc_approval_keeps_its_own_expiry_and_can_be_caught_by_the_program() {
    let f = fixture_with(1500, 700).await;
    let start = std::time::Instant::now();
    let result = execute(
        &f,
        "try { await tools.file_manage({}); } catch (e) { return e.message; }",
        Arc::new(|| false),
    )
    .await;
    f.runtime.dispose().await;
    assert!(!result.is_error, "{:?}", result.error);
    assert!(
        result.value.as_ref().unwrap()["result"]
            .as_str()
            .unwrap()
            .contains("approval did not permit")
    );
    assert_eq!(f.effects.load(Ordering::SeqCst), 0);
    assert!(start.elapsed() >= Duration::from_millis(700));
    assert_eq!(
        f.owner
            .session()
            .find_event_rev(|e| e.type_ == "approval/decided")
            .unwrap()
            .unwrap()
            .data["outcome"],
        "timed-out"
    );
}
