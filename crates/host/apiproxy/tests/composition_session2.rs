//! Composition-layer `session.rename` / `session.cancel` over the real
//! fetch carrier.

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
use dsh_session::{
    CreateSessionMeta, CreateSessionOptions, Session, SessionId, SessionStore, UserMessage,
    session_id,
};
use dsh_session_title::{Config as TitleConfig, SessionTitleService};

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
    cancelled: std::sync::atomic::AtomicBool,
}

impl StubAgent {
    fn new(ctx: &Context, id: &str) -> Arc<Self> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        Self::with_session(ctx, session)
    }

    fn with_session(ctx: &Context, session: Session) -> Arc<Self> {
        let id = session.id().clone();
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
            ctx: ctx.clone(),
            scope_key: ScopeKey::new(),
            cancelled: std::sync::atomic::AtomicBool::new(false),
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

    fn cancel(&self, cause: AgentCancelCause, _options: Option<&CancelOptions>) {
        assert!(matches!(cause, AgentCancelCause::User));
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
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
    agent: std::sync::Mutex<Option<Arc<StubAgent>>>,
    agents: Arc<AgentRegistry>,
    sessions: Arc<SessionStore>,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let sessions = SessionStore::install(&ctx);
        let agents = AgentRegistry::install(&ctx);
        SessionTitleService::install(
            &ctx,
            TitleConfig {
                fallback_max_words: 8,
                fallback_max_bytes: 128,
                max_title_bytes: 256,
            },
        );
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self {
            _ctx: ctx,
            handler,
            agent: std::sync::Mutex::new(None),
            agents,
            sessions,
        }
    }

    /// Attach the stub agent on the STORE's live session (rename checks
    /// pointer identity against the store).
    async fn attach_owner(&self) {
        let live = self
            .sessions
            .create(
                &self._ctx,
                Some(session_id("owner")),
                Some(CreateSessionOptions {
                    meta: Some(CreateSessionMeta {
                        cwd: Some("D:\\proj".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("live session");
        let agent = StubAgent::with_session(&self._ctx, live);
        register_agent(&self.agents, &(agent.clone() as Arc<dyn Agent>)).await;
        *self.agent.lock().unwrap() = Some(agent);
    }

    fn agent(&self) -> Arc<StubAgent> {
        self.agent.lock().unwrap().clone().expect("attached agent")
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
fn rename_accepts_a_title_and_reports_the_event_seq() {
    run(async {
        let harness = Harness::new();
        harness.attach_owner().await;
        let renamed = harness
            .post(
                "session.rename",
                serde_json::json!({ "sessionId": "owner", "title": "My Session" }),
            )
            .await;
        assert_eq!(renamed["result"]["ok"], true, "{renamed}");
        assert_eq!(renamed["result"]["value"]["title"], "My Session");
        assert!(renamed["result"]["value"]["seq"].as_i64().is_some());
    });
}

#[test]
fn cancel_delivers_the_user_cancel_and_unknown_sessions_are_session_not_found() {
    run(async {
        let harness = Harness::new();
        harness.attach_owner().await;
        let cancelled = harness
            .post(
                "session.cancel",
                serde_json::json!({ "sessionId": "owner" }),
            )
            .await;
        assert_eq!(cancelled["result"]["ok"], true, "{cancelled}");
        assert_eq!(cancelled["result"]["value"]["accepted"], true);
        assert!(
            harness
                .agent()
                .cancelled
                .load(std::sync::atomic::Ordering::SeqCst),
            "the agent received the user cancel"
        );

        let missing = harness
            .post(
                "session.cancel",
                serde_json::json!({ "sessionId": "ghost" }),
            )
            .await;
        assert_eq!(missing["result"]["ok"], false);
        assert_eq!(missing["result"]["error"]["code"], "session-not-found");
    });
}
