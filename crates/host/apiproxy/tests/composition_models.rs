//! Composition-layer `session.models` / `session.selectModel` over the
//! real fetch carrier with a stub adapter and a live agent.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
    InboxTarget,
};
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, ModelSelection, to_fetch_handler,
};
use dsh_llm::{ChunkStream, GenerateOptions, LlmAdapter, LlmModelInfo, LlmRuntime};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, UserMessage, session_id};
use futures::stream;

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

struct StubAdapter;

#[async_trait::async_trait]
impl LlmAdapter for StubAdapter {
    fn provider_info(&self, provider: &str) -> dsh_llm::LlmProviderInfo {
        dsh_llm::LlmProviderInfo {
            id: provider.to_string(),
            name: "Stub Provider".to_string(),
        }
    }

    async fn list_models(&self, _provider: &str) -> Vec<LlmModelInfo> {
        vec![LlmModelInfo {
            provider: "openai".to_string(),
            id: "gpt-x".to_string(),
            name: "GPT X".to_string(),
            description: None,
            input_modalities: None,
        }]
    }

    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        Box::pin(stream::empty())
    }
}

struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl StubAgent {
    fn new(ctx: &Context, id: &str) -> Arc<dyn Agent> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
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

impl Agent for StubAgent {
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
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
    }

    fn cancel(&self, _cause: AgentCancelCause, _options: Option<&CancelOptions>) {}

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: UserMessage) {}

    fn steer(&self, _message: UserMessage) {}

    fn inject(&self, _message: UserMessage) {}
}

async fn register_agent(registry: &AgentRegistry, agent: &Arc<dyn Agent>) {
    registry.register(&registry.ctx, agent.clone());
    let id = agent.id().clone();
    for _ in 0..10_000 {
        if registry.get(&id).is_some() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("agent never became live");
}

struct Harness {
    _ctx: Context,
    handler: dsh_host_apiproxy::FetchHandler,
    agents: Arc<AgentRegistry>,
    agent: Arc<dyn Agent>,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let agents = AgentRegistry::install(&ctx);
        let runtime = LlmRuntime::install(&ctx);
        runtime
            .register_adapter(&ctx, vec!["openai".to_string()], Arc::new(StubAdapter))
            .expect("adapter");
        let agent = StubAgent::new(&ctx, "owner");
        let service = ApiProxyService::install(
            &ctx,
            ApiProxyDefaults {
                default_model_selection: Arc::new(|| ModelSelection {
                    provider: "openai".to_string(),
                    model: "gpt-x".to_string(),
                    reasoning_effort: None,
                }),
                ..Default::default()
            },
        );
        let handler = to_fetch_handler(service);
        Self {
            _ctx: ctx,
            handler,
            agents,
            agent,
        }
    }

    async fn register_owner(&self) {
        register_agent(&self.agents, &self.agent).await;
    }

    async fn post(&self, method: &str, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": method,
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: format!("/api/{method}"),
                query: vec![],
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Some(body.into_bytes()),
            })
            .await;
        assert_eq!(response.status(), http::StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("unary answers are byte bodies");
        };
        serde_json::from_slice(&bytes).expect("json")
    }
}

#[test]
fn models_reports_the_default_selection_and_catalog() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        let response = harness
            .post(
                "session.models",
                serde_json::json!({ "sessionId": "owner" }),
            )
            .await;
        assert_eq!(response["result"]["ok"], true, "{response}");
        let value = &response["result"]["value"];
        assert_eq!(value["current"]["provider"], "openai");
        assert_eq!(value["current"]["model"], "gpt-x");
        assert_eq!(value["routable"], true);
        assert_eq!(value["groups"].as_array().expect("groups").len(), 1);
        assert_eq!(value["groups"][0]["models"][0]["id"], "gpt-x");
    });
}

#[test]
fn select_model_validates_and_records_the_selection() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        let selected = harness
            .post(
                "session.selectModel",
                serde_json::json!({ "sessionId": "owner", "provider": "openai", "model": "gpt-x" }),
            )
            .await;
        assert_eq!(selected["result"]["ok"], true, "{selected}");
        assert_eq!(
            selected["result"]["value"]["selected"]["provider"],
            "openai"
        );
        assert_eq!(selected["result"]["value"]["selected"]["model"], "gpt-x");

        // An unknown provider is model-unavailable.
        let rejected = harness
            .post(
                "session.selectModel",
                serde_json::json!({ "sessionId": "owner", "provider": "nope", "model": "x" }),
            )
            .await;
        assert_eq!(rejected["result"]["ok"], false, "{rejected}");
        assert_eq!(rejected["result"]["error"]["code"], "model-unavailable");
    });
}

#[test]
fn models_for_an_unknown_session_reports_the_resolver_failure() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        let response = harness
            .post(
                "session.models",
                serde_json::json!({ "sessionId": "ghost" }),
            )
            .await;
        assert_eq!(response["result"]["ok"], false);
        // No persistence backend in this harness: the resolver's cold
        // inspection fails internally (a mounted backend would classify
        // the missing identity as session-not-found).
        assert_eq!(response["result"]["error"]["code"], "internal");
    });
}
