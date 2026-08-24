use std::sync::Arc;

use cordis::Context;
use dsh_agent::{AgentOptions, AgentRegistry, AgentStatus};
use dsh_llm::{ContentBlock, call_id};
use dsh_session::{Session, SessionId, session_id};
use dsh_subagent::SubagentRuntime;
use dsh_tool_subagent_control::{INJECT, NAME, apply};
use dsh_tools::{ToolExecutionInput, ToolRuntime};

struct ProbeAgent {
    id: SessionId,
    session: Session,
    scope_key: dsh_scope::ScopeKey,
}

impl ProbeAgent {
    fn new(id: &str) -> Arc<Self> {
        Arc::new(Self {
            id: session_id(id),
            session: Session::create(session_id(id), None, None).expect("session"),
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
                &Session::create(session_id("probe-inbox"), None, None).expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Running
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

fn setup() -> (Context, Arc<ToolRuntime>) {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("systemPrompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    AgentRegistry::install(&ctx);
    SubagentRuntime::install(&ctx);
    apply(&ctx).expect("control tools");
    (ctx, tools)
}

fn input(
    name: &str,
    arguments: serde_json::Value,
    agent: Option<Arc<dyn dsh_agent::Agent>>,
) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id(format!("call-{name}")),
        root_call_id: None,
        name: name.to_string(),
        arguments,
        agent,
        parent: None,
        signal: Arc::new(|| false),
    }
}

fn text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn registers_exact_root_plugin_contract() {
    assert_eq!(NAME, "tool-subagent-control");
    assert_eq!(INJECT, ["tools", "subagents"]);
    let (_ctx, tools) = setup();
    let schemas = tools.schemas(None);
    let send = schemas
        .iter()
        .find(|schema| schema.name == "send_message")
        .expect("send_message");
    assert_eq!(
        send.parameters["required"],
        serde_json::json!(["subagent_id", "message"])
    );
    assert_eq!(send.parameters["additionalProperties"], false);
    assert!(send.description.contains("next turn"));
    let interrupt = schemas
        .iter()
        .find(|schema| schema.name == "interrupt_agent")
        .expect("interrupt_agent");
    assert_eq!(
        interrupt.parameters["required"],
        serde_json::json!(["agent_id"])
    );
    assert!(interrupt.description.contains("current turn"));
    assert!(interrupt.description.contains("send_message"));
}

#[tokio::test]
async fn root_tools_require_a_calling_agent() {
    let (_ctx, tools) = setup();
    let send = tools
        .execute(input(
            "send_message",
            serde_json::json!({
                "subagent_id": "child", "message": "hello"
            }),
            None,
        ))
        .await;
    assert!(send.is_error);
    assert!(text(&send.content).contains("send_message requires a calling agent"));

    let interrupt = tools
        .execute(input(
            "interrupt_agent",
            serde_json::json!({
                "agent_id": "child"
            }),
            None,
        ))
        .await;
    assert!(interrupt.is_error);
    assert!(text(&interrupt.content).contains("interrupt_agent requires a calling agent"));
}

#[tokio::test]
async fn interrupt_of_absent_target_is_an_accepted_noop() {
    let (_ctx, tools) = setup();
    let parent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("parent");
    let result = tools
        .execute(input(
            "interrupt_agent",
            serde_json::json!({
                "agent_id": "absent"
            }),
            Some(parent),
        ))
        .await;
    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(result.value, Some(serde_json::json!({ "accepted": true })));
    assert_eq!(
        text(&result.content),
        "interrupt requested for agent absent"
    );
}
