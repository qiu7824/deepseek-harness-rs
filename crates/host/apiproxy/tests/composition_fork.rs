//! Composition-layer `session.fork` over the real fetch carrier: turn
//! boundary anchoring and the fork-unavailable vocabulary.

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
    CreateSessionMeta, CreateSessionOptions, Session, SessionEvent, SessionId, SessionStore,
    SurfaceOp, UserMessage, session_id,
};

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

/// Factory recording the fork options the registry passed through.
struct RecordingFactory {
    created: parking_lot::Mutex<Vec<RecordedCreate>>,
}

#[derive(Debug)]
struct RecordedCreate {
    session_id: Option<SessionId>,
    seed: Option<Vec<SessionEvent>>,
    meta: Option<dsh_session::CreateSessionMeta>,
}

#[async_trait::async_trait]
impl AgentFactory for RecordingFactory {
    async fn create_agent(
        &self,
        owner_ctx: &Context,
        options: CreateAgentOptions,
    ) -> Result<AgentHandle, String> {
        self.created.lock().push(RecordedCreate {
            session_id: options.session_id.clone(),
            seed: options.seed.clone(),
            meta: options.meta.clone(),
        });
        let session_id = options.session_id.clone().expect("session id");
        let session = Session::create(session_id.clone(), options.seed.clone(), None)
            .map_err(|error| error.to_string())?;
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

fn turn_event(type_: &str, seq: u64, turn: u64) -> SessionEvent {
    SessionEvent {
        type_: type_.to_string(),
        seq,
        time: seq as i64 * 10,
        data: serde_json::json!({ "turn": turn, "reason": "completed" }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

struct Harness {
    _ctx: Context,
    handler: dsh_host_apiproxy::FetchHandler,
    sessions: Arc<SessionStore>,
    factory: Arc<RecordingFactory>,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let sessions = SessionStore::install(&ctx);
        let agents = AgentRegistry::install(&ctx);
        let factory = Arc::new(RecordingFactory {
            created: parking_lot::Mutex::new(Vec::new()),
        });
        agents.set_factory(factory.clone());
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self {
            _ctx: ctx,
            handler,
            sessions,
            factory,
        }
    }

    async fn seed(&self, id: &str, events: Vec<SessionEvent>) {
        let _ = self
            .sessions
            .create(
                &self._ctx,
                Some(session_id(id)),
                Some(CreateSessionOptions {
                    seed: Some(events),
                    meta: Some(CreateSessionMeta {
                        cwd: Some("D:\\proj".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("source session");
    }

    async fn post(&self, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "session.fork",
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/session.fork".to_string(),
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

fn completed_turn_session() -> Vec<SessionEvent> {
    vec![
        turn_event("turn/start", 0, 1),
        turn_event("turn/end", 1, 1),
        turn_event("turn/start", 2, 2),
        turn_event("turn/end", 3, 2),
    ]
}

#[test]
fn fork_anchors_the_last_completed_turn_and_inherits_the_lineage() {
    run(async {
        let harness = Harness::new();
        harness.seed("fork-src", completed_turn_session()).await;
        let forked = harness
            .post(serde_json::json!({ "sessionId": "fork-src" }))
            .await;
        assert_eq!(forked["result"]["ok"], true, "{forked}");
        let child_id = forked["result"]["value"]["sessionId"]
            .as_str()
            .expect("child id");
        assert!(child_id.starts_with("session-"));

        let created = harness.factory.created.lock();
        assert_eq!(created.len(), 1);
        let options = &created[0];
        assert_eq!(
            options.session_id.as_ref().map(|id| id.as_str()),
            Some(child_id)
        );
        // The seed carries both completed turns plus the source's automatic
        // session event (seed events are renumbered by the store).
        assert_eq!(options.seed.as_ref().map(Vec::len), Some(5));
        let meta = options.meta.as_ref().expect("meta");
        assert_eq!(
            meta.parent_session.as_ref().map(|id| id.as_str()),
            Some("fork-src")
        );
        assert_eq!(meta.seed_length, Some(5));
        assert_eq!(meta.cwd.as_deref(), Some("D:\\proj"));
    });
}

#[test]
fn an_open_turn_anchor_is_fork_unavailable() {
    run(async {
        let harness = Harness::new();
        // One completed turn + one open turn (no turn/end for turn 2).
        harness
            .seed(
                "open-src",
                vec![
                    turn_event("turn/start", 0, 1),
                    turn_event("turn/end", 1, 1),
                    turn_event("turn/start", 2, 2),
                ],
            )
            .await;
        // atSeq inside the open turn: no boundary at or after it.
        let forked = harness
            .post(serde_json::json!({ "sessionId": "open-src", "atSeq": 2 }))
            .await;
        assert_eq!(forked["result"]["ok"], false, "{forked}");
        assert_eq!(forked["result"]["error"]["code"], "fork-unavailable");

        // No anchor falls back to the last completed turn and succeeds.
        let forked = harness
            .post(serde_json::json!({ "sessionId": "open-src" }))
            .await;
        assert_eq!(forked["result"]["ok"], true, "{forked}");
    });
}

#[test]
fn a_turnless_session_is_fork_unavailable() {
    run(async {
        let harness = Harness::new();
        harness
            .seed("empty-src", vec![turn_event("session/created", 0, 0)])
            .await;
        let forked = harness
            .post(serde_json::json!({ "sessionId": "empty-src" }))
            .await;
        assert_eq!(forked["result"]["ok"], false);
        assert_eq!(forked["result"]["error"]["code"], "fork-unavailable");
    });
}
