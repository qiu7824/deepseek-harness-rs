//! Composition-layer `session.prompt` over the real fetch carrier: queue
//! and steer delivery, time-zone validation, and the image-admission fence.

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
use dsh_session::{Session, SessionId, UserMessage, session_id};
use parking_lot::Mutex;

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
    delivered: Mutex<Vec<(String, UserMessage)>>,
}

impl StubAgent {
    fn new(ctx: &Context, id: &str) -> Arc<Self> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
            ctx: ctx.clone(),
            scope_key: ScopeKey::new(),
            delivered: Mutex::new(Vec::new()),
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

    fn followup(&self, message: UserMessage) {
        self.delivered
            .lock()
            .push(("followup".to_string(), message));
    }

    fn steer(&self, message: UserMessage) {
        self.delivered.lock().push(("steer".to_string(), message));
    }

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
    agent: Arc<StubAgent>,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let agents = AgentRegistry::install(&ctx);
        let agent = StubAgent::new(&ctx, "owner");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self {
            _ctx: ctx,
            handler,
            agents,
            agent,
        }
    }

    async fn register_owner(&self) {
        register_agent(&self.agents, &(self.agent.clone() as Arc<dyn Agent>)).await;
    }

    async fn post(&self, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r9",
            "method": "session.prompt",
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/session.prompt".to_string(),
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
fn queue_delivers_a_followup_with_the_request_source() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        let accepted = harness
            .post(serde_json::json!({
                "sessionId": "owner",
                "mode": "queue",
                "content": [{ "type": "text", "text": "hello" }],
                "clientTimeZone": "Asia/Shanghai",
            }))
            .await;
        assert_eq!(accepted["result"]["ok"], true, "{accepted}");
        assert_eq!(accepted["result"]["value"]["accepted"], true);

        let delivered = harness.agent.delivered.lock();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, "followup");
        let dsh_llm::MessageSource::User {
            rpc_id,
            client_time_zone,
        } = &delivered[0].1.source
        else {
            panic!("expected user source");
        };
        assert_eq!(rpc_id.as_deref(), Some("r9"));
        assert_eq!(client_time_zone.as_deref(), Some("Asia/Shanghai"));
        match &delivered[0].1.content[0] {
            dsh_llm::ContentBlock::Text { text } => assert_eq!(text, "hello"),
            other => panic!("expected text, got {other:?}"),
        }
    });
}

#[test]
fn steer_delivers_through_the_steer_path() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        let accepted = harness
            .post(serde_json::json!({
                "sessionId": "owner",
                "mode": "steer",
                "content": [{ "type": "text", "text": "redo" }],
            }))
            .await;
        assert_eq!(accepted["result"]["ok"], true);
        let delivered = harness.agent.delivered.lock();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, "steer");
    });
}

#[test]
fn an_invalid_time_zone_and_image_content_are_rejected() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        let invalid = harness
            .post(serde_json::json!({
                "sessionId": "owner",
                "mode": "queue",
                "content": [{ "type": "text", "text": "x" }],
                "clientTimeZone": "Not/AZone",
            }))
            .await;
        assert_eq!(invalid["result"]["ok"], false);
        assert_eq!(invalid["result"]["error"]["code"], "invalid-time-zone");
        assert_eq!(invalid["result"]["error"]["details"]["value"], "Not/AZone");

        let image = harness
            .post(serde_json::json!({
                "sessionId": "owner",
                "mode": "queue",
                "content": [{ "type": "image", "mediaType": "image/png", "data": "xx" }],
            }))
            .await;
        assert_eq!(image["result"]["ok"], false);
        assert_eq!(image["result"]["error"]["code"], "attachment-error");
    });
}
