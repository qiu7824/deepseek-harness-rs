use std::sync::Arc;

use cordis::Context;
use dsh_agent::{AgentOptions, AgentRegistry, AgentStatus};
use dsh_llm::{ContentBlock, call_id};
use dsh_session::{Session, SessionId, session_id};
use dsh_subagent::{SubagentIdentityProjection, SubagentListEntry, SubagentRuntime};
use dsh_tool_subagent_control::list_agents::{INJECT, NAME, apply, project};
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
                &Session::create(session_id("probe-list-inbox"), None, None).expect("session"),
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
    dsh_session::SessionStore::install(&ctx);
    dsh_session_projection::SessionProjectionRegistry::install(&ctx);
    SubagentRuntime::install(&ctx);
    apply(&ctx).expect("list tool");
    (ctx, tools)
}
fn input(
    arguments: serde_json::Value,
    agent: Option<Arc<dyn dsh_agent::Agent>>,
) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id("call-list"),
        root_call_id: None,
        name: "list_agents".to_string(),
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
async fn registers_exact_list_plugin_contract() {
    assert_eq!(NAME, "tool-subagent-list-agents");
    assert_eq!(INJECT, ["tools", "subagents", "agents"]);
    let (_ctx, tools) = setup();
    let schema = tools
        .schemas(None)
        .into_iter()
        .find(|schema| schema.name == "list_agents")
        .expect("list_agents");
    assert_eq!(
        schema.parameters["properties"]["scope"]["enum"],
        serde_json::json!(["children", "descendants"])
    );
    assert_eq!(schema.parameters["required"], serde_json::json!([]));
    assert!(schema.description.contains("send_message"));
    assert!(schema.description.contains("interrupt_agent"));
}

#[tokio::test]
async fn list_requires_a_calling_agent() {
    let (_ctx, tools) = setup();
    let result = tools.execute(input(serde_json::json!({}), None)).await;
    assert!(result.is_error);
    assert!(text(&result.content).contains("list_agents requires a calling agent"));
}

#[tokio::test]
async fn empty_children_render_as_no_subagents() {
    let (_ctx, tools) = setup();
    let parent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("parent");
    let result = tools
        .execute(input(serde_json::json!({}), Some(parent)))
        .await;
    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(result.value, Some(serde_json::json!([])));
    assert_eq!(text(&result.content), "(no subagents)");
}

#[tokio::test]
async fn projection_filters_one_shot_maps_ready_and_preserves_position() {
    let ctx = Context::root();
    let agents = AgentRegistry::install(&ctx);
    assert!(
        project(
            &agents,
            SubagentListEntry::Child {
                id: session_id("once"),
                activity: "inactive".into(),
                has_children: false,
                identity: SubagentIdentityProjection::OneShot {
                    label: Some("once".into()),
                    seq: 1,
                },
            },
            None,
        )
        .is_none()
    );

    let child = project(
        &agents,
        SubagentListEntry::Child {
            id: session_id("cold"),
            activity: "inactive".into(),
            has_children: false,
            identity: SubagentIdentityProjection::Continuable {
                label: "cold label".into(),
                seq: 1,
            },
        },
        Some((&session_id("parent"), 2)),
    )
    .expect("continuable");
    assert_eq!(
        child,
        serde_json::json!({
            "kind": "child", "id": "cold", "label": "cold label", "status": "ready",
            "parent": "parent", "depth": 2
        })
    );

    let diagnostic = project(
        &agents,
        SubagentListEntry::Diagnostic {
            id: session_id("broken"),
            reason: "unavailable".into(),
        },
        Some((&session_id("parent"), 1)),
    )
    .expect("diagnostic");
    assert_eq!(
        diagnostic,
        serde_json::json!({
            "kind": "diagnostic", "id": "broken", "reason": "unavailable",
            "parent": "parent", "depth": 1
        })
    );
}
