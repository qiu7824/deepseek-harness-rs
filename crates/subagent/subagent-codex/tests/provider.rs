use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cordis::Context;
use dsh_agent::{Agent, AgentOptions};
use dsh_llm::ContentBlock;
use dsh_session::{SESSION_FORMAT_VERSION, Session, SessionHeader, SessionId, session_id};
use dsh_subagent::{SubagentRuntime, SubagentStartRequest, SubagentStopReason};
use dsh_subagent_codex::{Config, apply};
use dsh_subprocess_local::LocalSubprocessRuntime;

struct ParentAgent {
    id: SessionId,
    session: Session,
    inbox: dsh_agent::Inbox,
    ctx: Context,
    scope_key: dsh_scope::ScopeKey,
}

impl Agent for ParentAgent {
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
    fn inbox(&self) -> &dsh_agent::Inbox {
        &self.inbox
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
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn send(
        &self,
        _message: dsh_session::UserMessage,
        _target: dsh_agent::InboxTarget,
        _wakeup: bool,
    ) {
    }
    fn followup(&self, _message: dsh_session::UserMessage) {}
    fn steer(&self, _message: dsh_session::UserMessage) {}
    fn inject(&self, _message: dsh_session::UserMessage) {}
}

fn parent(cwd: &Path) -> Arc<dyn Agent> {
    let id = session_id("codex-parent");
    let header = SessionHeader {
        version: SESSION_FORMAT_VERSION,
        id: id.clone(),
        created_at: 1,
        cwd: Some(cwd.to_string_lossy().into_owned()),
        parent_session: None,
        seed_length: None,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    };
    let session = Session::create(id.clone(), None, Some(&header)).expect("parent session");
    let inbox = dsh_agent::Inbox::new(&session, Default::default()).expect("parent inbox");
    Arc::new(ParentAgent {
        id,
        session,
        inbox,
        ctx: Context::root(),
        scope_key: dsh_scope::ScopeKey::new(),
    })
}

fn test_dir(label: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("dsh-subagent-codex-{}-{label}", std::process::id(),));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create test dir");
    path
}

fn fixture_env(root: &Path, mode: &str) -> Vec<(String, String)> {
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_codex-app-server-fixture"));
    let shim_dir = root.join("bin");
    std::fs::create_dir_all(&shim_dir).expect("create shim dir");
    #[cfg(windows)]
    {
        let shim = shim_dir.join("codex.cmd");
        std::fs::write(
            shim,
            format!("@echo off\r\n\"{}\" %*\r\n", fixture.display()),
        )
        .expect("write codex shim");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let shim = shim_dir.join("codex");
        std::fs::write(
            &shim,
            format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", fixture.display()),
        )
        .expect("write codex shim");
        let mut permissions = std::fs::metadata(&shim)
            .expect("shim metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&shim, permissions).expect("make shim executable");
    }
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![shim_dir];
    paths.extend(std::env::split_paths(&inherited_path));
    let joined_path = std::env::join_paths(paths)
        .expect("join fixture PATH")
        .to_string_lossy()
        .into_owned();
    vec![
        ("PATH".to_string(), joined_path),
        ("CODEX_FIXTURE_MODE".to_string(), mode.to_string()),
        (
            "CODEX_FIXTURE_PID_FILE".to_string(),
            root.join("pid").to_string_lossy().into_owned(),
        ),
        (
            "CODEX_FIXTURE_TRACE_FILE".to_string(),
            root.join("trace").to_string_lossy().into_owned(),
        ),
    ]
}

fn request_with_signal(
    parent: Arc<dyn Agent>,
    task: &str,
    signal: Arc<dyn Fn() -> bool + Send + Sync>,
) -> SubagentStartRequest {
    SubagentStartRequest {
        label: Some("real fixture".to_string()),
        prompt: vec![ContentBlock::Text {
            text: task.to_string(),
        }],
        parent,
        signal,
        agent_options: None,
        output_schema: None,
        max_depth: None,
        tool_filter: None,
        persona: None,
    }
}

fn request(parent: Arc<dyn Agent>, task: &str) -> SubagentStartRequest {
    request_with_signal(parent, task, Arc::new(|| false))
}

fn setup(root: &Path, mode: &str) -> (Context, Arc<SubagentRuntime>, PathBuf) {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    let ctx = Context::root();
    let runtime = SubagentRuntime::install(&ctx);
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    apply(
        &ctx,
        &Config {
            env: fixture_env(root, mode),
            dispose_grace_ms: 500,
        },
    )
    .expect("register codex provider");
    (ctx, runtime, workspace)
}

fn recorded_pid(root: &Path) -> u32 {
    std::fs::read_to_string(root.join("pid"))
        .expect("fixture pid")
        .trim()
        .parse()
        .expect("numeric fixture pid")
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("query fixture process");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolved_codex_path_cannot_be_shadowed_by_the_parent_workspace() {
    let root = test_dir("workspace-shadow");
    let (_ctx, runtime, workspace) = setup(&root, "complete");
    let marker = root.join("workspace-shadow-marker");
    std::fs::write(
        workspace.join("codex.cmd"),
        format!(
            "@echo off\r\n>\"{}\" echo workspace-shadowed\r\nexit /b 41\r\n",
            marker.display()
        ),
    )
    .expect("write malicious workspace shim");

    let run = runtime
        .start(
            "codex",
            request(parent(&workspace), "use the resolved executable"),
        )
        .await
        .expect("trusted PATH shim should start");
    let result = tokio::time::timeout(Duration::from_secs(10), run.result())
        .await
        .expect("result timeout")
        .expect("result contract");

    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    assert!(!marker.exists(), "workspace codex.cmd hijacked execution");
    run.dispose().await.expect("dispose trusted child");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_app_server_fixture_returns_the_final_answer_through_the_registered_provider() {
    let root = test_dir("complete-red");
    let (_ctx, runtime, workspace) = setup(&root, "complete");

    let provider = runtime.get_provider("codex").expect("codex provider");
    assert_eq!(provider.name(), "codex");
    assert_eq!(provider.capabilities(), Default::default());
    assert!(!provider.inherits_parent_context());

    let run = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.start("codex", request(parent(&workspace), "trace this prompt")),
    )
    .await
    .expect("start timeout")
    .expect("published run");
    let result = tokio::time::timeout(Duration::from_secs(10), run.result())
        .await
        .expect("result timeout")
        .expect("result contract");

    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    assert_eq!(
        result.output,
        vec![ContentBlock::Text {
            text: "fixture answered: trace this prompt".to_string(),
        }],
    );
    run.dispose().await.expect("dispose child");
    let trace = std::fs::read_to_string(root.join("trace")).expect("fixture trace");
    assert_eq!(trace, "turn:trace this prompt\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nonzero_app_server_exit_is_a_flattened_error_result() {
    let root = test_dir("exit-9-red");
    let (_ctx, runtime, workspace) = setup(&root, "exit-9");
    let run = runtime
        .start("codex", request(parent(&workspace), "exit now"))
        .await
        .expect("published before fixture exit");

    let result = tokio::time::timeout(Duration::from_secs(10), run.result())
        .await
        .expect("result timeout")
        .expect("published result never rejects");
    assert_eq!(result.stop_reason, SubagentStopReason::Error);
    assert!(result.output.is_empty());
    run.dispose().await.expect("dispose exited child");
    assert!(!process_is_alive(recorded_pid(&root)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispose_aborts_the_result_and_kills_the_real_app_server_process() {
    let root = test_dir("dispose-kill-red");
    let (_ctx, runtime, workspace) = setup(&root, "hold");
    let aborted = Arc::new(AtomicBool::new(false));
    let signal_flag = aborted.clone();
    let signal: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || signal_flag.load(Ordering::SeqCst));
    let run = runtime
        .start(
            "codex",
            request_with_signal(parent(&workspace), "wait forever", signal),
        )
        .await
        .expect("published hold run");

    let pid = recorded_pid(&root);
    assert!(
        process_is_alive(pid),
        "fixture must be alive before dispose"
    );
    run.dispose().await.expect("dispose hold run");
    let result = tokio::time::timeout(Duration::from_secs(10), run.result())
        .await
        .expect("result timeout")
        .expect("published result never rejects");
    assert_eq!(result.stop_reason, SubagentStopReason::Aborted);
    assert!(
        !process_is_alive(pid),
        "dispose must await real process exit"
    );
    let trace = std::fs::read_to_string(root.join("trace")).expect("fixture trace");
    assert!(trace.lines().any(|line| line == "interrupt"), "{trace:?}");
}
