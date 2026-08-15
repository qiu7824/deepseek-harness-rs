//! Rust port of `projection.spec.ts`: the `permissions` projection unit
//! (table options + effective current value, `custom` appended exactly
//! while derived) folded from the three knob events over the composition
//! defaults, and the `/permission` command child (switch through
//! `permission.set`, bare invocation reports, unknown names error without
//! touching the log).

use std::sync::Arc;

use cordis::{Context, arc};
use dsh_agent::{Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget};
use dsh_commands::CommandRuntime;
use dsh_permission_presets::{
    Config, PermissionPresetService, PermissionPresetsPlugin, PermissionSelect,
};
use dsh_sandbox::SandboxMode;
use dsh_scope::ScopeKey;
use dsh_session::{AgentCancelCause, Session, SessionId, SessionStore, session_id};
use dsh_session_projection::{ProjectionChangeListener, SessionProjectionRegistry};
use dsh_shell::{ShellExecutor, ShellExecRequest, ShellExecSpec, ShellProcess, ShellRunResult};
use dsh_user_approval::{ApprovalService, Config as ApprovalConfig};

struct FakeShell;

impl ShellExecutor for FakeShell {
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        Some(SandboxMode::WorkspaceWrite)
    }

    fn resolve(&self, _request: ShellExecRequest) -> ShellExecSpec {
        panic!("permission tests do not execute bash")
    }

    fn run(
        &self,
        _spec: ShellExecSpec,
    ) -> cordis::BoxFuture<'static, Result<ShellRunResult, String>> {
        panic!("permission tests do not execute bash")
    }

    fn start(&self, _spec: ShellExecSpec) -> Arc<dyn ShellProcess> {
        panic!("permission tests do not execute bash")
    }
}

/// A live-agent stand-in over the projected session; records `inject()`ed
/// messages (the `/permission` handler writes the live policy switch
/// through `approval.setPolicy`).
struct ProbeAgent {
    id: SessionId,
    session: Session,
    injected: parking_lot::Mutex<Vec<dsh_session::UserMessage>>,
}

impl ProbeAgent {
    fn new(session: &Session) -> Arc<Self> {
        Arc::new(Self {
            id: session.id().clone(),
            session: session.clone(),
            injected: parking_lot::Mutex::new(Vec::new()),
        })
    }
}

impl Agent for ProbeAgent {
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
        static INBOX: std::sync::OnceLock<Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            Inbox::new(
                &Session::create(session_id("probe"), None, None).expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Running
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }

    fn scope_key(&self) -> &ScopeKey {
        static KEY: std::sync::OnceLock<ScopeKey> = std::sync::OnceLock::new();
        KEY.get_or_init(ScopeKey::new)
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

    fn send(&self, _message: dsh_session::UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, message: dsh_session::UserMessage) {
        self.injected.lock().push(message);
    }
}

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

async fn harness(
    with_permission: bool,
) -> (Context, Session, Option<Arc<PermissionPresetService>>) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let _projections = SessionProjectionRegistry::install(&ctx);
    let _commands = CommandRuntime::install(&ctx);
    let shell: Arc<dyn ShellExecutor> = Arc::new(FakeShell);
    ctx.register_service(shell);
    let _approval = ApprovalService::install(&ctx, ApprovalConfig::default());
    let service = if with_permission {
        let service = PermissionPresetService::install(&ctx, Config::default()).expect("install");
        service.ready().await.expect("ready");
        Some(service)
    } else {
        None
    };
    let session = store
        .create(&ctx, Some(session_id("perm-projected")), None)
        .await
        .expect("create");
    (ctx, session, service)
}

fn projection_registry(ctx: &Context) -> Arc<SessionProjectionRegistry> {
    ctx.get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
        .map(|slot| slot.as_ref().clone())
        .expect("sessionProjections")
}

// ---- permissions projection unit ----

#[tokio::test(flavor = "current_thread")]
async fn serves_the_pinned_new_session_default_select() {
    let (ctx, session, _service) = harness(true).await;
    let registry = projection_registry(&ctx);
    let snapshot = registry.snapshot(&session);
    let value = snapshot.values.get("permissions").expect("key");
    assert_eq!(value["currentValue"], "workspace-write");
    let options: Vec<&str> = value["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|option| option["value"].as_str().expect("value"))
        .collect();
    assert_eq!(options, vec!["workspace-write", "danger-full-access"]);
}

#[tokio::test(flavor = "current_thread")]
async fn folds_the_knob_events_and_notifies_the_change_feed_per_knob_append() {
    let (ctx, session, service) = harness(true).await;
    let registry = projection_registry(&ctx);
    let changes: Arc<parking_lot::Mutex<Vec<(String, serde_json::Value, i64)>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let changes_for_listener = changes.clone();
    let listener: ProjectionChangeListener = Arc::new(move |_session, key, value, seq| {
        changes_for_listener
            .lock()
            .push((key.to_string(), value.clone(), seq));
    });
    let _disposer = registry.on_changed(&ctx, listener);

    service
        .expect("service")
        .set(&session, "danger-full-access")
        .expect("set");
    assert_eq!(changes.lock().len(), 3);
    let last = changes.lock().last().expect("last").clone();
    assert_eq!(last.0, "permissions");
    assert_eq!(last.1["currentValue"], "danger-full-access");
    // Unrelated event: same-reference apply, no notification.
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");
    assert_eq!(changes.lock().len(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn appends_custom_as_a_current_only_option_when_the_knobs_match_no_preset() {
    let (ctx, session, _service) = harness(true).await;
    session
        .append(
            "sandbox/mode",
            serde_json::json!({ "mode": "read-only" }),
            None,
        )
        .expect("mode");
    let registry = projection_registry(&ctx);
    let snapshot = registry.snapshot(&session);
    let value = snapshot.values.get("permissions").expect("key");
    assert_eq!(value["currentValue"], "custom");
    let last_option = value["options"].as_array().expect("options").last().expect("last");
    assert_eq!(last_option["value"], "custom");
    assert_eq!(last_option["name"], "Custom");
}

#[tokio::test(flavor = "current_thread")]
async fn has_no_permissions_key_without_the_service_and_drops_it_on_unload() {
    let (ctx, session, _service) = harness(false).await;
    let registry = projection_registry(&ctx);
    assert!(!registry.snapshot(&session).values.contains_key("permissions"));

    let fiber = ctx.plugin(Arc::new(PermissionPresetsPlugin::new(Config::default())), arc(()));
    fiber.settle().await.expect("settle");
    // The inject child registers asynchronously; poll until the key lands.
    let mut landed = false;
    for _ in 0..100 {
        if registry
            .snapshot(&session)
            .values
            .get("permissions")
            .is_some_and(|value| value["currentValue"] == "workspace-write")
        {
            landed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(landed, "the permissions unit must land after mount");

    fiber.dispose().await;
    // Poll for the release too (unload runs through the fiber chain).
    let mut dropped = false;
    for _ in 0..100 {
        if !registry.snapshot(&session).values.contains_key("permissions") {
            dropped = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(dropped, "the permissions key must drop on unload");
}

// ---- /permission command ----

#[tokio::test(flavor = "current_thread")]
async fn switches_through_permission_set_and_logs_the_lifecycle_pair() {
    let (ctx, session, _service) = harness(true).await;
    let probe = ProbeAgent::new(&session);
    let agent: Arc<dyn dsh_agent::Agent> = probe.clone();
    let commands = ctx
        .get_typed::<Arc<CommandRuntime>>("commands", false)
        .map(|slot| slot.as_ref().clone())
        .expect("commands");
    let execution = commands
        .execute(&agent, "/permission danger-full-access", never_abort())
        .await
        .expect("execute")
        .expect("resolved");
    match &execution.result {
        dsh_commands::CommandResult::Success { text, .. } => {
            assert_eq!(text.as_deref(), Some("preset danger-full-access"));
        }
        other => panic!("success expected, got {other:?}"),
    }
    let service = ctx
        .get_typed::<Arc<PermissionPresetService>>("permissionPresets", false)
        .map(|slot| slot.as_ref().clone())
        .expect("permissionPresets");
    assert_eq!(service.current(&session.events()), "danger-full-access");
    let injected = probe.injected.lock();
    assert_eq!(injected.len(), 1);
    match injected[0].content.as_slice() {
        [dsh_llm::ContentBlock::Text { text }] => assert_eq!(
            text,
            "The approval policy changed from \"ask\" to \"never\" (changed by the user)."
        ),
        _ => panic!("single text block"),
    }
    let run = session
        .events()
        .iter()
        .find(|event| event.type_ == "command/run")
        .expect("command/run")
        .clone();
    assert_eq!(run.data["name"], "permission");
    assert_eq!(run.data["args"], " danger-full-access");
}

#[tokio::test(flavor = "current_thread")]
async fn reports_the_current_preset_and_the_table_on_bare_invocation() {
    let (ctx, session, _service) = harness(true).await;
    let probe = ProbeAgent::new(&session);
    let agent: Arc<dyn dsh_agent::Agent> = probe.clone();
    let commands = ctx
        .get_typed::<Arc<CommandRuntime>>("commands", false)
        .map(|slot| slot.as_ref().clone())
        .expect("commands");
    let execution = commands
        .execute(&agent, "/permission", never_abort())
        .await
        .expect("execute")
        .expect("resolved");
    match &execution.result {
        dsh_commands::CommandResult::Success { text, .. } => assert_eq!(
            text.as_deref(),
            Some("current preset workspace-write (available: workspace-write, danger-full-access)")
        ),
        other => panic!("success expected, got {other:?}"),
    }
    assert_eq!(
        session
            .events()
            .iter()
            .filter(|event| event.type_ == "permission/preset")
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_an_unknown_preset_without_touching_the_log() {
    let (ctx, session, _service) = harness(true).await;
    let probe = ProbeAgent::new(&session);
    let agent: Arc<dyn dsh_agent::Agent> = probe.clone();
    let commands = ctx
        .get_typed::<Arc<CommandRuntime>>("commands", false)
        .map(|slot| slot.as_ref().clone())
        .expect("commands");
    let before: Vec<(String, serde_json::Value)> = session
        .events()
        .iter()
        .filter(|event| event.type_ != "command/run" && event.type_ != "command/done")
        .map(|event| (event.type_.clone(), event.data.clone()))
        .collect();
    let execution = commands
        .execute(&agent, "/permission yolo", never_abort())
        .await
        .expect("execute")
        .expect("resolved");
    match &execution.result {
        dsh_commands::CommandResult::Error { text } => assert_eq!(
            text,
            "unknown preset \"yolo\" (available: workspace-write, danger-full-access)"
        ),
        other => panic!("error expected, got {other:?}"),
    }
    let after: Vec<(String, serde_json::Value)> = session
        .events()
        .iter()
        .filter(|event| event.type_ != "command/run" && event.type_ != "command/done")
        .map(|event| (event.type_.clone(), event.data.clone()))
        .collect();
    assert_eq!(after, before);
}

// ---- select shape round-trip ----

#[test]
fn select_serializes_to_the_wire_shape() {
    let select = PermissionSelect {
        options: vec![dsh_permission_presets::PresetOption {
            value: "workspace-write".to_string(),
            name: "workspace-write".to_string(),
            description: None,
        }],
        current_value: "workspace-write".to_string(),
    };
    let json = serde_json::to_value(&select).expect("serialize");
    assert_eq!(json["currentValue"], "workspace-write");
    assert!(json["options"][0].get("description").is_none());
    dsh_permission_presets::validate_permission_select(&json).expect("valid");
    let back: PermissionSelect = serde_json::from_value(json).expect("parse");
    assert_eq!(back, select);
}
