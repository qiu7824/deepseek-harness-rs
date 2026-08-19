use std::collections::BTreeMap;
use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentOptions, AgentStatus, Inbox};
use dsh_llm::call_id;
use dsh_lsp::{
    Lsp, LspLocation, LspOperation, LspPosition, LspProvider, LspProviderId, LspProviderQuery,
    LspQueryResult, LspRange,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionHeader, SessionId, session_id};
use dsh_system_prompt::SystemPrompt;
use dsh_tools::{ToolExecutionInput, ToolRuntime};

struct RecordingProvider {
    id: LspProviderId,
    extensions: BTreeMap<String, String>,
    seen: parking_lot::Mutex<Vec<LspProviderQuery>>,
}

#[async_trait::async_trait]
impl LspProvider for RecordingProvider {
    fn id(&self) -> &LspProviderId {
        &self.id
    }

    fn extension_to_language(&self) -> &BTreeMap<String, String> {
        &self.extensions
    }

    async fn query(&self, request: LspProviderQuery) -> Result<LspQueryResult, dsh_lsp::LspError> {
        self.seen.lock().push(request);
        Ok(LspQueryResult::Locations {
            locations: vec![LspLocation {
                uri: "file:///workspace/src/definition.rs".to_string(),
                range: LspRange {
                    start: LspPosition {
                        line: 7,
                        character: 2,
                    },
                    end: LspPosition {
                        line: 7,
                        character: 8,
                    },
                },
            }],
            resolved_workspace_uri: "file:///workspace".to_string(),
        })
    }
}

struct ProbeAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl ProbeAgent {
    fn agent(ctx: &Context, cwd: &str) -> Arc<dyn Agent> {
        let id = session_id("tool-lsp-agent");
        let header = SessionHeader {
            version: dsh_session::SESSION_FORMAT_VERSION,
            id: id.clone(),
            created_at: 1,
            cwd: Some(cwd.to_string()),
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        let session = Session::create(id.clone(), None, Some(&header)).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
            ctx: ctx.clone(),
            scope_key: ScopeKey::new(),
        })
    }
}

impl Agent for ProbeAgent {
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

#[tokio::test]
async fn model_tool_uses_session_cwd_and_converts_one_based_coordinates() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    let lsp = Arc::new(Lsp::new());
    let provider = Arc::new(RecordingProvider {
        id: LspProviderId::new("rust"),
        extensions: BTreeMap::from([(".rs".to_string(), "rust".to_string())]),
        seen: parking_lot::Mutex::new(Vec::new()),
    });
    let _registration = lsp
        .register_provider(provider.clone())
        .expect("register provider");
    dsh_tool_lsp::apply(&ctx, lsp).expect("mount tool");
    let agent = ProbeAgent::agent(&ctx, "C:/workspace");

    let result = tools
        .execute(ToolExecutionInput {
            call_id: call_id("tool-lsp-call"),
            root_call_id: None,
            name: "lsp".to_string(),
            arguments: serde_json::json!({
                "operation": "goToDefinition",
                "file_path": "src/main.rs",
                "line": 3,
                "character": 5
            }),
            agent: Some(agent),
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;

    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(result.value.as_ref().unwrap()["kind"], "locations");
    assert_eq!(
        result.value.as_ref().unwrap()["locations"][0]["range"]["start"]["line"],
        7
    );
    let seen = provider.seen.lock();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].operation, LspOperation::GoToDefinition);
    assert_eq!(
        seen[0].position,
        LspPosition {
            line: 2,
            character: 4
        }
    );
    assert_eq!(seen[0].workspace_root, "C:/workspace");
    assert_eq!(seen[0].language_id, "rust");
}
