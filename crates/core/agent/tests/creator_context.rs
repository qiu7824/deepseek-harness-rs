use cordis::{BoxFuture, Context};
use dsh_agent::{
    Agent, AgentCancelCause, AgentFactory, AgentHandle, AgentOptions, AgentRegistry, AgentStatus,
    CancelOptions, CreateAgentOptions, Inbox, InboxTarget, ResumeAgentOptions,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, UserMessage};
use std::sync::{Arc, Weak};

struct FixtureAgent {
    ctx: Context,
    key: ScopeKey,
    session: Session,
    inbox: Inbox,
    options: AgentOptions,
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
        &self.ctx
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
fn agent(ctx: &Context, id: SessionId) -> Arc<dyn Agent> {
    let key = ScopeKey::new();
    let scope = dsh_scope::create_scope(ctx, key.clone(), &Default::default());
    let session = Session::create(id, None, None, None).unwrap();
    let inbox = Inbox::new(&session, Default::default()).unwrap();
    Arc::new(FixtureAgent {
        ctx: scope.ctx,
        key,
        session,
        inbox,
        options: Default::default(),
    })
}

struct Factory {
    ctx: Context,
    registry: Weak<AgentRegistry>,
}
impl Factory {
    async fn create(&self, owner_ctx: &Context, id: SessionId) -> Result<AgentHandle, String> {
        let registry = self.registry.upgrade().unwrap();
        // The real agent-loop factory uses this same caller accessor to bind
        // runtime ownership separately from the child's durable parent id.
        let owner = owner_ctx
            .get_typed::<Arc<dyn Agent>>("agent", false)
            .map(|slot| slot.as_ref().clone());
        let agent = agent(&self.ctx, id);
        let detach = registry.enter(agent.clone(), owner)?;
        registry.announce(&agent).await?;
        Ok(AgentHandle {
            agent,
            dispose: Box::pin(async move { detach().await }),
        })
    }
}
#[async_trait::async_trait]
impl AgentFactory for Factory {
    async fn create_agent(
        &self,
        owner_ctx: &Context,
        options: CreateAgentOptions,
    ) -> Result<AgentHandle, String> {
        self.create(owner_ctx, options.session_id.unwrap()).await
    }
    async fn resume(
        &self,
        owner_ctx: &Context,
        options: ResumeAgentOptions,
    ) -> Result<AgentHandle, String> {
        self.create(owner_ctx, options.resume_session_id.unwrap())
            .await
    }
}

#[tokio::test]
async fn explicit_creator_context_survives_create_resume_and_detaches_without_retaining_parent() {
    let ctx = Context::root();
    let registry = AgentRegistry::install(&ctx);
    let parent = agent(&ctx, dsh_session::session_id("parent"));
    let parent_weak = Arc::downgrade(&parent);
    let detach_parent = registry.enter(parent.clone(), None).unwrap();
    let _factory = registry.set_factory(Arc::new(Factory {
        ctx: ctx.clone(),
        registry: Arc::downgrade(&registry),
    }));

    let child_id = dsh_session::session_id("child");
    let child = registry
        .create_with_context(
            parent.ctx(),
            CreateAgentOptions {
                session_id: Some(child_id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        registry.is_owned_by(&child_id, &parent),
        "the idle-retirement guard must see the creator's live child"
    );
    assert!(!registry.roots().iter().any(|agent| agent.id() == &child_id));
    let AgentHandle {
        agent: child_agent,
        dispose,
    } = child;
    dispose.await;
    drop(child_agent);
    assert!(registry.get(&child_id).is_none());

    let child = registry
        .resume_with_context(
            parent.ctx(),
            ResumeAgentOptions {
                resume_session_id: Some(child_id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(registry.is_owned_by(&child_id, &parent));
    let AgentHandle {
        agent: child_agent,
        dispose,
    } = child;
    dispose.await;
    drop(child_agent);

    let ordinary = registry
        .create(CreateAgentOptions {
            session_id: Some(dsh_session::session_id("ordinary")),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        !registry.is_owned_by(ordinary.agent.id(), &parent),
        "root creation stays unowned"
    );
    let AgentHandle {
        agent: ordinary_agent,
        dispose,
    } = ordinary;
    dispose.await;
    drop(ordinary_agent);
    let ordinary = registry
        .resume(ResumeAgentOptions {
            resume_session_id: Some(dsh_session::session_id("ordinary")),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        !registry.is_owned_by(ordinary.agent.id(), &parent),
        "root resume stays unowned"
    );
    let AgentHandle {
        agent: ordinary_agent,
        dispose,
    } = ordinary;
    dispose.await;
    drop(ordinary_agent);

    detach_parent().await;
    drop(detach_parent);
    drop(parent);
    assert!(
        parent_weak.upgrade().is_none(),
        "child ownership must not retain a detached parent"
    );
    assert!(registry.list().is_empty());
}
