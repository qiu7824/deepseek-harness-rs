use std::sync::Arc;
use std::time::Duration;

use cordis::{BoxFuture, Context};
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
    InboxNotifications, InboxTarget,
};
use dsh_scope::{Scope, ScopeKey, create_scope};
use dsh_session::{Session, SessionId, SessionStore, UserMessage, session_id};
use dsh_tools::ToolRuntime;

struct FixtureAgent {
    scope: Scope,
    key: ScopeKey,
    session: Session,
    inbox: Inbox,
    options: AgentOptions,
}
impl FixtureAgent {
    fn new(ctx: &Context, id: &str) -> Arc<Self> {
        let session = Session::create(session_id(id), None, None, None).unwrap();
        let key = ScopeKey::new();
        Arc::new(Self {
            scope: create_scope(ctx, key.clone(), &Default::default()),
            key,
            inbox: Inbox::new(&session, InboxNotifications::default()).unwrap(),
            session,
            options: AgentOptions::default(),
        })
    }
}
impl Agent for FixtureAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
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
        &self.scope.ctx
    }
    fn scope_key(&self) -> &ScopeKey {
        &self.key
    }
    fn cancel(&self, _: AgentCancelCause, _: Option<&CancelOptions>) {}
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        task: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()> {
        task()
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}

fn catalog(tools: &ToolRuntime, scope: &ScopeKey) -> Vec<String> {
    tools
        .schemas(Some(scope))
        .into_iter()
        .filter(|schema| schema.name.starts_with("schedule_"))
        .map(|schema| schema.name)
        .collect()
}
fn expected() -> Vec<String> {
    ["schedule_create", "schedule_delete", "schedule_list"]
        .map(str::to_owned)
        .to_vec()
}
fn fixture() -> (Context, Arc<AgentRegistry>, Arc<ToolRuntime>) {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    SessionStore::install(&ctx);
    let agents = AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    dsh_schedule::apply(&ctx);
    (ctx, agents, tools)
}

#[tokio::test(flavor = "current_thread")]
async fn first_catalog_is_complete_before_spawned_effect_setup_can_run() {
    let (ctx, agents, tools) = fixture();
    for _ in 0..32 {
        let concrete = FixtureAgent::new(&ctx, "resumed-root");
        let agent: Arc<dyn Agent> = concrete.clone();
        let weak = Arc::downgrade(&concrete);
        let detach = agents.enter(agent.clone(), None).unwrap();
        agents.announce(&agent).await.unwrap();
        // No yield or sleep may hide the publication race. On a current-
        // thread runtime, spawned effect initialization has not run yet.
        assert_eq!(catalog(&tools, agent.scope_key()), expected());
        let scope_key = agent.scope_key().clone();
        detach().await;
        drop(detach);
        tokio::time::timeout(Duration::from_secs(5), (concrete.scope.dispose)())
            .await
            .expect("immediate retirement must drain pending effect setup");
        assert!(
            catalog(&tools, &scope_key).is_empty(),
            "real tool removal must remain visible after retirement"
        );
        drop(agent);
        drop(concrete);
        tokio::time::timeout(Duration::from_secs(5), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("schedule lifecycle retained a retired agent");
    }
    ctx.fiber.dispose().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_root_publications_have_independent_complete_catalogs() {
    let (ctx, agents, tools) = fixture();
    let lifecycles = (0..32).map(|index| {
        let ctx = ctx.clone();
        let agents = agents.clone();
        let tools = tools.clone();
        async move {
            let concrete = FixtureAgent::new(&ctx, &format!("concurrent-root-{index}"));
            let agent: Arc<dyn Agent> = concrete.clone();
            let detach = agents.enter(agent.clone(), None).unwrap();
            agents.announce(&agent).await.unwrap();
            assert_eq!(catalog(&tools, agent.scope_key()), expected());
            tokio::task::yield_now().await;
            assert_eq!(
                catalog(&tools, agent.scope_key()),
                expected(),
                "unrelated agent cleanup changed this catalog"
            );
            detach().await;
            drop(detach);
            (concrete.scope.dispose)().await;
            assert!(catalog(&tools, agent.scope_key()).is_empty());
        }
    });
    tokio::time::timeout(
        Duration::from_secs(10),
        futures::future::join_all(lifecycles),
    )
    .await
    .expect("concurrent publications and immediate cleanup settle");
    assert!(
        tools
            .schemas(None)
            .iter()
            .all(|schema| !schema.name.starts_with("schedule_")),
        "agent-owned schedule tools must never leak into the global catalog"
    );
    ctx.fiber.dispose().await;
}
