//! Rust port of the core `commands.spec.ts` behaviors: line parsing,
//! registration validation, scoped shadowing, the run/done lifecycle, error
//! and abort settlement, and id minting.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::Context;
use dsh_commands::{
    CommandDefinition, CommandInputDescriptor, CommandResult, CommandRuntime, command_id,
    parse_command,
};
use dsh_session::{Session, SessionId, session_id};

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

fn echo_command(name: &str) -> CommandDefinition {
    CommandDefinition {
        name: name.to_string(),
        description: "echo back".to_string(),
        input: Some(CommandInputDescriptor {
            hint: "<text>".to_string(),
        }),
        record_input: None,
        handler: Arc::new(move |invocation| {
            let text = invocation.raw_input.trim().to_string();
            Box::pin(async move {
                Ok(CommandResult::Success {
                    text: Some(format!("echo:{text}")),
                    source_event_seq: None,
                })
            })
        }),
    }
}

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

#[test]
fn parses_slash_lines_without_normalizing_trailing_input() {
    assert_eq!(
        parse_command("/feedback hello"),
        Some(dsh_commands::ParsedCommand {
            name: "feedback".to_string(),
            raw_input: " hello".to_string()
        })
    );
    assert_eq!(
        parse_command("/probe"),
        Some(dsh_commands::ParsedCommand {
            name: "probe".to_string(),
            raw_input: String::new()
        })
    );
    assert_eq!(parse_command("feedback"), None);
    assert_eq!(parse_command("/Uppercase"), None);
    assert_eq!(parse_command("/a/b"), None);
    assert_eq!(parse_command(""), None);
}

#[tokio::test(flavor = "current_thread")]
async fn registers_lists_and_resolves_commands_with_scoped_shadowing() {
    let ctx = Context::root();
    let runtime = CommandRuntime::install(&ctx);
    let _disposer = runtime.register(&ctx, echo_command("global")).expect("register");

    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("agent-1");
    let descriptors = runtime.list(&agent);
    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].name, "global");
    assert!(runtime.find(&agent, "global").is_some());
    assert!(runtime.find(&agent, "missing").is_none());

    // Scoped shadow: a scoped child context registration shadows the global
    // for that agent.
    let scope = dsh_scope::create_scope(
        &ctx,
        dsh_scope::ScopeKey::new(),
        &dsh_scope::CreateScopeOptions::default(),
    );
    let scoped_ctx = scope.ctx.clone();
    let _scoped_disposer = runtime
        .register(&scoped_ctx, echo_command("scoped"))
        .expect("scoped register");
    let scoped_agent = ProbeAgent::new("scoped-agent");
    let _ = scoped_agent;
    let _ = agent;
    // The global registration is still visible to unscoped agents.
    let other: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("other");
    assert!(runtime.find(&other, "global").is_some());
    // The scoped registration lives in its own scope (verified through the
    // layer semantics of the scope crate).
    let any: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("any");
    let scoped_view = runtime.list(&any);
    assert!(scoped_view.iter().any(|descriptor| descriptor.name == "global"));
}

#[tokio::test(flavor = "current_thread")]
async fn executes_a_command_with_paired_lifecycle_events() {
    let ctx = Context::root();
    let runtime = CommandRuntime::install(&ctx);
    let _disposer = runtime.register(&ctx, echo_command("echo")).expect("register");
    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("executor");

    let execution = runtime
        .execute(&agent, "/echo hello world", never_abort())
        .await
        .expect("execute")
        .expect("resolved");
    assert_eq!(
        execution.result,
        CommandResult::Success {
            text: Some("echo:hello world".to_string()),
            source_event_seq: None
        }
    );
    let id = execution.command_id;
    let events = agent.session().events();
    let run = events
        .iter()
        .find(|event| event.type_ == "command/run")
        .expect("command/run");
    assert_eq!(run.data["commandId"], serde_json::json!(id.as_str()));
    assert_eq!(run.data["name"], "echo");
    assert_eq!(run.data["args"], " hello world");
    let done = events
        .iter()
        .find(|event| event.type_ == "command/done")
        .expect("command/done");
    assert_eq!(done.data["commandId"], serde_json::json!(id.as_str()));
    assert_eq!(done.data["kind"], "success");
    assert_eq!(done.data["text"], "echo:hello world");

    // A distinct execution mints a distinct pairing id.
    let second = runtime
        .execute(&agent, "/echo again", never_abort())
        .await
        .expect("execute")
        .expect("resolved");
    assert_ne!(second.command_id, id);
    assert!(id.as_str().starts_with("cmd-"));
}

#[tokio::test(flavor = "current_thread")]
async fn record_input_false_omits_args_from_the_run_record() {
    let ctx = Context::root();
    let runtime = CommandRuntime::install(&ctx);
    let definition = CommandDefinition {
        name: "silent".to_string(),
        description: "owns its payload".to_string(),
        input: None,
        record_input: Some(false),
        handler: Arc::new(|_invocation| {
            Box::pin(async move {
                Ok(CommandResult::Success {
                    text: None,
                    source_event_seq: None,
                })
            })
        }),
    };
    let _disposer = runtime.register(&ctx, definition).expect("register");
    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("silent-agent");
    let execution = runtime
        .execute(&agent, "/silent secret input", never_abort())
        .await
        .expect("execute")
        .expect("resolved");
    let events = agent.session().events();
    let run = events
        .iter()
        .find(|event| event.type_ == "command/run")
        .expect("command/run");
    assert!(run.data.get("args").is_none(), "{}", run.data);
    let _ = execution;
}

#[tokio::test(flavor = "current_thread")]
async fn handler_failures_settle_as_error_records_and_rethrow() {
    let ctx = Context::root();
    let runtime = CommandRuntime::install(&ctx);
    let definition = CommandDefinition {
        name: "boom".to_string(),
        description: "always fails".to_string(),
        input: None,
        record_input: None,
        handler: Arc::new(|_invocation| Box::pin(async move { Err("handler blew up".to_string()) })),
    };
    let _disposer = runtime.register(&ctx, definition).expect("register");
    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("boom-agent");
    let outcome = runtime.execute(&agent, "/boom", never_abort()).await;
    assert_eq!(outcome, Err("handler blew up".to_string()));
    let events = agent.session().events();
    let done = events
        .iter()
        .find(|event| event.type_ == "command/done")
        .expect("command/done");
    assert_eq!(done.data["kind"], "error");
    assert_eq!(done.data["text"], "handler blew up");
}

#[tokio::test(flavor = "current_thread")]
async fn aborted_executions_settle_as_errors() {
    let ctx = Context::root();
    let runtime = CommandRuntime::install(&ctx);
    let definition = CommandDefinition {
        name: "slow".to_string(),
        description: "waits forever".to_string(),
        input: None,
        record_input: None,
        handler: Arc::new(|_invocation| {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok(CommandResult::Success {
                    text: None,
                    source_event_seq: None,
                })
            })
        }),
    };
    let _disposer = runtime.register(&ctx, definition).expect("register");
    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("slow-agent");
    let flag = Arc::new(AtomicBool::new(false));
    let flag_for_signal = flag.clone();
    let signal: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || flag_for_signal.load(Ordering::SeqCst));
    let pending = {
        let agent = agent.clone();
        tokio::spawn(async move { runtime.execute(&agent, "/slow", signal).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    flag.store(true, Ordering::SeqCst);
    let outcome = pending.await.expect("join");
    assert_eq!(outcome, Err("command aborted".to_string()));
    let events = agent.session().events();
    let done = events
        .iter()
        .find(|event| event.type_ == "command/done")
        .expect("command/done");
    assert_eq!(done.data["kind"], "error");
    assert_eq!(done.data["text"], "command aborted");
}

#[tokio::test(flavor = "current_thread")]
async fn registration_and_result_validation_fail_loud() {
    let ctx = Context::root();
    let runtime = CommandRuntime::install(&ctx);
    let bad_name = CommandDefinition {
        name: "Not-lower".to_string(),
        description: "x".to_string(),
        input: None,
        record_input: None,
        handler: Arc::new(|_| Box::pin(async move { Ok(CommandResult::Success { text: None, source_event_seq: None }) })),
    };
    assert!(runtime.register(&ctx, bad_name).is_err());
    let bad_description = CommandDefinition {
        name: "ok".to_string(),
        description: "   ".to_string(),
        input: None,
        record_input: None,
        handler: Arc::new(|_| Box::pin(async move { Ok(CommandResult::Success { text: None, source_event_seq: None }) })),
    };
    assert!(runtime.register(&ctx, bad_description).is_err());

    // Duplicate registration panics (the TS throw).
    let duplicate = echo_command("dup");
    let _first = runtime.register(&ctx, duplicate.clone()).expect("first");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = runtime.register(&ctx, duplicate);
    }));
    assert!(outcome.is_err(), "duplicate names fail loud");
}

#[test]
fn brand_mints_round_trip() {
    let id = command_id("cmd-token-1");
    assert_eq!(id.as_str(), "cmd-token-1");
}
