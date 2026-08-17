//! Composition-layer `goal.*` over the real fetch carrier: the mutateGoal
//! ladder (resolver → goal service → verb → CAS ref) exercised end to end.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
    InboxTarget,
};
use dsh_goal::{Config as GoalConfig, GoalService};
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, UserMessage, session_id};
use dsh_session_persistence_jsonl::{JsonlConfig, JsonlSessionPersistence};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn cold_root() -> String {
    std::env::temp_dir()
        .join(format!(
            "dsh-apiproxy-goal-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
        .to_string_lossy()
        .into_owned()
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

/// Minimal live agent (the goal crate's test stub shape).
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
    agents: Arc<AgentRegistry>,
    agent: Arc<dyn Agent>,
    handler: dsh_host_apiproxy::FetchHandler,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let agents = AgentRegistry::install(&ctx);
        GoalService::install(&ctx, GoalConfig::default());
        let cold = cold_root();
        JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: cold,
                ..Default::default()
            },
        )
        .expect("jsonl backend");
        let agent = StubAgent::new(&ctx, "owner");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self {
            _ctx: ctx,
            agents,
            agent,
            handler,
        }
    }

    /// The agent must be live before any goal verb (runs inside the
    /// caller's runtime).
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
fn create_acknowledges_with_the_new_cas_ref_and_pause_increments_it() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        let created = harness
            .post(
                "goal.create",
                serde_json::json!({ "sessionId": "owner", "objective": "ship it" }),
            )
            .await;
        assert_eq!(created["result"]["ok"], true, "{created}");
        let goal_ref = &created["result"]["value"]["ref"];
        assert!(goal_ref["id"].as_str().expect("id").starts_with("goal-"));
        assert_eq!(goal_ref["revision"], 1);

        let paused = harness
            .post(
                "goal.pause",
                serde_json::json!({ "sessionId": "owner", "ref": goal_ref }),
            )
            .await;
        assert_eq!(paused["result"]["ok"], true, "{paused}");
        assert_eq!(paused["result"]["value"]["ref"]["revision"], 2);
        assert_eq!(
            paused["result"]["value"]["ref"]["id"],
            goal_ref["id"]
        );

        let cleared = harness
            .post(
                "goal.clear",
                serde_json::json!({
                    "sessionId": "owner",
                    "ref": paused["result"]["value"]["ref"],
                }),
            )
            .await;
        assert_eq!(cleared["result"]["ok"], true, "{cleared}");
        assert_eq!(cleared["result"]["value"]["cleared"], true);
    });
}

#[test]
fn an_unknown_session_is_session_not_found() {
    run(async {
        let harness = Harness::new();
        harness.register_owner().await;
        let created = harness
            .post(
                "goal.create",
                serde_json::json!({ "sessionId": "ghost", "objective": "x" }),
            )
            .await;
        assert_eq!(created["result"]["ok"], false);
        assert_eq!(created["result"]["error"]["code"], "session-not-found");
    });
}

#[test]
fn a_missing_goal_service_is_internal() {
    run(async {
        let ctx = Context::root();
        let agents = AgentRegistry::install(&ctx);
        let agent = StubAgent::new(&ctx, "no-goals");
        register_agent(&agents, &agent).await;
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "goal.create",
            "payload": { "sessionId": "no-goals", "objective": "x" },
        }))
        .expect("envelope");
        let response = handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/goal.create".to_string(),
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
