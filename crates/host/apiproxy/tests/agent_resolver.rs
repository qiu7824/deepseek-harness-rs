//! Agent-resolver behaviors over a real registry: live reuse, the
//! session-not-found classification, and the subagent ownership fence.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
    InboxTarget,
};
use dsh_host_apiproxy::{
    AgentResolver, ApiRemoteAgentOptions, ApiRemoteAgentResult,
};
use dsh_llm::message_id;
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionHeader, SessionId, UserMessage, session_id};
use dsh_session_persistence_jsonl::{JsonlConfig, JsonlSessionPersistence};

impl std::fmt::Debug for StubAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StubAgent({})", self.id)
    }
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_path(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "dsh-apiproxy-agent-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
        .to_string_lossy()
        .into_owned()
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
    fn new(ctx: &Context, id: &str, header: Option<&SessionHeader>) -> Arc<dyn Agent> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, header).expect("session");
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

fn resolver(ctx: &Context) -> Arc<AgentResolver> {
    AgentResolver::new(
        ctx,
        ApiRemoteAgentOptions {
            agent_options: Arc::new(AgentOptions::default),
            setup: None,
        },
    )
}

#[test]
fn reuses_the_live_agent_without_resuming() {
    run(async {
        let ctx = Context::root();
        let agents = AgentRegistry::install(&ctx);
        let agent = StubAgent::new(&ctx, "live-1", None);
        register_agent(&agents, &agent).await;
        let resolver = resolver(&ctx);

        let found = resolver.resolve(&session_id("live-1")).await;
        let ApiRemoteAgentResult::Agent(found) = found else {
            panic!("expected the live agent");
        };
        assert!(Arc::ptr_eq(&found, &agent), "live identity is reused");
    });
}

#[test]
fn an_unknown_cold_identity_is_session_not_found() {
    run(async {
        let ctx = Context::root();
        AgentRegistry::install(&ctx);
        let root = temp_path("cold");
        JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.clone(),
                ..Default::default()
            },
        )
        .expect("jsonl backend");
        let resolver = resolver(&ctx);

        let found = resolver.resolve(&session_id("never-created")).await;
        match found {
            ApiRemoteAgentResult::Error(error) => {
                assert_eq!(error.code().as_str(), "session-not-found");
            }
            other => panic!("expected session-not-found, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    });
}

#[test]
fn a_subagent_owned_live_identity_is_agent_busy() {
    run(async {
        let ctx = Context::root();
        let agents = AgentRegistry::install(&ctx);
        let header = SessionHeader {
            version: 0,
            id: session_id("child-1"),
            created_at: 0,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: Some("subagent".to_string()),
            delegation_depth: None,
            agent_preset: None,
        };
        let agent = StubAgent::new(&ctx, "child-1", Some(&header));
        register_agent(&agents, &agent).await;
        let resolver = resolver(&ctx);

        let found = resolver.resolve(&session_id("child-1")).await;
        match found {
            ApiRemoteAgentResult::Error(error) => {
                assert_eq!(error.code().as_str(), "agent-busy");
                assert_eq!(error.code(), dsh_host_apiproxy::RpcErrorCode::AgentBusy);
            }
            other => panic!("expected agent-busy, got {other:?}"),
        }
    });
}

#[test]
fn a_missing_persistence_service_is_internal() {
    run(async {
        let ctx = Context::root();
        AgentRegistry::install(&ctx);
        let resolver = resolver(&ctx);
        let found = resolver.resolve(&session_id("x")).await;
        match found {
            ApiRemoteAgentResult::Error(error) => {
                assert_eq!(error.code().as_str(), "internal");
            }
            other => panic!("expected internal, got {other:?}"),
        }
    });
}
