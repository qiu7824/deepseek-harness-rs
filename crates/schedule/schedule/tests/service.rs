//! Rust port of the core `packages/schedule/schedule/tests/{runtime,tools,plugin}.spec.ts`
//! behaviors: one-shot and fixed-rate dispatch through the maintenance
//! boundary, corrupt-log containment, and the three management tools.
//!
//! # Deviations
//!
//! - The Rust `Agent::run_maintenance` erases its result; the probe runs the
//!   task directly and the runtime reads the outcome slot.
//! - `flushSchedulePersistence` requires a real `session/flush` listener;
//!   tests register a no-op acknowledger.

use std::sync::Arc;

use cordis::{Context, Listener};
use dsh_agent::{Agent, AgentOptions, AgentRegistry, Inbox};
use dsh_llm::call_id;
use dsh_session::{
    CreateSessionMeta, CreateSessionOptions, Session, SessionEvent, SessionStore, UserMessage,
    session_id,
};
use dsh_system_prompt::SystemPrompt;
use dsh_tools::{ToolExecutionInput, ToolRuntime};

use dsh_schedule::domain::fold_schedule_events;
use dsh_schedule::runtime::ScheduleRuntime;
use dsh_schedule::tools::register_schedule_tools;

fn create_event(id: &str, kind: &str, scheduled_at: &str) -> SessionEvent {
    let schedule = match kind {
        "after" => serde_json::json!({
            "id": id, "kind": "after", "prompt": "due", "afterSeconds": 300, "scheduledAt": scheduled_at
        }),
        "every" => serde_json::json!({
            "id": id, "kind": "every", "prompt": "due", "everySeconds": 300, "scheduledAt": scheduled_at
        }),
        _ => serde_json::json!({
            "id": id, "kind": "at", "prompt": "due", "scheduledAt": scheduled_at
        }),
    };
    SessionEvent {
        type_: "schedule/change".to_string(),
        seq: 0,
        time: 1,
        data: serde_json::json!({ "version": 1, "operation": "create", "schedule": schedule }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

struct ProbeAgent {
    id: dsh_session::SessionId,
    session: Session,
    scope_key: dsh_scope::ScopeKey,
    followups: parking_lot::Mutex<Vec<UserMessage>>,
    ctx: Context,
}

impl ProbeAgent {
    fn new(id: &str, session: Session, ctx: &Context) -> Arc<Self> {
        Arc::new(Self {
            id: session_id(id),
            session,
            scope_key: dsh_scope::ScopeKey::new(),
            followups: parking_lot::Mutex::new(Vec::new()),
            ctx: ctx.clone(),
        })
    }

    fn followups(&self) -> Vec<UserMessage> {
        self.followups.lock().clone()
    }
}

impl dsh_agent::Agent for ProbeAgent {
    fn id(&self) -> &dsh_session::SessionId {
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
        static INBOX: std::sync::OnceLock<Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            Inbox::new(
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
        &self.ctx
    }

    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        &self.scope_key
    }

    fn cancel(
        &self,
        _cause: dsh_session::AgentCancelCause,
        _options: Option<&dsh_agent::CancelOptions>,
    ) {
    }

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        task()
    }

    fn send(&self, _message: UserMessage, _target: dsh_agent::InboxTarget, _wakeup: bool) {}

    fn followup(&self, message: UserMessage) {
        self.followups.lock().push(message);
    }

    fn steer(&self, _message: UserMessage) {}

    fn inject(&self, _message: UserMessage) {}
}

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

async fn setup() -> Context {
    let ctx = Context::root();
    let _system_prompt =
        SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("systemPrompt");
    let _tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let _agents = AgentRegistry::install(&ctx);
    let _store = SessionStore::install(&ctx);
    // A no-op durability acknowledger so `sessions.flush` reports a listener.
    let ack: Arc<Listener> = Arc::new(|_ctx, _args| Box::pin(async move { None }));
    ctx.on("session/flush", ack, Default::default()).await;
    ctx
}

async fn register_agent(ctx: &Context, agent: &Arc<dyn dsh_agent::Agent>) {
    let registry = ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
        .expect("agents");
    let _detach = registry.enter(agent.clone(), None).expect("enter");
    let _ = registry.announce(agent).await;
}

fn store_of(ctx: &Context) -> Arc<SessionStore> {
    ctx.get_typed::<Arc<SessionStore>>("sessions", false)
        .map(|slot| slot.as_ref().clone())
        .expect("sessions")
}

async fn agent_with_schedule(ctx: &Context, id: &str, seed: Vec<SessionEvent>) -> Arc<ProbeAgent> {
    let store = store_of(ctx);
    let session = store
        .create(
            ctx,
            Some(session_id(id)),
            Some(CreateSessionOptions {
                seed: Some(seed),
                meta: Some(CreateSessionMeta {
                    created_at: Some(1),
                    seed_length: Some(0),
                    ..Default::default()
                }),
            }),
        )
        .await
        .expect("session");
    let probe = ProbeAgent::new(id, session, ctx);
    let agent: Arc<dyn dsh_agent::Agent> = probe.clone();
    register_agent(ctx, &agent).await;
    probe
}

async fn wait_for<F: Fn() -> bool>(predicate: F) -> bool {
    for _ in 0..200 {
        if predicate() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    predicate()
}

#[tokio::test(flavor = "current_thread")]
async fn drives_one_shot_dispatch_through_maintenance() {
    let ctx = setup().await;
    let probe = agent_with_schedule(
        &ctx,
        "one-shot",
        vec![create_event(
            "schedule-1",
            "after",
            "2000-01-01T00:00:00.000Z",
        )],
    )
    .await;
    let runtime = ScheduleRuntime::new(&ctx, probe.clone());
    runtime.start();

    let dispatched = wait_for(|| !probe.followups().is_empty()).await;
    assert!(dispatched, "expected a followup reminder");
    let text = probe
        .followups()
        .into_iter()
        .map(|message| serde_json::to_string(&message).expect("message"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("[SCHEDULE REMINDER]"), "{text}");
    // The serialized envelope escapes the inner framing quotes.
    assert!(text.contains("reminder_prompt_json: \\\"due\\\""), "{text}");

    let events = probe.session().events();
    let dispatched_events: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| event.type_ == "schedule/change" && event.data["operation"] == "dispatch")
        .collect();
    assert_eq!(dispatched_events.len(), 1);
    let folded = fold_schedule_events(&events, 0).expect("fold");
    assert!(folded.active.is_empty());
    runtime.dispose().await;
}

#[tokio::test(flavor = "current_thread")]
async fn dispatches_every_batch_with_accepted_at_and_advances_the_record() {
    let ctx = setup().await;
    let probe = agent_with_schedule(
        &ctx,
        "every",
        vec![create_event(
            "schedule-every",
            "every",
            "2000-01-01T00:00:00.000Z",
        )],
    )
    .await;
    let runtime = ScheduleRuntime::new(&ctx, probe.clone());
    runtime.start();

    let dispatched = wait_for(|| !probe.followups().is_empty()).await;
    assert!(dispatched, "expected an every batch");
    let text = probe
        .followups()
        .into_iter()
        .map(|message| serde_json::to_string(&message).expect("message"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("[SCHEDULE REMINDER BATCH]"), "{text}");

    let events = probe.session().events();
    let dispatch = events
        .iter()
        .find(|event| event.type_ == "schedule/change" && event.data["operation"] == "dispatch")
        .expect("dispatch");
    assert!(dispatch.data["acceptedAt"].is_string());
    let folded = fold_schedule_events(&events, 0).expect("fold");
    assert_eq!(folded.active.len(), 1);
    assert!(folded.active[0].is_every());
    runtime.dispose().await;
}

#[tokio::test(flavor = "current_thread")]
async fn contains_a_corrupt_durable_stream_without_dispatching() {
    let ctx = setup().await;
    let corrupt = SessionEvent {
        type_: "schedule/change".to_string(),
        seq: 0,
        time: 1,
        data: serde_json::json!({ "version": 1, "operation": "delete", "id": "missing" }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    };
    let probe = agent_with_schedule(&ctx, "corrupt", vec![corrupt]).await;
    let runtime = ScheduleRuntime::new(&ctx, probe.clone());
    runtime.start();
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert!(probe.followups().is_empty());
    runtime.dispose().await;
}

#[tokio::test(flavor = "current_thread")]
async fn creates_lists_and_deletes_reminders_through_the_tools() {
    let ctx = setup().await;
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools");
    let probe = agent_with_schedule(&ctx, "tools", vec![]).await;
    let _disposer = register_schedule_tools(&ctx, probe.ctx(), probe.clone(), Arc::new(|| {}));

    let input = |name: &str, arguments: serde_json::Value| ToolExecutionInput {
        call_id: call_id(format!("c-{name}")),
        root_call_id: None,
        name: name.to_string(),
        arguments,
        agent: Some(probe.clone()),
        parent: None,
        signal: never_abort(),
    };

    // Validation errors never touch the session.
    let too_fast = tools
        .execute(input(
            "schedule_create",
            serde_json::json!({ "prompt": "x", "every_seconds": 299 }),
        ))
        .await;
    assert!(!too_fast.is_error);
    assert_eq!(
        too_fast.value.as_ref().expect("value")["code"],
        "frequency_too_high"
    );
    let two_selectors = tools
        .execute(input(
            "schedule_create",
            serde_json::json!({ "prompt": "x", "after_seconds": 10, "every_seconds": 300 }),
        ))
        .await;
    assert_eq!(
        two_selectors.value.as_ref().expect("value")["code"],
        "invalid_selector"
    );
    let blank = tools
        .execute(input(
            "schedule_create",
            serde_json::json!({ "prompt": "  ", "after_seconds": 10 }),
        ))
        .await;
    assert_eq!(
        blank.value.as_ref().expect("value")["code"],
        "invalid_prompt"
    );

    // A valid after create returns a scheduled view and appends the change.
    let created = tools
        .execute(input(
            "schedule_create",
            serde_json::json!({ "prompt": "  check logs  ", "after_seconds": 3600 }),
        ))
        .await;
    assert!(!created.is_error);
    let view = created.value.as_ref().expect("value");
    assert_eq!(view["kind"], "after");
    assert_eq!(view["prompt"], "check logs");
    assert_eq!(view["state"], "scheduled");
    assert_eq!(view["deliveryMode"], "session-local");
    let id = view["id"].as_str().expect("id").to_string();

    // List reports the active reminder in creation order.
    let listed = tools
        .execute(input("schedule_list", serde_json::json!({})))
        .await;
    assert!(!listed.is_error);
    let views = listed
        .value
        .as_ref()
        .expect("value")
        .as_array()
        .expect("array");
    assert_eq!(views.len(), 1);
    assert_eq!(views[0]["id"], view["id"]);

    // Delete removes it once and reports not-found afterward.
    let deleted = tools
        .execute(input(
            "schedule_delete",
            serde_json::json!({ "id": id.clone() }),
        ))
        .await;
    let deleted_value = deleted.value.as_ref().expect("value");
    assert_eq!(deleted_value["deleted"], true);
    let again = tools
        .execute(input("schedule_delete", serde_json::json!({ "id": id })))
        .await;
    let again_value = again.value.as_ref().expect("value");
    assert_eq!(again_value["deleted"], false);
    assert_eq!(again_value["code"], "schedule_not_found");

    let bad_id = tools
        .execute(input("schedule_delete", serde_json::json!({ "id": " x" })))
        .await;
    assert_eq!(
        bad_id.value.as_ref().expect("value")["code"],
        "invalid_rule"
    );

    // A foreign agent cannot drive this agent's tools.
    let other_session = store_of(&ctx)
        .create(
            &ctx,
            Some(session_id("other")),
            Some(CreateSessionOptions {
                seed: None,
                meta: Some(CreateSessionMeta {
                    created_at: Some(1),
                    ..Default::default()
                }),
            }),
        )
        .await
        .expect("other");
    let other = ProbeAgent::new("other", other_session, &ctx);
    let other_agent: Arc<dyn dsh_agent::Agent> = other.clone();
    register_agent(&ctx, &other_agent).await;
    let foreign = tools
        .execute(ToolExecutionInput {
            call_id: call_id("c-foreign"),
            root_call_id: None,
            name: "schedule_list".to_string(),
            arguments: serde_json::json!({}),
            agent: Some(other_agent),
            parent: None,
            signal: never_abort(),
        })
        .await;
    assert_eq!(
        foreign.value.as_ref().expect("value")["code"],
        "internal_error"
    );
}
