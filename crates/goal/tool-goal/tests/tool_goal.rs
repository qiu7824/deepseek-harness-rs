use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentOptions, AgentRegistry, AgentStatus, Inbox};
use dsh_goal::GoalService;
use dsh_llm::call_id;
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, session_id};
use dsh_system_prompt::{AssembleContext, SystemPrompt};
use dsh_tool_goal::Config;
use dsh_tools::{
    ToolDefinition, ToolExecutionInput, ToolExecutionMode, ToolOutputDefinition, ToolRuntime,
};

struct RootAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl RootAgent {
    fn new(raw_id: &str) -> Arc<dyn Agent> {
        let id = session_id(raw_id);
        let session = Session::create(id.clone(), None, None).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
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
        AgentStatus::Running
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

fn open_human_turn(agent: &Arc<dyn Agent>, turn: u64) {
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
                "source": { "kind": "user" }
            }),
            None,
        )
        .expect("user/message");
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
    assert!(
        outcome.is_err() || outcome.as_ref().is_ok_and(|result| result.is_err()),
        "the conflicting installation must reject"
    );
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
async fn human_turn_reads_creates_edits_pauses_and_resumes_by_exact_revision() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    dsh_tool_goal::apply(&ctx, &Config::default()).expect("apply");
    let agent = RootAgent::new("tool-goal-human");
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
