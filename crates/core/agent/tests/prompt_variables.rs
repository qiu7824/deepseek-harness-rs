use std::sync::Arc;

use cordis::{BoxFuture, Context};
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxNotifications,
    InboxTarget, assemble_context_for,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, SessionStore, UserMessage, session_id};

struct TestAgent {
    id: SessionId,
    options: AgentOptions,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl Agent for TestAgent {
    fn id(&self) -> &SessionId {
        &self.id
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
        &self.scope_key
    }

    fn cancel(&self, _cause: AgentCancelCause, _options: Option<&CancelOptions>) {}

    fn followup(&self, _message: UserMessage) {}

    fn inject(&self, _message: UserMessage) {}

    fn steer(&self, _message: UserMessage) {}

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

#[tokio::test]
async fn assembly_context_exposes_agent_provider_and_model() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let id = session_id("prompt-variable-agent");
    let session = store
        .create(&ctx, Some(id.clone()), None)
        .await
        .expect("session");
    let inbox = Inbox::new(&session, InboxNotifications::default()).expect("inbox");
    let agent: Arc<dyn Agent> = Arc::new(TestAgent {
        id,
        options: AgentOptions {
            provider: Some("custom-provider".to_string()),
            model: Some("gpt-5.6-sol".to_string()),
            ..Default::default()
        },
        session,
        inbox,
        ctx,
        scope_key: ScopeKey::new(),
    });

    let assembly = assemble_context_for(&agent);
    assert_eq!(assembly.field_str("provider"), Some("custom-provider"));
    assert_eq!(assembly.field_str("model"), Some("gpt-5.6-sol"));
}
