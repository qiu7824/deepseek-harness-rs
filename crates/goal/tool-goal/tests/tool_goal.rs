use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentOptions, AgentRegistry, AgentStatus, Inbox};
use dsh_goal::GoalService;
use dsh_llm::call_id;
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, SessionStore, session_id};
use dsh_system_prompt::{AssembleContext, SystemPrompt};
use dsh_tool_goal::Config;
use dsh_tools::{
    ToolDefinition, ToolExecutionInput, ToolExecutionMode, ToolOutputDefinition, ToolRuntime,
};

struct RootAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    status: AgentStatus,
    ctx: Context,
    scope_key: ScopeKey,
}

impl RootAgent {
    fn boxed(raw_id: &str) -> Arc<dyn Agent> {
        Self::with_status(raw_id, AgentStatus::Running)
    }

    fn with_status(raw_id: &str, status: AgentStatus) -> Arc<dyn Agent> {
        let id = session_id(raw_id);
        let session = Session::create(id, None, None).expect("session");
        Self::with_session_and_status(session, status)
    }

    fn with_session(session: Session) -> Arc<dyn Agent> {
        Self::with_session_and_status(session, AgentStatus::Running)
    }

    fn with_session_and_status(session: Session, status: AgentStatus) -> Arc<dyn Agent> {
        let id = session.id().clone();
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
            status,
            ctx: Context::root(),
            scope_key: ScopeKey::new(),
        })
    }
}

impl Agent for RootAgent {
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
        self.status
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

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

fn tool_input(
    name: &str,
    arguments: serde_json::Value,
    agent: Option<Arc<dyn Agent>>,
) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id(format!("call-{name}")),
        root_call_id: None,
        name: name.to_string(),
        arguments,
        agent,
        parent: None,
        signal: never_abort(),
    }
}

fn append_turn_message(agent: &Arc<dyn Agent>, turn: u64, source: serde_json::Value) {
    agent
        .session()
        .append("turn/start", serde_json::json!({ "turn": turn }), None)
        .expect("turn/start");
    agent
        .session()
        .append(
            "user/message",
            serde_json::json!({
                "id": format!("message-{turn}"),
                "content": [{ "type": "text", "text": "continue until done" }],
                "source": source
            }),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("user/message");
}

fn open_human_turn(agent: &Arc<dyn Agent>, turn: u64) {
    append_turn_message(agent, turn, serde_json::json!({ "kind": "user" }));
}

fn close_turn(agent: &Arc<dyn Agent>, turn: u64) {
    agent
        .session()
        .append(
            "turn/end",
            serde_json::json!({ "turn": turn, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("turn/end");
}

async fn execute_as(
    agents: &Arc<AgentRegistry>,
    tools: &Arc<ToolRuntime>,
    agent: Arc<dyn Agent>,
    name: &str,
    args: serde_json::Value,
) -> Arc<dsh_tools::ToolExecutionResult> {
    agents
        .with_initiator(
            agent.clone(),
            tools.execute(tool_input(name, args, Some(agent))),
        )
        .await
        .expect("initiator boundary")
}

fn placeholder_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: "pre-existing test tool".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {}
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
            render: Arc::new(|_, _| Ok(Vec::new())),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(|_, _| Box::pin(async { Ok(serde_json::json!({})) })),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn registers_three_exclusive_tools_and_guidance_then_disposes_everything() {
    let ctx = Context::root();
    let system_prompt = SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());

    let disposer = dsh_tool_goal::apply(
        &ctx,
        &Config {
            blocked_after_consecutive_rounds: Some(5),
        },
    )
    .expect("apply");

    for name in ["create_goal", "get_goal", "update_goal"] {
        assert_eq!(
            tools.get(name, None).map(|tool| tool.name.clone()),
            Some(name.to_string())
        );
        assert_eq!(
            tools.execution_mode(&tool_input(name, serde_json::json!({}), None)),
            ToolExecutionMode::Exclusive
        );
    }
    let assembly = system_prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .expect("assemble");
    let section = assembly
        .sections
        .iter()
        .find(|section| section.name == "tool:goal")
        .expect("goal guidance");
    assert!(
        section.text.contains("infer goal intent"),
        "{}",
        section.text
    );
    assert!(
        section.text.contains("at least 5 consecutive goal rounds"),
        "{}",
        section.text
    );

    disposer().await;
    assert!(tools.get("get_goal", None).is_none());
    let assembly = system_prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .expect("assemble after dispose");
    assert!(
        !assembly
            .sections
            .iter()
            .any(|section| section.name == "tool:goal")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_and_invariant_release_every_owned_registration_on_dispose() {
    let ctx = Context::root();
    let prompt = SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    dsh_invariants::InvariantRegistry::new(&ctx, Default::default());

    let tool_fiber = ctx.plugin(
        Arc::new(dsh_tool_goal::ToolGoalPlugin),
        cordis::arc(Config::default()),
    );
    tool_fiber.settle().await.expect("tool-goal plugin settles");
    for name in ["get_goal", "create_goal", "update_goal"] {
        assert!(tools.get(name, None).is_some(), "{name} is installed");
    }
    tool_fiber.dispose().await;
    for name in ["get_goal", "create_goal", "update_goal"] {
        assert!(tools.get(name, None).is_none(), "{name} is released");
    }
    let assembly = prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .expect("assemble after dispose");
    assert!(
        !assembly
            .sections
            .iter()
            .any(|section| section.name == "tool:goal")
    );

    let invariant_fiber = ctx.plugin(
        Arc::new(dsh_tool_goal::invariant::ToolGoalInvariantPlugin),
        cordis::arc(()),
    );
    invariant_fiber
        .settle()
        .await
        .expect("tool-goal invariant settles");
    let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_tool_goal::invariant::apply(&ctx)
    }));
    assert!(
        duplicate.is_err(),
        "package is reserved while plugin is live"
    );
    invariant_fiber.dispose().await;
    let disposer = dsh_tool_goal::invariant::apply(&ctx);
    disposer().await;
}

#[tokio::test(flavor = "current_thread")]
async fn failed_registration_leaves_no_partial_tools_or_prompt_section() {
    let ctx = Context::root();
    let system_prompt = SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    tools
        .register(&ctx, placeholder_tool("create_goal"))
        .expect("reserve create_goal");

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_tool_goal::apply(&ctx, &Config::default())
    }));
    let result = outcome.expect("the conflicting installation must return Err, not panic");
    assert!(result.is_err(), "the conflicting installation must reject");
    assert!(tools.get("get_goal", None).is_none(), "get_goal leaked");
    assert!(
        tools.get("update_goal", None).is_none(),
        "update_goal leaked"
    );
    assert!(
        tools.get("create_goal", None).is_some(),
        "pre-existing tool removed"
    );
    let assembly = system_prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .expect("assemble");
    assert!(
        !assembly
            .sections
            .iter()
            .any(|section| section.name == "tool:goal"),
        "guidance section leaked from the rejected install"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_section_conflict_returns_err_without_registering_tools() {
    let ctx = Context::root();
    let system_prompt = SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    let prompt_disposer = system_prompt.section(
        &ctx,
        dsh_system_prompt::PromptSection {
            name: "tool:goal".to_string(),
            order: 114.0,
            text: dsh_system_prompt::PromptText::Static("occupied".to_string()),
            complete: None,
        },
    );

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_tool_goal::apply(&ctx, &Config::default())
    }));
    let result = outcome.expect("the prompt conflict must return Err, not panic");
    assert!(result.is_err(), "the prompt conflict must reject");
    for name in ["get_goal", "create_goal", "update_goal"] {
        assert!(tools.get(name, None).is_none(), "{name} leaked");
    }

    prompt_disposer().await;
}

#[tokio::test(flavor = "current_thread")]
async fn reentrant_tool_conflict_rolls_back_every_partial_registration() {
    let ctx = Context::root();
    let system_prompt = SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());

    let claimed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let claimed_for_listener = claimed.clone();
    let tools_for_listener = tools.clone();
    let claimed_disposer = Arc::new(std::sync::Mutex::new(None));
    let disposer_for_listener = claimed_disposer.clone();
    ctx.on(
        "system-prompt/change",
        Arc::new(move |listener_ctx, _args| {
            if !claimed_for_listener.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let disposer = tools_for_listener
                    .register(listener_ctx, placeholder_tool("create_goal"))
                    .expect("reentrant create_goal claim");
                *disposer_for_listener.lock().expect("disposer lock") = Some(disposer);
            }
            Box::pin(async { None })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_tool_goal::apply(&ctx, &Config::default())
    }));
    let result = outcome.expect("the reentrant conflict must return Err, not panic");
    assert!(result.is_err(), "the reentrant conflict must reject");
    assert!(claimed.load(std::sync::atomic::Ordering::SeqCst));
    assert!(tools.get("get_goal", None).is_none(), "get_goal leaked");
    assert!(
        tools.get("update_goal", None).is_none(),
        "update_goal leaked"
    );
    assert!(
        tools.get("create_goal", None).is_some(),
        "the reentrant claimant must remain registered"
    );
    let assembly = system_prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .expect("assemble after rejected install");
    assert!(
        !assembly
            .sections
            .iter()
            .any(|section| section.name == "tool:goal"),
        "guidance section leaked from the rejected install"
    );

    let disposer = claimed_disposer.lock().expect("disposer lock").take();
    if let Some(disposer) = disposer {
        disposer().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn direct_config_rejects_values_above_the_javascript_safe_integer_limit() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());

    let error = dsh_tool_goal::apply(
        &ctx,
        &Config {
            blocked_after_consecutive_rounds: Some(9_007_199_254_740_992),
        },
    )
    .err()
    .expect("unsafe integer must reject");
    assert_eq!(
        error,
        "blockedAfterConsecutiveRounds must be a positive safe integer"
    );
    assert!(tools.get("get_goal", None).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn create_rejects_a_round_cap_above_the_javascript_safe_integer_limit() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-unsafe-cap");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);

    let result = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "create_goal",
        serde_json::json!({
            "objective": "must not be created",
            "max_goal_rounds": 9_007_199_254_740_992_u64
        }),
    )
    .await;
    assert!(result.is_error);
    let info = result
        .error
        .as_ref()
        .and_then(|error| error.info.as_ref())
        .expect("typed error");
    assert_eq!(info.name, "GoalError");
    assert_eq!(info.code, "GOAL_INVALID_MAX_ROUNDS");
    assert!(
        !agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "goal/change"),
        "an unsafe cap must not create a durable goal"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn update_rejects_a_revision_above_the_javascript_safe_integer_limit() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-unsafe-revision");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "remain unchanged".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");

    let result = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "update_goal",
        serde_json::json!({
            "goal_id": created.id.as_str(),
            "revision": 9_007_199_254_740_992_u64,
            "action": "pause"
        }),
    )
    .await;
    assert!(result.is_error);
    let info = result
        .error
        .as_ref()
        .and_then(|error| error.info.as_ref())
        .expect("typed error");
    assert_eq!(info.name, "HarnessError");
    assert_eq!(info.code, "GOAL_TOOL_INVALID_UPDATE");
    assert_eq!(
        agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "goal/change")
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn integral_json_floats_are_accepted_as_safe_integer_goal_arguments() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-integral-floats");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);

    let created = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "create_goal",
        serde_json::json!({ "objective": "accept wire numbers", "max_goal_rounds": 8.0 }),
    )
    .await;
    assert!(
        !created.is_error,
        "{:?}",
        created.error.as_ref().map(|error| &error.message)
    );
    let goal = &created.value.as_ref().expect("value")["goal"];
    assert_eq!(goal["maxGoalRounds"], 8);

    let paused = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "update_goal",
        serde_json::json!({
            "goal_id": goal["id"],
            "revision": 1.0,
            "action": "pause"
        }),
    )
    .await;
    assert!(
        !paused.is_error,
        "{:?}",
        paused.error.as_ref().map(|error| &error.message)
    );
    assert_eq!(
        paused.value.as_ref().expect("value")["goal"]["phase"],
        "paused"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn presentation_soft_fails_malformed_args_and_accepts_integral_revision_numbers() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");

    let get = tools.get("get_goal", None).expect("get_goal");
    assert!(
        (get.present_call.as_ref().expect("presentCall"))(&serde_json::json!({
            "unexpected": true
        }))
        .is_none(),
        "malformed replay args must not render a card"
    );

    let update = tools.get("update_goal", None).expect("update_goal");
    let view = (update.present_call.as_ref().expect("presentCall"))(&serde_json::json!({
        "goal_id": "goal-1",
        "revision": 2.0,
        "action": "resume"
    }))
    .expect("integral JSON revision should present");
    match view {
        dsh_tools::ToolCallView::Generic {
            title, raw_input, ..
        } => {
            assert_eq!(title, "Resume goal");
            assert_eq!(raw_input, Some(serde_json::json!("goal-1")));
        }
        other => panic!("generic view expected, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_forged_round_zero_source_cannot_complete_an_unstarted_goal() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-forged-round-zero");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "must require a real admitted round".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");
    agent
        .session()
        .append(
            "turn/end",
            serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("turn/end");
    append_turn_message(
        &agent,
        2,
        serde_json::json!({
            "kind": "goal",
            "goalId": created.id.as_str(),
            "revision": created.revision,
            "round": 0
        }),
    );

    let result = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "update_goal",
        serde_json::json!({
            "goal_id": created.id.as_str(),
            "revision": created.revision,
            "action": "complete"
        }),
    )
    .await;
    assert!(
        result.is_error,
        "a forged round zero must not complete the goal"
    );
    let info = result
        .error
        .as_ref()
        .and_then(|error| error.info.as_ref())
        .expect("typed authority error");
    assert_eq!(info.name, "HarnessError");
    assert_eq!(info.code, "GOAL_TOOL_AUTHORITY_REQUIRED");
    assert_eq!(
        goals.get(&agent).expect("get").expect("goal").phase,
        dsh_goal::GoalPhase::Active
    );
    assert_eq!(
        agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "goal/change")
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_replayed_goal_round_cannot_reuse_autonomous_completion_authority() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-replayed-round");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "reject replayed authority".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");
    agent
        .session()
        .append(
            "turn/end",
            serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("turn/end");

    let source = serde_json::json!({
        "kind": "goal",
        "goalId": created.id.as_str(),
        "revision": created.revision,
        "round": 1
    });
    append_turn_message(&agent, 2, source.clone());
    assert_eq!(
        goals
            .get(&agent)
            .expect("get admitted round")
            .expect("goal")
            .rounds_started,
        1
    );
    agent
        .session()
        .append(
            "turn/end",
            serde_json::json!({ "turn": 2, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("turn/end");
    append_turn_message(&agent, 3, source);

    let result = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "update_goal",
        serde_json::json!({
            "goal_id": created.id.as_str(),
            "revision": created.revision,
            "action": "complete"
        }),
    )
    .await;
    assert!(
        result.is_error,
        "a replayed goal round must not carry authority"
    );
    let info = result
        .error
        .as_ref()
        .and_then(|error| error.info.as_ref())
        .expect("typed authority error");
    assert_eq!(info.code, "GOAL_TOOL_AUTHORITY_REQUIRED");
    assert_eq!(
        goals.get(&agent).expect("get").expect("goal").phase,
        dsh_goal::GoalPhase::Active
    );
    assert_eq!(
        agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "goal/change")
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_admitted_goal_round_completes_with_one_wrapup_context() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-autonomous-complete");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "finish autonomously".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");
    agent
        .session()
        .append(
            "turn/end",
            serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("turn/end");
    append_turn_message(
        &agent,
        2,
        serde_json::json!({
            "kind": "goal",
            "goalId": created.id.as_str(),
            "revision": created.revision,
            "round": 1
        }),
    );
    let admitted = goals.get(&agent).expect("get").expect("goal");
    assert_eq!(admitted.rounds_started, 1);

    let result = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "update_goal",
        serde_json::json!({
            "goal_id": admitted.id.as_str(),
            "revision": admitted.revision,
            "action": "complete"
        }),
    )
    .await;
    assert!(
        !result.is_error,
        "{:?}",
        result.error.as_ref().map(|error| &error.message)
    );
    assert_eq!(
        result.value.as_ref().expect("value")["goal"]["phase"],
        "complete"
    );
    assert_eq!(result.additional_contexts.len(), 1);
    let context = &result.additional_contexts[0];
    assert_eq!(context.source.kind(), "plugin");
    let text = context
        .content
        .iter()
        .find_map(|block| match block {
            dsh_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .expect("text wrapup");
    assert!(text.contains("<goal_complete>"), "{text}");
    assert!(text.contains("\"finish autonomously\""), "{text}");
    assert!(!result.concludes_turn);
}

fn error_code(result: &dsh_tools::ToolExecutionResult) -> Option<&str> {
    result
        .error
        .as_ref()
        .and_then(|error| error.info.as_ref())
        .map(|info| info.code.as_str())
}

#[tokio::test(flavor = "current_thread")]
async fn autonomous_blocking_requires_three_admitted_rounds_and_injects_wrapup() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(
        &ctx,
        &Config {
            blocked_after_consecutive_rounds: Some(3),
        },
    )
    .expect("apply");
    let agent = RootAgent::boxed("tool-goal-block-threshold");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "wait for the credential".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");
    close_turn(&agent, 1);

    let reason = "The required credential is still unavailable.";
    for round in 1..=2 {
        let turn = round + 1;
        append_turn_message(
            &agent,
            turn,
            serde_json::json!({
                "kind": "goal",
                "goalId": created.id.as_str(),
                "revision": created.revision,
                "round": round
            }),
        );
        let result = execute_as(
            &agents,
            &tools,
            agent.clone(),
            "update_goal",
            serde_json::json!({
                "goal_id": created.id.as_str(),
                "revision": created.revision,
                "action": "blocked",
                "blocked_reason": reason
            }),
        )
        .await;
        assert_eq!(error_code(&result), Some("GOAL_TOOL_BLOCK_THRESHOLD"));
        assert_eq!(
            agent
                .session()
                .events()
                .iter()
                .filter(|event| event.type_ == "goal/change")
                .count(),
            1,
            "threshold rejection must not mutate"
        );
        close_turn(&agent, turn);
    }

    append_turn_message(
        &agent,
        4,
        serde_json::json!({
            "kind": "goal",
            "goalId": created.id.as_str(),
            "revision": created.revision,
            "round": 3
        }),
    );
    let blocked = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "update_goal",
        serde_json::json!({
            "goal_id": created.id.as_str(),
            "revision": created.revision,
            "action": "blocked",
            "blocked_reason": reason
        }),
    )
    .await;
    assert!(
        !blocked.is_error,
        "{:?}",
        blocked.error.as_ref().map(|error| &error.message)
    );
    let value = blocked.value.as_ref().expect("value");
    assert_eq!(value["goal"]["phase"], "blocked");
    assert_eq!(value["goal"]["roundsStarted"], 3);
    assert_eq!(value["goal"]["blockedReason"]["code"], "model-reported");
    assert_eq!(value["goal"]["blockedReason"]["message"], reason);
    assert_eq!(blocked.additional_contexts.len(), 1);
    let text = blocked.additional_contexts[0]
        .content
        .iter()
        .find_map(|block| match block {
            dsh_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .expect("blocked wrapup text");
    assert!(text.contains("<goal_blocked>"), "{text}");
    assert!(text.contains(reason), "{text}");
    assert!(!blocked.concludes_turn);
}

#[tokio::test(flavor = "current_thread")]
async fn direct_human_can_block_before_the_autonomous_round_threshold() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(
        &ctx,
        &Config {
            blocked_after_consecutive_rounds: Some(9),
        },
    )
    .expect("apply");
    let agent = RootAgent::boxed("tool-goal-human-block");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "wait for a human prerequisite".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");

    let result = execute_as(
        &agents,
        &tools,
        agent,
        "update_goal",
        serde_json::json!({
            "goal_id": created.id.as_str(),
            "revision": created.revision,
            "action": "blocked",
            "blocked_reason": "The user asked to wait for a prerequisite."
        }),
    )
    .await;
    assert!(
        !result.is_error,
        "{:?}",
        result.error.as_ref().map(|error| &error.message)
    );
    assert_eq!(
        result.value.as_ref().expect("value")["goal"]["phase"],
        "blocked"
    );
    assert_eq!(
        result.value.as_ref().expect("value")["goal"]["roundsStarted"],
        0
    );
    assert!(result.additional_contexts.is_empty());
    assert!(!result.concludes_turn);
}

#[tokio::test(flavor = "current_thread")]
async fn an_autonomous_goal_round_cannot_edit_but_can_complete_its_exact_goal() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-round-authority");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "round-owned".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");
    close_turn(&agent, 1);
    append_turn_message(
        &agent,
        2,
        serde_json::json!({
            "kind": "goal",
            "goalId": created.id.as_str(),
            "revision": created.revision,
            "round": 1
        }),
    );

    let edit = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "update_goal",
        serde_json::json!({
            "goal_id": created.id.as_str(),
            "revision": created.revision,
            "action": "edit",
            "objective": "forbidden"
        }),
    )
    .await;
    assert_eq!(error_code(&edit), Some("GOAL_TOOL_AUTHORITY_REQUIRED"));
    let current = goals.get(&agent).expect("get").expect("goal");
    assert_eq!(current.revision, 1);
    assert_eq!(current.phase, dsh_goal::GoalPhase::Active);
    assert_eq!(current.rounds_started, 1);

    let complete = execute_as(
        &agents,
        &tools,
        agent,
        "update_goal",
        serde_json::json!({
            "goal_id": created.id.as_str(),
            "revision": created.revision,
            "action": "complete"
        }),
    )
    .await;
    assert!(
        !complete.is_error,
        "{:?}",
        complete.error.as_ref().map(|error| &error.message)
    );
    assert_eq!(
        complete.value.as_ref().expect("value")["goal"]["phase"],
        "complete"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn durable_commit_failure_remains_a_typed_tool_infrastructure_error() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let sessions = SessionStore::install(&ctx);
    let session = sessions
        .create(&ctx, Some(session_id("tool-goal-commit-failure")), None)
        .await
        .expect("attached session");
    let agent = RootAgent::with_session(session);
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);

    let published = Arc::new(std::sync::Mutex::new(0_u32));
    let published_for_listener = published.clone();
    ctx.on(
        "goal/changed",
        Arc::new(move |_ctx, _args| {
            *published_for_listener.lock().expect("published lock") += 1;
            Box::pin(async { None })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;

    let result_slot = Arc::new(std::sync::Mutex::new(None));
    let result_for_listener = result_slot.clone();
    let agents_for_listener = agents.clone();
    let tools_for_listener = tools.clone();
    let agent_for_listener = agent.clone();
    ctx.on(
        "session/event",
        Arc::new(move |_ctx, args| {
            let is_trigger = args
                .get(1)
                .and_then(|value| cordis::downcast::<dsh_session::SessionEvent>(value))
                .is_some_and(|event| event.type_ == "tool-goal-test/trigger");
            if is_trigger {
                let input = tool_input(
                    "create_goal",
                    serde_json::json!({ "objective": "must roll back" }),
                    Some(agent_for_listener.clone()),
                );
                let result = futures::executor::block_on(agents_for_listener.with_initiator(
                    agent_for_listener.clone(),
                    tools_for_listener.execute(input),
                ))
                .expect("initiator boundary");
                *result_for_listener.lock().expect("result lock") = Some(result);
            }
            Box::pin(async { None })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;

    agent
        .session()
        .append("tool-goal-test/trigger", serde_json::json!({}), None)
        .expect("outer append");
    let result = result_slot
        .lock()
        .expect("result lock")
        .take()
        .expect("tool attempted");
    assert!(result.is_error);
    let info = result
        .error
        .as_ref()
        .and_then(|error| error.info.as_ref())
        .expect("typed error");
    assert_eq!(info.name, "GoalError");
    assert_eq!(info.code, "GOAL_COMMIT_FAILED");
    assert!(result.value.is_none());
    assert!(result.additional_contexts.is_empty());
    assert!(goals.get(&agent).expect("goal read").is_none());
    assert_eq!(
        agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "goal/change")
            .count(),
        0
    );
    assert_eq!(*published.lock().expect("published lock"), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_before_dispatch_never_creates_a_durable_goal() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-cancelled");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);

    let input = ToolExecutionInput {
        call_id: call_id("call-cancelled-create"),
        root_call_id: None,
        name: "create_goal".to_string(),
        arguments: serde_json::json!({ "objective": "must never exist" }),
        agent: Some(agent.clone()),
        parent: None,
        signal: Arc::new(|| true),
    };
    let result = agents
        .with_initiator(agent.clone(), tools.execute(input))
        .await
        .expect("initiator boundary");
    assert!(result.is_error);
    assert_eq!(error_code(&result), Some("ABORTED_BEFORE_DISPATCH"));
    assert!(
        !agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "goal/change")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restored_goal_rearms_only_after_human_steering_inside_its_goal_turn() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-restored-steering");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "continue after restore".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");
    close_turn(&agent, 1);
    goals.disarm(&agent).expect("disarm restored goal");
    append_turn_message(
        &agent,
        2,
        serde_json::json!({
            "kind": "goal",
            "goalId": created.id.as_str(),
            "revision": created.revision,
            "round": 1
        }),
    );

    let before_human = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "update_goal",
        serde_json::json!({
            "goal_id": created.id.as_str(),
            "revision": created.revision,
            "action": "resume"
        }),
    )
    .await;
    assert_eq!(
        error_code(&before_human),
        Some("GOAL_TOOL_AUTHORITY_REQUIRED")
    );
    assert_eq!(
        goals.get(&agent).expect("get").expect("goal").activation,
        dsh_goal::GoalActivation::Disarmed
    );

    agent
        .session()
        .append(
            "user/message",
            serde_json::json!({
                "id": "message-human-steer",
                "content": [{ "type": "text", "text": "Continue this goal now." }],
                "source": { "kind": "user" }
            }),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("human steering");
    let resumed = execute_as(
        &agents,
        &tools,
        agent,
        "update_goal",
        serde_json::json!({
            "goal_id": created.id.as_str(),
            "revision": created.revision,
            "action": "resume"
        }),
    )
    .await;
    assert!(
        !resumed.is_error,
        "{:?}",
        resumed.error.as_ref().map(|error| &error.message)
    );
    assert_eq!(
        resumed.value.as_ref().expect("value")["goal"]["revision"],
        2
    );
    assert_eq!(
        resumed.value.as_ref().expect("value")["activation"],
        "armed"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn conditional_goal_arguments_return_stable_typed_failures() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-conditional-errors");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);

    let invalid_create = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "create_goal",
        serde_json::json!({ "objective": " " }),
    )
    .await;
    assert_eq!(error_code(&invalid_create), Some("GOAL_INVALID_OBJECTIVE"));
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "valid".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");
    let cases = [
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 1,
            "action": "pause", "objective": "not valid for pause"
        }),
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 1,
            "action": "complete", "max_goal_rounds": 2
        }),
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 1,
            "action": "blocked"
        }),
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 1,
            "action": "blocked", "blocked_reason": " "
        }),
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 1,
            "action": "complete", "blocked_reason": "not valid"
        }),
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 1,
            "action": "edit", "objective": "still valid",
            "blocked_reason": "not valid"
        }),
        serde_json::json!({
            "goal_id": "", "revision": 0,
            "action": "edit", "objective": "x"
        }),
    ];
    for arguments in cases {
        let result = execute_as(&agents, &tools, agent.clone(), "update_goal", arguments).await;
        assert_eq!(error_code(&result), Some("GOAL_TOOL_INVALID_UPDATE"));
    }
    let current = goals.get(&agent).expect("get").expect("goal");
    assert_eq!(current.revision, 1);
    assert_eq!(current.phase, dsh_goal::GoalPhase::Active);
}

#[tokio::test(flavor = "current_thread")]
async fn empty_fillers_are_ignored_for_fields_unused_by_the_selected_action() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let goals = GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-empty-fillers");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);
    let created = goals
        .create(
            &agent,
            dsh_goal::CreateGoalRequest {
                objective: "valid".to_string(),
                max_goal_rounds: None,
            },
        )
        .expect("create");

    let steps = [
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 1, "action": "edit",
            "objective": "edited", "max_goal_rounds": 0, "blocked_reason": ""
        }),
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 2, "action": "edit",
            "objective": "", "max_goal_rounds": 8, "blocked_reason": ""
        }),
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 3, "action": "pause",
            "objective": "", "max_goal_rounds": 0, "blocked_reason": ""
        }),
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 4, "action": "resume",
            "objective": "", "max_goal_rounds": 0, "blocked_reason": ""
        }),
        serde_json::json!({
            "goal_id": created.id.as_str(), "revision": 5, "action": "complete",
            "objective": "", "max_goal_rounds": 0, "blocked_reason": ""
        }),
    ];
    for arguments in steps {
        let result = execute_as(&agents, &tools, agent.clone(), "update_goal", arguments).await;
        assert!(
            !result.is_error,
            "{:?}",
            result.error.as_ref().map(|error| &error.message)
        );
    }
    let current = goals.get(&agent).expect("get").expect("goal");
    assert_eq!(current.objective, "edited");
    assert_eq!(current.max_goal_rounds, 8);
    assert_eq!(current.revision, 6);
    assert_eq!(current.phase, dsh_goal::GoalPhase::Complete);
}

#[tokio::test(flavor = "current_thread")]
async fn execution_authority_rejects_agentless_driverless_nonhuman_child_and_stale_calls() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");

    let agentless = tools
        .execute(tool_input("get_goal", serde_json::json!({}), None))
        .await;
    assert_eq!(error_code(&agentless), Some("GOAL_TOOL_AGENT_REQUIRED"));

    let root = RootAgent::boxed("tool-goal-authority-root");
    agents.enter(root.clone(), None).expect("enter root");
    let before_turn = execute_as(
        &agents,
        &tools,
        root.clone(),
        "get_goal",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(error_code(&before_turn), Some("GOAL_TOOL_DRIVER_REQUIRED"));

    open_human_turn(&root, 1);
    let driverless = tools
        .execute(tool_input(
            "get_goal",
            serde_json::json!({}),
            Some(root.clone()),
        ))
        .await;
    assert_eq!(error_code(&driverless), Some("GOAL_TOOL_DRIVER_REQUIRED"));
    root.session()
        .append(
            "turn/end",
            serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("turn/end");
    let after_turn = execute_as(
        &agents,
        &tools,
        root.clone(),
        "get_goal",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(error_code(&after_turn), Some("GOAL_TOOL_DRIVER_REQUIRED"));

    let plugin_root = RootAgent::boxed("tool-goal-authority-plugin");
    agents
        .enter(plugin_root.clone(), None)
        .expect("enter plugin root");
    append_turn_message(
        &plugin_root,
        1,
        serde_json::json!({ "kind": "plugin", "plugin": "test" }),
    );
    let nonhuman = execute_as(
        &agents,
        &tools,
        plugin_root.clone(),
        "create_goal",
        serde_json::json!({ "objective": "forged" }),
    )
    .await;
    assert_eq!(error_code(&nonhuman), Some("GOAL_TOOL_AUTHORITY_REQUIRED"));

    let child = RootAgent::boxed("tool-goal-authority-child");
    agents
        .enter(child.clone(), Some(root.clone()))
        .expect("enter child");
    open_human_turn(&child, 1);
    let child_create = execute_as(
        &agents,
        &tools,
        child.clone(),
        "create_goal",
        serde_json::json!({ "objective": "child goal" }),
    )
    .await;
    assert_eq!(
        error_code(&child_create),
        Some("GOAL_TOOL_AUTHORITY_REQUIRED")
    );

    let stale = RootAgent::with_session(root.session().clone());
    let stale_call = agents
        .with_initiator(
            stale.clone(),
            tools.execute(tool_input("get_goal", serde_json::json!({}), Some(stale))),
        )
        .await
        .expect("initiator boundary");
    assert_eq!(error_code(&stale_call), Some("GOAL_TOOL_DRIVER_REQUIRED"));

    let idle = RootAgent::with_status("tool-goal-authority-idle", AgentStatus::Idle);
    agents.enter(idle.clone(), None).expect("enter idle");
    open_human_turn(&idle, 1);
    let idle_call = execute_as(
        &agents,
        &tools,
        idle.clone(),
        "get_goal",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(error_code(&idle_call), Some("GOAL_TOOL_DRIVER_REQUIRED"));

    let other = RootAgent::boxed("tool-goal-authority-other");
    agents.enter(other.clone(), None).expect("enter other");
    open_human_turn(&other, 1);
    let mismatched = agents
        .with_initiator(
            root.clone(),
            tools.execute(tool_input(
                "get_goal",
                serde_json::json!({}),
                Some(other.clone()),
            )),
        )
        .await
        .expect("initiator boundary");
    assert_eq!(error_code(&mismatched), Some("GOAL_TOOL_DRIVER_REQUIRED"));

    for agent in [&root, &plugin_root, &child, &idle, &other] {
        assert!(
            !agent
                .session()
                .events()
                .iter()
                .any(|event| event.type_ == "goal/change"),
            "rejected authority must not mutate {}",
            agent.id()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn schema_rejections_preserve_the_standard_invalid_args_error_contract() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");

    let result = tools
        .execute(tool_input(
            "get_goal",
            serde_json::json!({ "unexpected": true }),
            None,
        ))
        .await;
    assert!(result.is_error);
    let error = result.error.as_ref().expect("error");
    let info = error.info.as_ref().expect("typed error info");
    assert_eq!(info.name, "ToolArgsError");
    assert_eq!(info.code, "INVALID_ARGS");
    assert!(
        error.message.starts_with("invalid arguments:"),
        "{}",
        error.message
    );
}

#[tokio::test(flavor = "current_thread")]
async fn human_turn_reads_creates_edits_pauses_and_resumes_by_exact_revision() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::boxed("tool-goal-human");
    agents.enter(agent.clone(), None).expect("enter root");
    open_human_turn(&agent, 1);

    let read = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "get_goal",
        serde_json::json!({}),
    )
    .await;
    assert!(
        !read.is_error,
        "{:?}",
        read.error.as_ref().map(|error| &error.message)
    );
    assert_eq!(read.value, Some(serde_json::json!({ "goal": null })));

    let created = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "create_goal",
        serde_json::json!({ "objective": "finish the feature", "max_goal_rounds": 8 }),
    )
    .await;
    assert!(
        !created.is_error,
        "{:?}",
        created.error.as_ref().map(|error| &error.message)
    );
    let mut goal = created.value.as_ref().expect("created value")["goal"].clone();
    assert_eq!(goal["revision"], 1);
    assert_eq!(goal["phase"], "active");
    assert_eq!(goal["maxGoalRounds"], 8);

    let edited = execute_as(
        &agents,
        &tools,
        agent.clone(),
        "update_goal",
        serde_json::json!({
            "goal_id": goal["id"],
            "revision": goal["revision"],
            "action": "edit",
            "objective": "ship the feature"
        }),
    )
    .await;
    assert!(
        !edited.is_error,
        "{:?}",
        edited.error.as_ref().map(|error| &error.message)
    );
    goal = edited.value.as_ref().expect("edited value")["goal"].clone();
    assert_eq!(goal["revision"], 2);
    assert_eq!(goal["objective"], "ship the feature");

    for (action, phase, revision) in [("pause", "paused", 3), ("resume", "active", 4)] {
        let result = execute_as(
            &agents,
            &tools,
            agent.clone(),
            "update_goal",
            serde_json::json!({
                "goal_id": goal["id"],
                "revision": goal["revision"],
                "action": action
            }),
        )
        .await;
        assert!(
            !result.is_error,
            "{:?}",
            result.error.as_ref().map(|error| &error.message)
        );
        goal = result.value.as_ref().expect("updated value")["goal"].clone();
        assert_eq!(goal["phase"], phase);
        assert_eq!(goal["revision"], revision);
    }

    assert_eq!(
        agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "goal/change")
            .count(),
        4
    );
}
