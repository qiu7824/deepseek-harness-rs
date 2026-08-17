//! Composition-layer `subagent.*` over the real fetch carrier: listing
//! with parent availability and the interrupt acknowledgement.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
    InboxTarget,
};
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, SessionStore, UserMessage, session_id};
use dsh_subagent::SubagentRuntime;

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
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

fn stub(ctx: &Context, id: &str) -> Arc<dyn Agent> {
    let id = session_id(id);
    let session = Session::create(id.clone(), None, None).expect("session");
    let inbox = Inbox::new(&session, Default::default()).expect("inbox");
    Arc::new(StubAgent {
        id,
        session,
        inbox,
        ctx: ctx.clone(),
        scope_key: ScopeKey::new(),
    })
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
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        SessionStore::install(&ctx);
        dsh_session_projection::SessionProjectionRegistry::install(&ctx);
        let agents = AgentRegistry::install(&ctx);
        SubagentRuntime::install(&ctx);
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self {
            _ctx: ctx,
            handler,
            agents,
        }
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
fn list_reports_an_empty_catalog_and_parent_availability() {
    run(async {
        let harness = Harness::new();
        // No children, parent not live.
        let listed = harness
            .post(
                "subagent.list",
                serde_json::json!({ "parentSessionId": "parent-1" }),
            )
            .await;
        assert_eq!(listed["result"]["ok"], true, "{listed}");
        assert_eq!(
            listed["result"]["value"]["entries"].as_array().expect("entries").len(),
            0
        );
        assert_eq!(listed["result"]["value"]["parentAvailable"], false);

        // Attach the parent: availability flips.
        let parent = stub(&harness._ctx, "parent-1");
        register_agent(&harness.agents, &parent).await;
        let listed = harness
            .post(
                "subagent.list",
                serde_json::json!({ "parentSessionId": "parent-1" }),
            )
            .await;
        assert_eq!(listed["result"]["value"]["parentAvailable"], true);
    });
}

#[test]
fn interrupt_acknowledges_the_admitted_signal() {
    run(async {
        let harness = Harness::new();
        // An absent child is still accepted (fire-and-return semantics).
        let interrupted = harness
            .post(
                "subagent.interrupt",
                serde_json::json!({
                    "parentSessionId": "parent-1",
                    "childSessionId": "child-1",
                    "mode": "continuable",
                }),
            )
            .await;
        assert_eq!(interrupted["result"]["ok"], true, "{interrupted}");
        assert_eq!(interrupted["result"]["value"]["accepted"], true);
    });
}

#[test]
fn prompt_without_a_live_parent_is_subagent_parent_unavailable() {
    run(async {
        let harness = Harness::new();
        let prompted = harness
            .post(
                "subagent.prompt",
                serde_json::json!({
                    "parentSessionId": "ghost-parent",
                    "childSessionId": "child-1",
                    "mode": "continuable",
                    "content": [{ "type": "text", "text": "hi" }],
                }),
            )
            .await;
        assert_eq!(prompted["result"]["ok"], false);
        assert_eq!(
            prompted["result"]["error"]["code"],
            "subagent-parent-unavailable"
        );
    });
}

#[test]
fn a_missing_runtime_is_internal() {
    run(async {
        let ctx = Context::root();
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "subagent.list",
            "payload": { "parentSessionId": "p" },
        }))
        .expect("envelope");
        let response = handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/subagent.list".to_string(),
                query: vec![],
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Some(body.into_bytes()),
            })
            .await;
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(parsed["result"]["ok"], false);
        assert_eq!(parsed["result"]["error"]["code"], "internal");
    });
}
