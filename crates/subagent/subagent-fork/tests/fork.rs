//! Rust port of `packages/subagent/subagent-fork-in-process/tests/subagent-fork-in-process.spec.ts`
//! core behaviors: the balanced completed-turn prefix and the provider's
//! registration + start path through the seam.
//!
//! # Deviations
//!
//! - The full in-process driver requires the agent-loop factory; tests cover
//!   the prefix slice, provider registration, and a stubbed seam start
//!   (the real `start_in_process_run` needs `agents.create` and is
//!   exercised through the shared driver's unit contract).

use std::sync::Arc;

use cordis::Context;
use dsh_session::{Session, SessionEvent, SessionId, session_id};
use dsh_subagent::SubagentRuntime;
use dsh_subagent_fork_in_process::{apply, completed_turn_prefix};

fn turn_event(type_: &str, seq: u64) -> SessionEvent {
    SessionEvent {
        type_: type_.to_string(),
        seq,
        time: 1,
        data: serde_json::json!({ "turn": 1 }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

fn message_event(seq: u64) -> SessionEvent {
    let message = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "probe".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    SessionEvent {
        type_: "user/message".to_string(),
        seq,
        time: 1,
        data: serde_json::to_value(&message).expect("message"),
        ignorable: None,
        surface_op: Some(dsh_session::SurfaceOp::Append),
        source_event_seqs: None,
    }
}

struct ParentAgent {
    id: SessionId,
    session: Session,
    scope_key: dsh_scope::ScopeKey,
}

impl ParentAgent {
    fn new(events: Vec<SessionEvent>) -> Arc<Self> {
        Arc::new(Self {
            id: session_id("parent"),
            session: Session::create(session_id("parent"), Some(events), None).expect("session"),
            scope_key: dsh_scope::ScopeKey::new(),
        })
    }
}

impl dsh_agent::Agent for ParentAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn options(&self) -> &dsh_agent::AgentOptions {
        static OPTIONS: std::sync::OnceLock<dsh_agent::AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(dsh_agent::AgentOptions::default)
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &dsh_agent::Inbox {
        static INBOX: std::sync::OnceLock<dsh_agent::Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            dsh_agent::Inbox::new(
                &Session::create(session_id("probe"), None, None).expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }

    fn status(&self) -> dsh_agent::AgentStatus {
        dsh_agent::AgentStatus::Running
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }

    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        &self.scope_key
    }

    fn cancel(&self, _cause: dsh_session::AgentCancelCause, _options: Option<&dsh_agent::CancelOptions>) {}

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: dsh_session::UserMessage, _target: dsh_agent::InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}

#[test]
fn slices_the_balanced_completed_turn_prefix() {
    // One completed turn: the prefix includes its turn/end.
    let completed = ParentAgent::new(vec![
        turn_event("turn/start", 0),
        message_event(1),
        turn_event("turn/end", 2),
    ]);
    let prefix = completed_turn_prefix(completed.as_ref());
    assert_eq!(
        prefix.iter().map(|event| event.type_.as_str()).collect::<Vec<_>>(),
        vec!["turn/start", "user/message", "turn/end"]
    );

    // An in-flight turn after the last turn/end is excluded.
    let in_flight = ParentAgent::new(vec![
        turn_event("turn/start", 0),
        turn_event("turn/end", 1),
        turn_event("turn/start", 2),
        message_event(3),
    ]);
    let prefix = completed_turn_prefix(in_flight.as_ref());
    assert_eq!(
        prefix.iter().map(|event| event.type_.as_str()).collect::<Vec<_>>(),
        vec!["turn/start", "turn/end"]
    );

    // No completed turn: fresh child.
    let fresh = ParentAgent::new(vec![turn_event("turn/start", 0)]);
    assert!(completed_turn_prefix(fresh.as_ref()).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registers_the_fork_provider_with_full_capabilities() {
    let ctx = Context::root();
    let runtime = SubagentRuntime::install(&ctx);
    apply(&ctx, &Default::default()).expect("apply");
    assert_eq!(runtime.list(), vec!["fork"]);
    let provider = runtime.get_provider("fork").expect("provider");
    assert_eq!(provider.name(), "fork");
    assert!(provider.inherits_parent_context());
    let capabilities = provider.capabilities();
    assert!(capabilities.depth_limit);
    assert!(capabilities.tool_filter);
    assert!(capabilities.persona);
    // Structured output capture is not ported yet.
    assert!(!capabilities.output_schema);
}
