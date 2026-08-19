//! Composition-layer `session.list` / `session.create` over the real fetch
//! carrier: attached/cold summary merge and the create-with-agent ladder.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{
    Agent, AgentCancelCause, AgentFactory, AgentHandle, AgentOptions, AgentRegistry, AgentStatus,
    CancelOptions, CreateAgentOptions, Inbox, InboxTarget, ResumeAgentOptions,
};
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_scope::ScopeKey;
use dsh_session::{
    CreateSessionMeta, CreateSessionOptions, Session, SessionHeader, SessionId, SessionStore,
    UserMessage, session_id,
};
use dsh_session_persistence_jsonl::{JsonlConfig, JsonlSessionPersistence};

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_root() -> String {
    std::env::temp_dir()
        .join(format!(
            "dsh-apiproxy-sess-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
        .to_string_lossy()
        .into_owned()
}

/// Minimal live agent.
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

/// Factory producing stub agents for create/resume.
struct StubFactory;

#[async_trait::async_trait]
impl AgentFactory for StubFactory {
    async fn create_agent(
        &self,
        owner_ctx: &Context,
        options: CreateAgentOptions,
    ) -> Result<AgentHandle, String> {
        let session_id = options.session_id.expect("session id");
        let session =
            Session::create(session_id.clone(), None, None).map_err(|error| error.to_string())?;
        let inbox = Inbox::new(&session, Default::default()).map_err(|error| error.to_string())?;
        let agent: Arc<dyn Agent> = Arc::new(StubAgent {
            id: session_id,
            session,
            inbox,
            ctx: owner_ctx.clone(),
            scope_key: ScopeKey::new(),
        });
        Ok(AgentHandle {
            agent,
            dispose: Box::pin(async {}),
        })
    }

    async fn resume(
        &self,
        owner_ctx: &Context,
        options: ResumeAgentOptions,
    ) -> Result<AgentHandle, String> {
        self.create_agent(
            owner_ctx,
            CreateAgentOptions {
                session_id: options.resume_session_id,
                ..Default::default()
            },
        )
        .await
    }
}

struct Harness {
    _ctx: Context,
    handler: dsh_host_apiproxy::FetchHandler,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        SessionStore::install(&ctx);
        let agents = AgentRegistry::install(&ctx);
        agents.set_factory(Arc::new(StubFactory));
        let root = temp_root();
        JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root,
                ..Default::default()
            },
        )
        .expect("jsonl backend");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self { _ctx: ctx, handler }
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
fn create_publishes_a_session_with_an_idle_agent() {
    run(async {
        let harness = Harness::new();
        let created = harness
            .post(
                "session.create",
                serde_json::json!({ "sessionId": "s-new", "cwd": "D:\\proj" }),
            )
            .await;
        assert_eq!(created["result"]["ok"], true, "{created}");
        assert_eq!(created["result"]["value"]["sessionId"], "s-new");

        let listed = harness.post("session.list", serde_json::json!({})).await;
        let items = listed["result"]["value"]["items"]
            .as_array()
            .expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["sessionId"], "s-new");
        assert_eq!(items[0]["running"], false);
        assert_eq!(items[0]["blank"], true);
        assert_eq!(items[0]["cwd"], "D:\\proj");
    });
}

#[test]
fn list_merges_attached_and_cold_sessions_sorted_by_updated_at() {
    run(async {
        let harness = Harness::new();
        // One attached session directly through the store.
        let sessions = harness
            ._ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .expect("sessions")
            .as_ref()
            .clone();
        let attached = sessions
            .create(
                &harness._ctx,
                Some(session_id("attached-1")),
                Some(CreateSessionOptions {
                    meta: Some(CreateSessionMeta {
                        cwd: Some("D:\\a".to_string()),
                        created_at: Some(100),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("attached");
        let _ = attached;
        // One cold session through the persistence backend.
        let persistence = harness
            ._ctx
            .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                "sessionPersistence",
                false,
            )
            .expect("persistence")
            .as_ref()
            .clone();
        persistence
            .create(SessionHeader {
                version: 1,
                id: session_id("cold-1"),
                created_at: 200,
                cwd: Some("D:\\c".to_string()),
                parent_session: None,
                seed_length: None,
                origin: Some("subagent".to_string()),
                delegation_depth: None,
                agent_preset: None,
            })
            .await
            .expect("cold session");
        // Materialize the artifact so the scanner lists it.
        persistence
            .append(
                &session_id("cold-1"),
                &[dsh_session::SessionEvent {
                    type_: "turn/start".to_string(),
                    seq: 0,
                    time: 200,
                    data: serde_json::json!({ "turn": 1 }),
                    ignorable: None,
                    surface_op: None,
                    source_event_seqs: None,
                }],
            )
            .await
            .expect("append");

        let listed = harness.post("session.list", serde_json::json!({})).await;
        let items = listed["result"]["value"]["items"]
            .as_array()
            .expect("items");
        assert_eq!(items.len(), 2, "{listed}");
        // cold-1 (updatedAt 200) sorts before attached-1 (100).
        assert_eq!(items[0]["sessionId"], "cold-1");
        assert_eq!(items[0]["running"], false);
        assert_eq!(items[0]["origin"], "subagent");
        assert_eq!(items[1]["sessionId"], "attached-1");
    });
}

#[test]
fn create_with_a_workspace_id_is_internal_until_attachment_lands() {
    run(async {
        let harness = Harness::new();
        let created = harness
            .post(
                "session.create",
                serde_json::json!({ "sessionId": "s-ws", "workspaceId": "w1" }),
            )
            .await;
        assert_eq!(created["result"]["ok"], false);
        assert_eq!(created["result"]["error"]["code"], "internal");
    });
}
