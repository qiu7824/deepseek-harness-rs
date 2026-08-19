//! E2B subprocess adapter integration: executable resolution, spawn
//! settlement through the scripted remote, the terminate ladder, and the
//! environment scrub/serialize vocabulary.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use cordis::Context;
use dsh_e2b::{
    Config, E2bBackgroundOptions, E2bCommandHandle, E2bCommandResult, E2bCreateOptions, E2bRuntime,
    E2bSandbox, E2bSdk, E2bSdkError, E2bSdkErrorKind,
};
use dsh_subprocess::{
    SubprocessCollect, SubprocessOutcome, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use dsh_subprocess_e2b::{Config as AdapterConfig, E2bSubprocessRuntime};
use futures::FutureExt;
use futures::future::BoxFuture;
use parking_lot::Mutex;

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

/// A scripted remote: state files, recorded commands, and a scripted
/// background handle.
struct FakeSandbox {
    files: Mutex<HashMap<String, Vec<u8>>>,
    runs: Mutex<Vec<String>>,
    run_results: Mutex<VecDeque<Result<E2bCommandResult, E2bSdkError>>>,
    background_results: Mutex<VecDeque<Result<i32, E2bSdkError>>>,
    kills: Mutex<Vec<String>>,
    stdin: Arc<Mutex<Vec<u8>>>,
}

impl FakeSandbox {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            files: Mutex::new(HashMap::new()),
            runs: Mutex::new(Vec::new()),
            run_results: Mutex::new(VecDeque::new()),
            background_results: Mutex::new(VecDeque::new()),
            kills: Mutex::new(Vec::new()),
            stdin: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn put(&self, path: &str, data: &[u8]) {
        self.files.lock().insert(path.to_string(), data.to_vec());
    }

    fn recorded_runs(&self) -> Vec<String> {
        self.runs.lock().clone()
    }

    fn recorded_kills(&self) -> Vec<String> {
        self.kills.lock().clone()
    }
}

#[async_trait::async_trait]
impl E2bSandbox for FakeSandbox {
    fn sandbox_id(&self) -> &str {
        "fake"
    }

    async fn make_dir(&self, _path: &str) -> Result<bool, E2bSdkError> {
        Ok(false)
    }

    async fn get_info(&self, path: &str) -> Result<dsh_e2b::E2bEntryInfo, E2bSdkError> {
        if path == "/home/user" || path.contains(".dsh-e2b") {
            return Ok(dsh_e2b::E2bEntryInfo {
                file_type: dsh_e2b::FileType::Dir,
                path: path.to_string(),
                ..Default::default()
            });
        }
        Err(E2bSdkError::not_found(path))
    }

    async fn read_bytes(&self, path: &str) -> Result<Vec<u8>, E2bSdkError> {
        self.files
            .lock()
            .get(path)
            .cloned()
            .ok_or_else(|| E2bSdkError::not_found(path))
    }

    async fn read_stream(
        &self,
        _path: &str,
    ) -> Result<Box<dyn dsh_e2b::E2bReadStream>, E2bSdkError> {
        Err(E2bSdkError::other("no streams in this fake"))
    }

    async fn list(&self, _path: &str) -> Result<Vec<dsh_e2b::E2bEntryInfo>, E2bSdkError> {
        Ok(Vec::new())
    }

    async fn write(
        &self,
        path: &str,
        content: &[u8],
        _metadata: Option<HashMap<String, String>>,
    ) -> Result<(), E2bSdkError> {
        self.put(path, content);
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> Result<dsh_e2b::E2bEntryInfo, E2bSdkError> {
        let data = self.read_bytes(from).await?;
        self.put(to, &data);
        Ok(dsh_e2b::E2bEntryInfo::default())
    }

    async fn remove(&self, _path: &str) -> Result<(), E2bSdkError> {
        Ok(())
    }

    async fn run(
        &self,
        command: &str,
        _options: &dsh_e2b::E2bCommandOptions,
    ) -> Result<E2bCommandResult, E2bSdkError> {
        self.runs.lock().push(command.to_string());
        // Control-plane no-ops never consume the scripted queue.
        if command.starts_with("chmod ") || command.starts_with("kill -") {
            return Ok(E2bCommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            });
        }
        // The remote-environment probe answers two canonical base64 lines.
        if command.starts_with("set -o pipefail; dsh_e2b_passwd=") {
            use base64::Engine as _;
            let home = base64::engine::general_purpose::STANDARD.encode("/home/user");
            let env = base64::engine::general_purpose::STANDARD.encode("PATH=/usr/bin\0");
            return Ok(E2bCommandResult {
                exit_code: 0,
                stdout: format!("{home}\n{env}\n"),
                stderr: String::new(),
            });
        }
        self.run_results
            .lock()
            .pop_front()
            .unwrap_or(Ok(E2bCommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            }))
    }

    async fn run_background(
        &self,
        command: &str,
        _options: &E2bBackgroundOptions,
    ) -> Result<Arc<dyn E2bCommandHandle>, E2bSdkError> {
        self.runs.lock().push(format!("bg: {command}"));
        let pid = self
            .background_results
            .lock()
            .pop_front()
            .unwrap_or(Ok(100))?;
        Ok(Arc::new(FakeHandle {
            pid,
            kills: Arc::new(Mutex::new(Vec::new())),
            stdin: self.stdin.clone(),
        }))
    }

    async fn kill(&self) -> Result<(), E2bSdkError> {
        Ok(())
    }
}

struct FakeHandle {
    pid: i32,
    kills: Arc<Mutex<Vec<String>>>,
    stdin: Arc<Mutex<Vec<u8>>>,
}

#[async_trait::async_trait]
impl E2bCommandHandle for FakeHandle {
    fn pid(&self) -> i32 {
        self.pid
    }

    async fn wait(&self) -> Result<E2bCommandResult, E2bSdkError> {
        Ok(E2bCommandResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    async fn kill(&self) -> Result<(), E2bSdkError> {
        self.kills.lock().push(format!("pid-{}", self.pid));
        Ok(())
    }

    async fn send_stdin(&self, data: &[u8]) -> Result<(), E2bSdkError> {
        self.stdin.lock().extend_from_slice(data);
        Ok(())
    }

    async fn close_stdin(&self) -> Result<(), E2bSdkError> {
        Ok(())
    }
}

struct FakeSdk {
    sandbox: Arc<FakeSandbox>,
    created: Arc<Mutex<bool>>,
}

#[async_trait::async_trait]
impl E2bSdk for FakeSdk {
    fn create(
        &self,
        _options: &E2bCreateOptions,
    ) -> BoxFuture<'static, Result<Arc<dyn E2bSandbox>, E2bSdkError>> {
        let sandbox = self.sandbox.clone();
        let created = self.created.clone();
        async move {
            if *created.lock() {
                return Err(E2bSdkError::other("already created"));
            }
            *created.lock() = true;
            Ok(sandbox as Arc<dyn E2bSandbox>)
        }
        .boxed()
    }
}

fn harness() -> (Context, Arc<E2bSubprocessRuntime>, Arc<FakeSandbox>) {
    let ctx = Context::root();
    let sandbox = FakeSandbox::new();
    let sdk = Arc::new(FakeSdk {
        sandbox: sandbox.clone(),
        created: Arc::new(Mutex::new(false)),
    });
    let _e2b = E2bRuntime::install(
        &ctx,
        sdk,
        Config {
            api_key: Some("test-key".to_string()),
            cwd: Some("/home/user".to_string()),
            timeout_ms: Some(60_000),
        },
        Arc::new(|_| None),
    )
    .expect("e2b runtime");
    let runtime = E2bSubprocessRuntime::install(&ctx, AdapterConfig { poll_ms: 5 })
        .expect("subprocess runtime");
    (ctx, runtime, sandbox)
}

fn spawn_spec(argv: &[&str]) -> SubprocessSpawnSpec {
    SubprocessSpawnSpec {
        argv: argv.iter().map(|arg| arg.to_string()).collect(),
        cwd: "/home/user".to_string(),
        stdio: SubprocessStdio {
            stdin: SubprocessStdinMode::Ignore,
            stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                max_bytes: 4096,
                spill: None,
            }),
            stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                max_bytes: 4096,
                spill: None,
            }),
        },
        grace_ms: 1_000,
        signal: None,
        env: None,
    }
}

#[test]
fn resolve_executable_verifies_an_absolute_program() {
    run(async {
        let (_ctx, runtime, sandbox) = harness();
        sandbox.run_results.lock().push_back(Ok(E2bCommandResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let resolved = runtime
            .resolve_executable("/usr/bin/echo", None, None)
            .await
            .expect("resolve");
        assert_eq!(resolved, "/usr/bin/echo");
        let runs = sandbox.recorded_runs();
        assert!(
            runs.iter().any(|run| run.starts_with("test -f ")),
            "{runs:?}"
        );

        // A failed verification fails the resolution.
        sandbox.run_results.lock().push_back(Err(E2bSdkError {
            kind: E2bSdkErrorKind::CommandExit { exit_code: 1 },
            message: "missing".to_string(),
            stderr: Some(String::new()),
        }));
        assert!(
            runtime
                .resolve_executable("/usr/bin/ghost", None, None)
                .await
                .is_err()
        );
    });
}

#[test]
fn resolve_executable_resolves_a_bare_path_name() {
    run(async {
        let (_ctx, runtime, sandbox) = harness();
        sandbox.run_results.lock().push_back(Ok(E2bCommandResult {
            exit_code: 0,
            stdout: "/usr/bin/printf\n".to_string(),
            stderr: String::new(),
        }));
        let resolved = runtime
            .resolve_executable("printf", None, None)
            .await
            .expect("resolve");
        assert_eq!(resolved, "/usr/bin/printf");

        // A multi-line answer is not one executable.
        sandbox.run_results.lock().push_back(Ok(E2bCommandResult {
            exit_code: 0,
            stdout: "/a\n/b\n".to_string(),
            stderr: String::new(),
        }));
        assert!(
            runtime
                .resolve_executable("multi", None, None)
                .await
                .is_err()
        );
    });
}

#[test]
fn spawn_rejects_invalid_programs_and_graces() {
    run(async {
        let (_ctx, runtime, _sandbox) = harness();
        assert!(runtime.spawn(spawn_spec(&[])).is_err());
        let mut bad_grace = spawn_spec(&["echo"]);
        bad_grace.grace_ms = 0;
        assert!(runtime.spawn(bad_grace).is_err());
    });
}

#[test]
fn spawn_settles_from_the_remote_exit_code_file() {
    run(async {
        let (_ctx, runtime, sandbox) = harness();
        let handle = runtime.spawn(spawn_spec(&["echo", "hi"])).expect("spawn");
        // The run state machine started with the constructor; wait for its
        // first durable fact before planting the remote state files.
        let state_prefix = wait_state_prefix(&sandbox).await;
        sandbox.put(&format!("{state_prefix}/pid"), b"101");
        sandbox.put(&format!("{state_prefix}/exit-code"), b"7");
        let outcome = handle.done().await.expect("outcome");
        assert_eq!(
            outcome,
            SubprocessOutcome {
                exit_code: Some(7),
                signal: None,
            }
        );
    });
}

async fn wait_state_prefix(sandbox: &Arc<FakeSandbox>) -> String {
    for _ in 0..2000 {
        let prefix = sandbox.files.lock().keys().find_map(|key| {
            key.strip_suffix("/environment")
                .map(|prefix| prefix.to_string())
        });
        if let Some(prefix) = prefix {
            return prefix;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("environment file written")
}

#[test]
fn terminate_signals_the_remote_group_after_the_grace() {
    run(async {
        let (_ctx, runtime, sandbox) = harness();
        let handle = runtime.spawn(spawn_spec(&["sleep", "60"])).expect("spawn");
        let prefix = wait_state_prefix(&sandbox).await;
        sandbox.put(&format!("{prefix}/pid"), b"202");
        sandbox.run_results.lock().push_back(Ok(E2bCommandResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        sandbox.run_results.lock().push_back(Ok(E2bCommandResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        handle.terminate();
        // The TERM ladder tick runs in a spawned task; give it headroom.
        for _ in 0..200 {
            tokio::task::yield_now().await;
            if sandbox
                .recorded_runs()
                .iter()
                .any(|run| run.contains("kill -TERM"))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let runs = sandbox.recorded_runs();
        assert!(
            runs.iter().any(|run| run.contains("kill -TERM")),
            "{runs:?}"
        );
    });
}

#[test]
fn scrub_and_serialize_environment_apply_the_seam_vocabulary() {
    let raw = "PATH=/usr/bin\0DSH_SECRET=x\0API_TOKEN=y\0KEEP=1\0";
    let scrubbed = dsh_subprocess_e2b::environment::scrub_remote_environment(raw);
    assert!(!scrubbed.contains_key("DSH_SECRET"));
    assert!(!scrubbed.contains_key("API_TOKEN"));
    assert_eq!(scrubbed.get("KEEP").map(String::as_str), Some("1"));

    let serialized = dsh_subprocess_e2b::environment::serialize_remote_environment(
        raw,
        Some(&[
            ("OVERRIDE".to_string(), Some("v".to_string())),
            ("PATH".to_string(), None),
        ]),
    )
    .expect("serialize");
    assert!(serialized.contains("OVERRIDE=v\0"), "{serialized}");
    assert!(!serialized.contains("PATH="), "{serialized}");
    assert!(!serialized.contains("API_TOKEN"), "{serialized}");
}
