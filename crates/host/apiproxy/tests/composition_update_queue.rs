//! Composition-layer `session.updateQueue` over the real fetch carrier:
//! inbox edit/remove/steer and the rejection vocabulary.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
    InboxTarget,
};
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_llm::{Message, MessageSource, Role, message_id};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, UserMessage, session_id};

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

fn text_message(id: &str, text: &str) -> UserMessage {
    Message {
        id: message_id(id),
        role: Role::User,
        content: vec![dsh_llm::ContentBlock::Text {
            text: text.to_string(),
        }],
        source: MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
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
            "rpcId": "r1",
            "method": "session.updateQueue",
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/session.updateQueue".to_string(),
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
fn edit_replaces_the_pending_message_content() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        harness
            .agent
            .inbox
            .append(InboxTarget::NextTurn, text_message("m1", "before"))
            .expect("append");
        let edited = harness
            .post(serde_json::json!({
                "sessionId": "owner",
                "itemId": "m1",
                "action": { "kind": "edit", "content": [{ "type": "text", "text": "after" }] },
            }))
            .await;
        assert_eq!(edited["result"]["ok"], true, "{edited}");
        assert_eq!(edited["result"]["value"]["accepted"], true);
        let pending = harness.agent.inbox.next_turn();
        assert_eq!(pending.len(), 1);
        match &pending[0].content[0] {
            dsh_llm::ContentBlock::Text { text } => assert_eq!(text, "after"),
            other => panic!("expected text, got {other:?}"),
        }
    });
}

#[test]
fn remove_drops_the_pending_message() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        harness
            .agent
            .inbox
            .append(InboxTarget::NextTurn, text_message("m1", "x"))
            .expect("append");
        let removed = harness
            .post(serde_json::json!({
                "sessionId": "owner",
                "itemId": "m1",
                "action": { "kind": "remove" },
            }))
            .await;
        assert_eq!(removed["result"]["ok"], true, "{removed}");
        assert!(harness.agent.inbox.next_turn().is_empty());
    });
}

#[test]
fn the_rejection_vocabulary_covers_steer_missing_and_non_text_edits() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        harness
            .agent
            .inbox
            .append(InboxTarget::NextTurn, text_message("m1", "x"))
            .expect("append");

        // Steer against an idle agent: steer-unavailable.
        let steer = harness
            .post(serde_json::json!({
                "sessionId": "owner",
                "itemId": "m1",
                "action": { "kind": "steer" },
            }))
            .await;
        assert_eq!(steer["result"]["ok"], false);
        assert_eq!(steer["result"]["error"]["code"], "steer-unavailable");

        // A non-text edit: attachment-error.
        let non_text = harness
            .post(serde_json::json!({
                "sessionId": "owner",
                "itemId": "m1",
                "action": { "kind": "edit", "content": [{ "type": "reasoning", "text": "think" }] },
            }))
            .await;
        assert_eq!(non_text["result"]["ok"], false);
        assert_eq!(non_text["result"]["error"]["code"], "attachment-error");
        assert_eq!(
            non_text["result"]["error"]["details"]["reason"],
            "QUEUE_EDIT_NON_TEXT"
        );

        // An unknown item: queue-item-not-found.
        let missing = harness
            .post(serde_json::json!({
                "sessionId": "owner",
                "itemId": "ghost",
                "action": { "kind": "remove" },
            }))
            .await;
        assert_eq!(missing["result"]["ok"], false);
        assert_eq!(missing["result"]["error"]["code"], "queue-item-not-found");
    });
}
