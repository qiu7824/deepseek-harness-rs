use super::*;
use dsh_agent::{
    AgentCancelCause, AgentControlBusy, AgentControlGuard, AgentOptions, AgentStatus,
    CancelOptions, Inbox, InboxTarget,
};
use dsh_session::{SessionId, SessionStore, UserMessage};
use std::sync::atomic::{AtomicU64, Ordering};

struct TestAgent {
    ctx: Context,
    session: Session,
    inbox: Inbox,
    scope: ScopeKey,
    options: AgentOptions,
    generation: AtomicU64,
}
struct Guard;
impl AgentControlGuard for Guard {}
impl Agent for TestAgent {
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
        &self.scope
    }
    fn cancel(&self, _: AgentCancelCause, _: Option<&CancelOptions>) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
    fn cancellation_generation(&self) -> Option<u64> {
        Some(self.generation.load(Ordering::SeqCst))
    }
    fn try_generation_control(
        &self,
        expected: u64,
    ) -> Result<Box<dyn AgentControlGuard + '_>, AgentControlBusy> {
        if self.cancellation_generation() != Some(expected) {
            return Err(AgentControlBusy::Contended);
        }
        Ok(Box::new(Guard))
    }
    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        action: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        action()
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}
async fn fixture() -> (Context, Arc<CommandRuntime>, Arc<dyn Agent>) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let session = store.create(&ctx, None, None).await.unwrap();
    let agent: Arc<dyn Agent> = Arc::new(TestAgent {
        ctx: ctx.clone(),
        inbox: Inbox::new(&session, Default::default()).unwrap(),
        session,
        scope: ScopeKey::new(),
        options: AgentOptions::default(),
        generation: AtomicU64::new(0),
    });
    let commands = CommandRuntime::install(&ctx);
    (ctx, commands, agent)
}
#[tokio::test]
async fn owner_stop_cancels_a_waiting_command_and_releases_edit_protection() {
    let (ctx, commands, agent) = fixture().await;
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = started.clone();
    let _registration = commands
        .register(
            &ctx,
            CommandDefinition {
                name: "compact".into(),
                description: "Controlled wait".into(),
                input: None,
                record_input: Some(false),
                handler: Arc::new(move |_| {
                    let notify = notify.clone();
                    Box::pin(async move {
                        notify.notify_one();
                        std::future::pending().await
                    })
                }),
            },
        )
        .unwrap();
    let execute = commands.clone();
    let owner = agent.clone();
    let task = tokio::spawn(async move {
        execute
            .execute(&owner, "/compact", Arc::new(|| false))
            .await
    });
    started.notified().await;
    assert!(commands.has_owner_activity(&agent));
    agent.cancel(AgentCancelCause::User, None);
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.unwrap_err().contains("aborted"));
    assert!(!commands.has_owner_activity(&agent));
    assert_eq!(
        agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "command/done")
            .count(),
        1
    );
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
}
#[tokio::test]
async fn dropped_request_closes_command_lifecycle_without_clearing_other_requests() {
    let (ctx, commands, agent) = fixture().await;
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let notify = started.clone();
    let _registration = commands
        .register(
            &ctx,
            CommandDefinition {
                name: "wait".into(),
                description: "Controlled wait".into(),
                input: None,
                record_input: Some(false),
                handler: Arc::new(move |_| {
                    let notify = notify.clone();
                    Box::pin(async move {
                        notify.add_permits(1);
                        std::future::pending().await
                    })
                }),
            },
        )
        .unwrap();
    let spawn = || {
        let execute = commands.clone();
        let owner = agent.clone();
        tokio::spawn(async move { execute.execute(&owner, "/wait", Arc::new(|| false)).await })
    };
    let first = spawn();
    let second = spawn();
    started.acquire_many(2).await.unwrap().forget();
    first.abort();
    let _ = first.await;
    assert!(commands.has_owner_activity(&agent));
    second.abort();
    let _ = second.await;
    assert!(!commands.has_owner_activity(&agent));
    assert_eq!(
        agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "command/done")
            .count(),
        2
    );
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
}
