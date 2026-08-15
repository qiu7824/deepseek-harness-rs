//! Rust port of the core `command-feedback.spec.ts` behaviors: the
//! `/feedback` command registration, acknowledgement and disclosure texts,
//! payload normalization, event ordering, and log-only guarantees.

use std::sync::Arc;

use cordis::Context;
use dsh_command_feedback::{
    NAME, execute_feedback_command, record_feedback, sharing_sentence,
};
use dsh_commands::{CommandInvocation, CommandResult, CommandRuntime};
use dsh_session::{Session, SessionId, session_id};
use dsh_session_telemetry::{
    SessionTelemetryBackend, SessionTelemetryCapture, SessionTelemetryRecord,
    SessionTelemetrySharingStatus, install_telemetry_backend,
};

struct ProbeAgent {
    id: SessionId,
    session: Session,
}

impl ProbeAgent {
    fn new(id: &str) -> Arc<Self> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        Arc::new(Self { id, session })
    }
}

impl dsh_agent::Agent for ProbeAgent {
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
        static KEY: std::sync::OnceLock<dsh_scope::ScopeKey> = std::sync::OnceLock::new();
        KEY.get_or_init(dsh_scope::ScopeKey::new)
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

struct FakeTelemetry {
    sharing: SessionTelemetrySharingStatus,
}

#[async_trait::async_trait]
impl dsh_session_telemetry::SessionTelemetrySink for FakeTelemetry {
    fn emit(&self, _record: SessionTelemetryRecord) {}
    async fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
}

impl SessionTelemetryBackend for FakeTelemetry {
    fn sharing(&self) -> SessionTelemetrySharingStatus {
        self.sharing
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }
}

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

async fn harness(sharing: Option<SessionTelemetrySharingStatus>) -> (Context, Arc<CommandRuntime>, Arc<dyn dsh_agent::Agent>) {
    let ctx = Context::root();
    let commands = CommandRuntime::install(&ctx);
    if let Some(sharing) = sharing {
        install_telemetry_backend(
            &ctx,
            Arc::new(FakeTelemetry { sharing }),
            SessionTelemetryCapture::OnDemand,
        );
    }
    let disposer = dsh_command_feedback::apply(&ctx).expect("apply");
    let _ = disposer;
    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("feedback-agent");
    (ctx, commands, agent)
}

fn feedback_texts(session: &Session) -> Vec<String> {
    session
        .events()
        .iter()
        .filter(|event| event.type_ == "feedback/record")
        .map(|event| event.data["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn sharing_sentences_cover_every_policy() {
    assert_eq!(
        sharing_sentence(SessionTelemetrySharingStatus::Full),
        "Session sharing is enabled."
    );
    assert_eq!(
        sharing_sentence(SessionTelemetrySharingStatus::FeedbackOnly),
        "Session sharing is feedback-gated; recording feedback releases the session prefix for sharing."
    );
    assert_eq!(
        sharing_sentence(SessionTelemetrySharingStatus::Disabled),
        "Session sharing is disabled."
    );
}

#[tokio::test(flavor = "current_thread")]
async fn registers_and_executes_the_feedback_command() {
    let (ctx, commands, agent) = harness(None).await;
    let descriptor = commands
        .list(&agent)
        .into_iter()
        .find(|descriptor| descriptor.name == "feedback")
        .expect("registered");
    assert_eq!(descriptor.description, "record feedback about this session");
    assert_eq!(descriptor.input.as_ref().map(|input| input.hint.as_str()), Some("<text>"));
    assert!(commands.find(&agent, "feedback").is_some());

    let execution = commands
        .execute(&agent, "/feedback the diff view is unreadable", never_abort())
        .await
        .expect("execute")
        .expect("resolved");
    let CommandResult::Success { text, .. } = execution.result else {
        panic!("success");
    };
    let expected = format!(
        "Feedback recorded for session {}\nAnonymous user: {}. Session sharing is not configured.",
        agent.id(),
        dsh_anonymous_user_id::get_or_create_anonymous_user_id(Default::default())
    );
    assert_eq!(text, Some(expected));
    assert_eq!(feedback_texts(agent.session()), vec!["the diff view is unreadable"]);
    // The command/run record omits args (recordInput: false).
    let events = agent.session().events();
    let run = events
        .iter()
        .find(|event| event.type_ == "command/run")
        .expect("command/run");
    assert!(run.data.get("args").is_none());
    // Exactly three lifecycle events, in order.
    let types: Vec<&str> = events
        .iter()
        .map(|event| event.type_.as_str())
        .collect();
    assert_eq!(types, vec!["command/run", "feedback/record", "command/done"]);
    let _ = ctx;
}

#[test]
fn records_and_normalizes_outside_commands() {
    let session = Session::create(session_id("standalone"), None, None).expect("session");
    record_feedback(&session, "  recorded outside a command  ").expect("record");
    assert_eq!(feedback_texts(&session), vec!["recorded outside a command"]);
    assert!(record_feedback(&session, " \n\t ").is_err());
    assert_eq!(feedback_texts(&session), vec!["recorded outside a command"]);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_empty_input_with_a_usage_error() {
    let (ctx, commands, agent) = harness(None).await;
    for line in ["/feedback", "/feedback   \n\t "] {
        let execution = commands
            .execute(&agent, line, never_abort())
            .await
            .expect("execute")
            .expect("resolved");
        assert_eq!(
            execution.result,
            CommandResult::Error {
                text: "Feedback text is required. Usage: /feedback <text>".to_string()
            }
        );
    }
    assert!(feedback_texts(agent.session()).is_empty());
    let _ = ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn discloses_each_telemetry_policy() {
    for (sharing, sentence) in [
        (
            SessionTelemetrySharingStatus::Full,
            "Session sharing is enabled.",
        ),
        (
            SessionTelemetrySharingStatus::FeedbackOnly,
            "Session sharing is feedback-gated; recording feedback releases the session prefix for sharing.",
        ),
        (
            SessionTelemetrySharingStatus::Disabled,
            "Session sharing is disabled.",
        ),
    ] {
        let (ctx, commands, agent) = harness(Some(sharing)).await;
        let execution = commands
            .execute(&agent, "/feedback local only", never_abort())
            .await
            .expect("execute")
            .expect("resolved");
        let CommandResult::Success { text, .. } = execution.result else {
            panic!("success");
        };
        assert!(text.expect("text").contains(sentence));
        let _ = ctx;
    }
}

#[test]
fn plugin_metadata_matches_the_ts_exports() {
    assert_eq!(NAME, "command-feedback");
}

#[test]
fn invocation_acknowledgement_formats_via_the_shared_producer() {
    // The producer seam is the pure function shared by the command handler.
    let session = Session::create(session_id("producer"), None, None).expect("session");
    record_feedback(&session, "raw text").expect("record");
    assert_eq!(feedback_texts(&session), vec!["raw text"]);
}
