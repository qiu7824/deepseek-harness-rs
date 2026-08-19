//! Rust port of the core `plan-mode.spec.ts` + `invariant.spec.ts`
//! behaviors: the fold, config validation, the set state machine (idle
//! commit + narration, open-turn queue + boundary commit, cancel, noop),
//! the exit tool review flow (approve / keep planning / dismissed / no
//! channel / not-in-plan-mode / bad heading), the `/plan` command, the
//! projection unit, and the `plan/mode` payload invariant.

use std::sync::Arc;

use cordis::{Context, arc, downcast_arc};
use dsh_agent::{AgentEventDispatch, AgentPreStepPayload, PreStepDecision};
use dsh_commands::{CommandResult, CommandRuntime};
use dsh_llm::{ContentBlock, call_id};
use dsh_plan_mode::invariant::{self, PlanModeInvariantPlugin};
use dsh_plan_mode::{
    EXIT_PLAN_MODE, PlanModeConfig, PlanModeController, SetOutcome, first_heading, fold_plan_mode,
    has_open_turn, resolve_config,
};
use dsh_session::{Session, SessionId, SurfaceIntent, SurfaceOp, session_id};
use dsh_session_projection::SessionProjectionRegistry;
use dsh_system_prompt::SystemPrompt;
use dsh_tools::{ToolExecutionInput, ToolRuntime};
use dsh_user_questions::{
    AskUserQuestionAnswer, AskUserQuestionAnswerItem, AskUserQuestionRequest, UserQuestionError,
    UserQuestionProvider, UserQuestionService,
};

// ---- helpers ----

struct ProbeAgent {
    id: SessionId,
    session: Session,
    scope_key: dsh_scope::ScopeKey,
    injected: parking_lot::Mutex<Vec<dsh_session::UserMessage>>,
    steered: parking_lot::Mutex<Vec<dsh_session::UserMessage>>,
}

impl ProbeAgent {
    fn new(id: &str, session: Session) -> Arc<Self> {
        Arc::new(Self {
            id: session_id(id),
            session,
            scope_key: dsh_scope::ScopeKey::new(),
            injected: parking_lot::Mutex::new(Vec::new()),
            steered: parking_lot::Mutex::new(Vec::new()),
        })
    }

    fn injected(&self) -> Vec<dsh_session::UserMessage> {
        self.injected.lock().clone()
    }

    fn steered(&self) -> Vec<dsh_session::UserMessage> {
        self.steered.lock().clone()
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

    fn steer(&self, message: dsh_session::UserMessage) {
        self.steered.lock().push(message);
    }

    fn inject(&self, message: dsh_session::UserMessage) {
        self.injected.lock().push(message);
    }
}

fn session_with_agent(id: &str) -> (Session, Arc<ProbeAgent>, Arc<dyn dsh_agent::Agent>) {
    let session = Session::create(session_id(id), None, None).expect("session");
    let probe = ProbeAgent::new(id, session.clone());
    let agent: Arc<dyn dsh_agent::Agent> = probe.clone();
    (session, probe, agent)
}

struct FakeProvider {
    answer: parking_lot::Mutex<Result<AskUserQuestionAnswer, UserQuestionError>>,
}

#[async_trait::async_trait]
impl UserQuestionProvider for FakeProvider {
    async fn ask(
        &self,
        _request: &AskUserQuestionRequest,
    ) -> Result<AskUserQuestionAnswer, UserQuestionError> {
        self.answer.lock().clone()
    }
}

fn approve_answer() -> AskUserQuestionAnswer {
    AskUserQuestionAnswer {
        answers: vec![AskUserQuestionAnswerItem {
            id: "plan-review".to_string(),
            selected: vec!["Approve".to_string()],
            custom: None,
        }],
    }
}

fn keep_planning_answer() -> AskUserQuestionAnswer {
    AskUserQuestionAnswer {
        answers: vec![AskUserQuestionAnswerItem {
            id: "plan-review".to_string(),
            selected: vec!["Keep planning".to_string()],
            custom: None,
        }],
    }
}

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

async fn setup() -> (Context, Arc<PlanModeController>) {
    let ctx = Context::root();
    let _system_prompt =
        SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("systemPrompt");
    let _tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let _questions = UserQuestionService::install(&ctx);
    let _commands = CommandRuntime::install(&ctx);
    let _projections = SessionProjectionRegistry::install(&ctx);
    let _agents = dsh_agent::AgentRegistry::install(&ctx);
    let _sessions = dsh_session::SessionStore::install(&ctx);
    let service = PlanModeController::install(
        &ctx,
        &PlanModeConfig {
            section: "PLAN GUIDANCE".to_string(),
        },
    )
    .expect("install");
    (ctx, service)
}

fn register_agent(ctx: &Context, agent: &Arc<dyn dsh_agent::Agent>) {
    let registry = ctx
        .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
        .expect("agents");
    // Enter + announce directly: the `register` effect body lands
    // asynchronously and would race the immediate exit-tool call.
    let _detach = registry.enter(agent.clone(), None).expect("enter");
    let _ = futures::executor::block_on(registry.announce(agent));
}

fn tools_of(ctx: &Context) -> Arc<ToolRuntime> {
    ctx.get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools")
}

fn questions_of(ctx: &Context) -> Arc<UserQuestionService> {
    ctx.get_typed::<Arc<UserQuestionService>>("userQuestions", false)
        .map(|slot| slot.as_ref().clone())
        .expect("userQuestions")
}

async fn fire_step(ctx: &Context, agent: &Arc<dyn dsh_agent::Agent>) -> PreStepDecision {
    let dispatch = AgentEventDispatch::new(ctx, agent.clone());
    let decision_value = dispatch
        .waterfall(
            "agent/pre-step",
            move |agent| {
                arc(AgentPreStepPayload {
                    agent: agent.clone(),
                    messages: Vec::new(),
                    turn: 1,
                    step: 1,
                })
            },
            Box::pin(async move {
                arc(PreStepDecision::Enter {
                    messages: Vec::new(),
                })
            }),
        )
        .await;
    let decision = downcast_arc::<PreStepDecision>(&decision_value)
        .expect("decision")
        .as_ref()
        .clone();
    if let PreStepDecision::Enter { messages } = &decision {
        for message in messages {
            agent
                .session()
                .append(
                    "user/message",
                    serde_json::to_value(message).expect("serialize"),
                    Some(SurfaceIntent {
                        surface_op: SurfaceOp::Append,
                        source_event_seqs: None,
                    }),
                )
                .expect("append");
        }
    }
    decision
}

fn exit_input(agent: Arc<dyn dsh_agent::Agent>, plan: &str) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id("c1"),
        root_call_id: None,
        name: EXIT_PLAN_MODE.to_string(),
        arguments: serde_json::json!({ "plan": plan }),
        agent: Some(agent),
        parent: None,
        signal: never_abort(),
    }
}

// ---- pure helpers ----

#[test]
fn folds_the_last_plan_mode_event_or_inactive_without_one() {
    let session = Session::create(session_id("fold"), None, None).expect("session");
    assert!(!fold_plan_mode(&session.events(), session.events().len()));
    session
        .append("plan/mode", serde_json::json!({ "active": true }), None)
        .expect("append");
    session
        .append("plan/mode", serde_json::json!({ "active": false }), None)
        .expect("append");
    session
        .append("plan/mode", serde_json::json!({ "active": true }), None)
        .expect("append");
    assert!(fold_plan_mode(&session.events(), session.events().len()));
    assert!(!fold_plan_mode(&session.events(), 2));
}

#[test]
fn validates_deployment_guidance_and_headings() {
    assert_eq!(
        resolve_config(&PlanModeConfig {
            section: "  guidance  ".to_string()
        })
        .expect("ok"),
        "guidance"
    );
    let blank = resolve_config(&PlanModeConfig {
        section: "   ".to_string(),
    })
    .expect_err("blank");
    assert!(blank.contains("non-empty"), "{blank}");
    assert_eq!(
        first_heading("# The Plan\nbody"),
        Some("The Plan".to_string())
    );
    assert_eq!(
        first_heading("### Deep heading"),
        Some("Deep heading".to_string())
    );
    assert_eq!(first_heading("no heading here"), None);
}

#[test]
fn tracks_open_turns() {
    let session = Session::create(session_id("turn-fold"), None, None).expect("session");
    assert!(!has_open_turn(&session.events()));
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("append");
    assert!(has_open_turn(&session.events()));
    session
        .append(
            "turn/end",
            serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("append");
    assert!(!has_open_turn(&session.events()));
}

// ---- set state machine ----

#[tokio::test(flavor = "current_thread")]
async fn commits_an_idle_selection_and_narrates_against_the_last_header() {
    let (ctx, service) = setup().await;
    let (session, probe, agent) = session_with_agent("idle-commit");
    session
        .append("request/header", serde_json::json!({ "request": 1 }), None)
        .expect("header");

    assert_eq!(service.set(&agent, true), SetOutcome::Committed);
    assert!(fold_plan_mode(&session.events(), session.events().len()));
    assert_eq!(
        service.get(&agent),
        dsh_plan_mode::PlanRead {
            active: true,
            pending: None
        }
    );
    // The narration names the switch because the last header told the other
    // mode.
    let injected = probe.injected();
    assert_eq!(injected.len(), 1);
    let text = match &injected[0].content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text"),
    };
    assert_eq!(text, "The user switched this session to plan mode.");

    // Switching back narrates the inverse — once a later header has seen the
    // active state.
    session
        .append("request/header", serde_json::json!({ "request": 2 }), None)
        .expect("header");
    assert_eq!(service.set(&agent, false), SetOutcome::Committed);
    assert_eq!(probe.injected().len(), 2);
    let text = match &probe.injected()[1].content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text"),
    };
    assert_eq!(
        text,
        "The user switched this session back to the default mode."
    );
    let _ = ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn queues_an_open_turn_selection_until_the_next_accepted_pre_step() {
    let (ctx, service) = setup().await;
    let (session, _probe, agent) = session_with_agent("open-turn-queue");
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");

    assert_eq!(service.set(&agent, true), SetOutcome::Queued);
    assert!(!fold_plan_mode(&session.events(), session.events().len()));
    assert_eq!(
        service.get(&agent),
        dsh_plan_mode::PlanRead {
            active: false,
            pending: Some(true)
        }
    );

    // An accepted pre-step commits the selection.
    let decision = fire_step(&ctx, &agent).await;
    assert!(matches!(decision, PreStepDecision::Enter { .. }));
    assert!(fold_plan_mode(&session.events(), session.events().len()));
    assert_eq!(
        service.get(&agent),
        dsh_plan_mode::PlanRead {
            active: true,
            pending: None
        }
    );

    // A repeated selection of the current state is a no-op.
    assert_eq!(service.set(&agent, true), SetOutcome::Noop);
}

#[tokio::test(flavor = "current_thread")]
async fn an_opposite_pending_selection_cancels_without_appending() {
    let (ctx, service) = setup().await;
    let (session, _probe, agent) = session_with_agent("cancel-selection");
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");

    assert_eq!(service.set(&agent, true), SetOutcome::Queued);
    // Selecting the currently-logged state again replaces the pending entry
    // with the logged value (TS 'cancelled': nothing remains to commit).
    assert_eq!(service.set(&agent, false), SetOutcome::Cancelled);
    assert_eq!(
        service.get(&agent),
        dsh_plan_mode::PlanRead {
            active: false,
            pending: Some(false)
        }
    );
    let decision = fire_step(&ctx, &agent).await;
    assert!(matches!(decision, PreStepDecision::Enter { .. }));
    // The boundary clears the matching selection without appending.
    assert_eq!(
        service.get(&agent),
        dsh_plan_mode::PlanRead {
            active: false,
            pending: None
        }
    );
    assert!(!fold_plan_mode(&session.events(), session.events().len()));
}

// ---- exit tool review flow ----

#[tokio::test(flavor = "current_thread")]
async fn approves_the_plan_and_leaves_plan_mode_at_the_next_step() {
    let (ctx, service) = setup().await;
    let provider = Arc::new(FakeProvider {
        answer: parking_lot::Mutex::new(Ok(approve_answer())),
    });
    let _ = questions_of(&ctx)
        .register_provider(provider)
        .expect("provider");
    let (session, _probe, agent) = session_with_agent("approve-review");
    register_agent(&ctx, &agent);
    service.set(&agent, true);
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");

    let result = tools_of(&ctx)
        .execute(exit_input(agent.clone(), "# The Plan\nbody"))
        .await;
    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(result.value.as_ref().expect("value")["approved"], true);
    // Still active until the next accepted pre-step commits the silent
    // selection.
    assert!(fold_plan_mode(&session.events(), session.events().len()));
    assert_eq!(
        service.get(&agent),
        dsh_plan_mode::PlanRead {
            active: true,
            pending: Some(false)
        }
    );

    let _ = fire_step(&ctx, &agent).await;
    assert!(!fold_plan_mode(&session.events(), session.events().len()));
}

#[tokio::test(flavor = "current_thread")]
async fn keep_planning_returns_the_revision_feedback() {
    let (ctx, service) = setup().await;
    let provider = Arc::new(FakeProvider {
        answer: parking_lot::Mutex::new(Ok(keep_planning_answer())),
    });
    let _ = questions_of(&ctx)
        .register_provider(provider)
        .expect("provider");
    let (_session, _probe, agent) = session_with_agent("keep-planning");
    register_agent(&ctx, &agent);
    service.set(&agent, true);

    let result = tools_of(&ctx)
        .execute(exit_input(agent.clone(), "# The Plan\nbody"))
        .await;
    assert!(result.is_error);
    let text = match &result.content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text"),
    };
    assert!(
        text.contains("The user chose to keep planning; revise the plan and present it again."),
        "{text}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_dismissed_review_tells_the_model_to_stop_and_wait() {
    let (ctx, service) = setup().await;
    let provider = Arc::new(FakeProvider {
        answer: parking_lot::Mutex::new(Err(UserQuestionError::new("ASK_ABORTED", "dismissed"))),
    });
    let _ = questions_of(&ctx)
        .register_provider(provider)
        .expect("provider");
    let (_session, _probe, agent) = session_with_agent("dismissed-review");
    register_agent(&ctx, &agent);
    service.set(&agent, true);

    let result = tools_of(&ctx)
        .execute(exit_input(agent.clone(), "# The Plan\nbody"))
        .await;
    assert!(result.is_error);
    let text = match &result.content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text"),
    };
    assert!(
        text.contains("The user dismissed the plan review to speak instead; stay in plan mode, stop here, and wait for their message."),
        "{text}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_exit_outside_plan_mode_without_a_heading_and_without_a_channel() {
    let ctx = Context::root();
    let _system_prompt =
        SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("systemPrompt");
    let _tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let service = PlanModeController::install(
        &ctx,
        &PlanModeConfig {
            section: "g".to_string(),
        },
    )
    .expect("install");
    let (_session, _probe, agent) = session_with_agent("exit-errors");

    // Not in plan mode.
    let result = tools_of(&ctx)
        .execute(exit_input(agent.clone(), "# The Plan\nbody"))
        .await;
    assert!(result.is_error);
    let text = match &result.content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text"),
    };
    assert!(text.contains("only available in plan mode"), "{text}");

    // In plan mode but a plan without a # heading.
    service.set(&agent, true);
    let result = tools_of(&ctx)
        .execute(exit_input(agent.clone(), "no heading"))
        .await;
    assert!(result.is_error);
    let text = match &result.content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text"),
    };
    assert!(
        text.contains("requires a non-empty markdown plan starting with a # heading"),
        "{text}"
    );

    // No user-questions channel composed in this context.
    let result = tools_of(&ctx)
        .execute(exit_input(agent.clone(), "# The Plan\nbody"))
        .await;
    assert!(result.is_error);
    let text = match &result.content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text"),
    };
    assert!(
        text.contains("no user-questions channel is available"),
        "{text}"
    );
}

// ---- /plan command ----

#[tokio::test(flavor = "current_thread")]
async fn the_plan_command_enters_steers_and_leaves() {
    let (ctx, _service) = setup().await;
    let commands = ctx
        .get_typed::<Arc<CommandRuntime>>("commands", false)
        .map(|slot| slot.as_ref().clone())
        .expect("commands");
    let (session, probe, agent) = session_with_agent("plan-command");

    // The command child registers through an inject fiber; poll until the
    // registry resolves it.
    let mut execution = None;
    for _ in 0..100 {
        if let Ok(Some(exec)) = commands
            .execute(&agent, "/plan write a parser", never_abort())
            .await
        {
            execution = Some(exec);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let execution = execution.expect("command registered");
    match &execution.result {
        CommandResult::Success { text, .. } => {
            assert_eq!(
                text.as_deref(),
                Some("Plan mode on. Use /plan off to leave.")
            );
        }
        other => panic!("success expected, got {other:?}"),
    }
    assert!(fold_plan_mode(&session.events(), session.events().len()));
    // The non-off message steers back to the model.
    assert_eq!(probe.steered().len(), 1);

    let execution = commands
        .execute(&agent, "/plan off", never_abort())
        .await
        .expect("execute")
        .expect("resolved");
    match &execution.result {
        CommandResult::Success { text, .. } => {
            assert_eq!(text.as_deref(), Some("Plan mode off."));
        }
        other => panic!("success expected, got {other:?}"),
    }
    assert!(!fold_plan_mode(&session.events(), session.events().len()));
}

// ---- projection unit ----

#[tokio::test(flavor = "current_thread")]
async fn projects_active_and_pending_from_the_two_event_fold() {
    let (ctx, service) = setup().await;
    let registry = ctx
        .get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
        .map(|slot| slot.as_ref().clone())
        .expect("sessionProjections");
    // The projection registry drives cells from published session events, so
    // the session must be store-attached (a detached Session never
    // publishes).
    let store = ctx
        .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
        .map(|slot| slot.as_ref().clone())
        .expect("sessions");
    let session = store
        .create(&ctx, Some(session_id("plan-projection")), None)
        .await
        .expect("create");
    let probe = ProbeAgent::new("plan-projection", session.clone());
    let agent: Arc<dyn dsh_agent::Agent> = probe.clone();
    let _ = probe;
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");

    let snapshot = |registry: &SessionProjectionRegistry, session: &Session| {
        registry
            .snapshot(session)
            .values
            .get("plan")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({ "active": false, "pending": false }))
    };
    // The projection unit registers through an inject child; poll until the
    // key lands.
    let mut landed = false;
    for _ in 0..100 {
        if registry.snapshot(&session).values.contains_key("plan") {
            landed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(landed, "the plan projection unit must land after install");
    assert_eq!(
        snapshot(&registry, &session),
        serde_json::json!({ "active": false, "pending": false })
    );

    // A queued selection records command/run before plan/mode commits.
    assert_eq!(service.set(&agent, true), SetOutcome::Queued);
    session
        .append(
            "command/run",
            serde_json::json!({ "commandId": "cmd-1", "name": "plan", "source": { "kind": "user" }, "args": " on" }),
            None,
        )
        .expect("append");
    assert_eq!(
        snapshot(&registry, &session),
        serde_json::json!({ "active": false, "pending": true })
    );

    let _ = fire_step(&ctx, &agent).await;
    assert_eq!(
        snapshot(&registry, &session),
        serde_json::json!({ "active": true, "pending": false })
    );
}

// ---- invariant ----

#[test]
fn checker_rejects_non_boolean_plan_mode_payloads() {
    let event = dsh_session::SessionEvent {
        type_: "plan/mode".to_string(),
        seq: 0,
        time: 0,
        data: serde_json::json!({ "active": "yes" }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    };
    let error = invariant::validate_event(&event).expect_err("non-boolean");
    assert!(error.contains("expected a boolean"), "{error}");
    let valid = dsh_session::SessionEvent {
        data: serde_json::json!({ "active": true }),
        ..event
    };
    invariant::validate_event(&valid).expect("boolean");
    let unrelated = dsh_session::SessionEvent {
        type_: "turn/start".to_string(),
        data: serde_json::json!({ "turn": 1 }),
        ..valid
    };
    invariant::validate_event(&unrelated).expect("unrelated");
}

#[tokio::test(flavor = "current_thread")]
async fn companion_accepts_valid_state_and_contains_invalid_payloads() {
    let ctx = Context::root();
    let store = dsh_session::SessionStore::install(&ctx);
    let _registry = dsh_invariants::InvariantRegistry::new(
        &ctx,
        dsh_invariants::InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let fiber = ctx.plugin(Arc::new(PlanModeInvariantPlugin), arc(()));
    fiber.settle().await.expect("settle");

    let session = store
        .create(&ctx, Some(session_id("plan-invariant")), None)
        .await
        .expect("create");
    session
        .append("plan/mode", serde_json::json!({ "active": true }), None)
        .expect("valid");
    // Deviation note: the TS append veto throws; this port contains
    // internal-listener panics, so the invalid payload commits and the
    // checker rejects the same shape.
    let event = session
        .append("plan/mode", serde_json::json!({ "active": "yes" }), None)
        .expect("contained");
    assert!(invariant::validate_event(&event).is_err());
}
