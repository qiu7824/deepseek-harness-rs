use std::collections::HashMap;
use std::sync::Arc;

use cordis::{Context, make_disposer};
use dsh_code_runtime::{
    CodeBindingFunction, CodeRunFailure, CodeRunFailureKind, CodeRunRequest, CodeRunResult,
    CodeRuntime,
};
use dsh_sandbox::{ConfinedSandboxMode, SandboxEnforcement, SandboxPolicy, SandboxProvider};
use dsh_subprocess::{
    SubprocessCollect, SubprocessOutputMode, SubprocessRuntime, SubprocessSpawnSpec,
    SubprocessStdinMode, SubprocessStdio,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinSet;

const RUNNER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/runner.cjs");
const RUNNER_SOURCE: &str = include_str!("../assets/runner.cjs");

#[derive(Debug, Clone)]
pub struct Config {
    pub node_command: String,
    pub runner_directory: Option<std::path::PathBuf>,
    pub compute_ms: u64,
    pub max_wall_ms: u64,
    pub max_output_bytes: u64,
    pub max_old_generation_size_mb: u64,
    /// Require an OS sandbox in addition to Node's permission model.
    pub require_os_sandbox: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            node_command: "node".to_string(),
            runner_directory: None,
            compute_ms: 60_000,
            max_wall_ms: 600_000,
            max_output_bytes: 67_108_864,
            max_old_generation_size_mb: 512,
            require_os_sandbox: false,
        }
    }
}

pub struct NodeCodeRuntime {
    subprocess: Arc<dyn SubprocessRuntime>,
    sandbox: Option<Arc<dyn SandboxProvider>>,
    config: Config,
    lifecycle: Arc<Lifecycle>,
}

struct ChildGuard {
    child: Arc<dyn dsh_subprocess::SubprocessHandle>,
    lifecycle: Arc<Lifecycle>,
    id: u64,
    armed: bool,
}

#[derive(Default)]
struct LifecycleState {
    accepting: bool,
    next_id: u64,
    active: HashMap<u64, Arc<dyn dsh_subprocess::SubprocessHandle>>,
}

struct Lifecycle {
    state: parking_lot::Mutex<LifecycleState>,
    disposal: tokio::sync::Mutex<()>,
}

impl Lifecycle {
    fn new() -> Self {
        Self {
            state: parking_lot::Mutex::new(LifecycleState {
                accepting: true,
                ..LifecycleState::default()
            }),
            disposal: tokio::sync::Mutex::new(()),
        }
    }

    fn accepting(&self) -> bool {
        self.state.lock().accepting
    }

    fn remove(&self, id: u64) {
        self.state.lock().active.remove(&id);
    }
}

impl ChildGuard {
    fn new(
        child: Arc<dyn dsh_subprocess::SubprocessHandle>,
        lifecycle: Arc<Lifecycle>,
        id: u64,
    ) -> Self {
        Self {
            child,
            lifecycle,
            id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.lifecycle.remove(self.id);
        self.armed = false;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.armed {
            self.child.terminate();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let child = self.child.clone();
                let lifecycle = self.lifecycle.clone();
                let id = self.id;
                runtime.spawn(async move {
                    let _ = child.wait_for_exit(None).await;
                    lifecycle.remove(id);
                });
            }
        }
    }
}

impl NodeCodeRuntime {
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        if config.compute_ms == 0
            || config.max_wall_ms == 0
            || config.max_output_bytes < 4
            || config.max_old_generation_size_mb == 0
        {
            return Err("code-runtime-node limits must be positive".to_string());
        }
        let subprocess = ctx
            .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "code-runtime-node requires the subprocess service".to_string())?;
        let runtime = Arc::new(Self {
            subprocess,
            sandbox: ctx
                .get_typed::<Arc<dyn SandboxProvider>>("sandbox", false)
                .map(|slot| slot.as_ref().clone()),
            config,
            lifecycle: Arc::new(Lifecycle::new()),
        });
        let teardown = runtime.clone();
        let _ = ctx.effect(
            "node code runtime teardown",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let runtime = teardown.clone();
                    Box::pin(async move { runtime.dispose().await })
                }))
            }),
        );
        let service: Arc<dyn CodeRuntime> = runtime.clone();
        ctx.register_service(service);
        Ok(runtime)
    }

    pub async fn dispose(&self) {
        let _disposal = self.lifecycle.disposal.lock().await;
        let active = {
            let mut state = self.lifecycle.state.lock();
            state.accepting = false;
            state
                .active
                .iter()
                .map(|(id, handle)| (*id, handle.clone()))
                .collect::<Vec<_>>()
        };
        for (_, handle) in &active {
            handle.terminate();
        }
        futures::future::join_all(active.iter().map(|(_, handle)| handle.wait_for_exit(None)))
            .await;
        let mut state = self.lifecycle.state.lock();
        for (id, _) in active {
            state.active.remove(&id);
        }
    }
}

impl CodeRuntime for NodeCodeRuntime {
    fn language(&self) -> String {
        "typescript".to_string()
    }

    fn isolation(&self) -> String {
        "process-worker-thread".to_string()
    }

    fn run(
        &self,
        request: CodeRunRequest,
    ) -> futures::future::BoxFuture<'static, Result<CodeRunResult, String>> {
        if !self.lifecycle.accepting() {
            return Box::pin(async { Err("code-runtime-node: runtime is disposed".to_string()) });
        }
        let subprocess = self.subprocess.clone();
        let sandbox = self.sandbox.clone();
        let config = self.config.clone();
        let lifecycle = self.lifecycle.clone();
        Box::pin(async move { run_one(subprocess, sandbox, config, lifecycle, request).await })
    }
}

async fn run_one(
    subprocess: Arc<dyn SubprocessRuntime>,
    sandbox: Option<Arc<dyn SandboxProvider>>,
    config: Config,
    lifecycle: Arc<Lifecycle>,
    request: CodeRunRequest,
) -> Result<CodeRunResult, String> {
    if request.signal.as_ref().is_some_and(|signal| signal()) {
        return Ok(failure(CodeRunFailureKind::Abort, "aborted"));
    }
    let executable = subprocess
        .resolve_executable(&config.node_command, None, request.signal.clone())
        .await
        .map_err(|error| format!("code-runtime-node: node unavailable: {error}"))?;
    let cwd = std::env::current_dir()
        .map_err(|error| format!("code-runtime-node: current directory: {error}"))?
        .to_string_lossy()
        .into_owned();
    let runner_root = config
        .runner_directory
        .as_deref()
        .unwrap_or_else(|| {
            std::path::Path::new(RUNNER)
                .parent()
                .expect("runner parent")
        })
        .to_string_lossy()
        .into_owned();
    // Resolve once, then launch exact argv. Node's permission boundary denies
    // writes, child processes and native addons even if model code recovers a
    // builtin loader. Production additionally requires the host OS sandbox.
    let mut argv = vec![
        executable,
        "--permission".to_string(),
        "--allow-worker".to_string(),
        "--no-addons".to_string(),
        "--eval".to_string(),
        RUNNER_SOURCE.to_string(),
    ];
    if config.require_os_sandbox {
        let provider = sandbox.ok_or_else(|| {
            "code-runtime-node: SANDBOX_UNAVAILABLE: OS sandbox service is required".to_string()
        })?;
        let confined = provider
            .confine(
                &argv,
                &SandboxPolicy {
                    mode: ConfinedSandboxMode::ReadOnly,
                    workspace_root: runner_root.clone(),
                    session_id: None,
                },
            )
            .map_err(|error| format!("code-runtime-node: {}: {error}", error.code()))?;
        if confined.enforcement != SandboxEnforcement::Full {
            return Err("code-runtime-node: SANDBOX_UNAVAILABLE: partial enforcement".to_string());
        }
        argv = confined.argv;
    }
    let (child, id) = {
        let mut state = lifecycle.state.lock();
        if !state.accepting {
            return Err("code-runtime-node: runtime is disposed".to_string());
        }
        let child = subprocess.spawn(SubprocessSpawnSpec {
            argv,
            cwd: if config.require_os_sandbox {
                runner_root
            } else {
                cwd
            },
            stdio: SubprocessStdio {
                stdin: SubprocessStdinMode::Pipe,
                stdout: SubprocessOutputMode::Pipe,
                stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 64 * 1024,
                    spill: None,
                }),
            },
            grace_ms: 1_000,
            signal: request.signal.clone(),
            // SubprocessRuntime supplies its canonical credential/DSH-scrubbed
            // startup environment. The runner clears process.env before it
            // creates the model Worker, after Node/AppContainer initialization
            // has consumed the Windows runtime coordinates it requires.
            env: Some(Vec::new()),
        })?;
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        state.active.insert(id, child.clone());
        (child, id)
    };
    let mut child_guard = ChildGuard::new(child.clone(), lifecycle.clone(), id);
    let mut stdin = child
        .stdin()
        .ok_or_else(|| "code-runtime-node: child stdin was not piped".to_string())?;
    let stdout = child
        .stdout()
        .ok_or_else(|| "code-runtime-node: child stdout was not piped".to_string())?;
    let mut reader = BufReader::new(stdout);

    let mut functions: HashMap<(String, String), CodeBindingFunction> = HashMap::new();
    let namespaces = request
        .bindings
        .iter()
        .map(|namespace| {
            for (name, function) in &namespace.functions {
                functions.insert((namespace.global.clone(), name.clone()), function.clone());
            }
            json!({
                "global": namespace.global,
                "names": namespace.functions.iter().map(|(name, _)| name).collect::<Vec<_>>(),
                "error_class": namespace.error_class.as_ref().map(|descriptor| json!({
                    "name": descriptor.name,
                    "member_name_property": descriptor.member_name_property,
                })),
            })
        })
        .collect::<Vec<_>>();
    write_frame(
        &mut stdin,
        &json!({
            "type": "run",
            "program": request.program,
            "namespaces": namespaces,
            "limits": {
                "compute_ms": config.compute_ms,
                "max_wall_ms": config.max_wall_ms,
                "max_output_bytes": config.max_output_bytes,
                "max_old_generation_size_mb": config.max_old_generation_size_mb,
            },
        }),
    )
    .await?;
    let output = Arc::new(tokio::sync::Mutex::new(stdin));
    let mut binding_tasks: JoinSet<Result<(), String>> = JoinSet::new();

    loop {
        let mut line = String::new();
        let read = if binding_tasks.is_empty() {
            reader
                .read_line(&mut line)
                .await
                .map_err(|error| format!("code-runtime-node: stdout read failed: {error}"))?
        } else {
            tokio::select! {
                read = reader.read_line(&mut line) => {
                    read.map_err(|error| format!("code-runtime-node: stdout read failed: {error}"))?
                }
                outcome = binding_tasks.join_next() => {
                    match outcome {
                        Some(Ok(Ok(()))) => continue,
                        Some(Ok(Err(error))) => return Err(error),
                        Some(Err(error)) => return Err(format!("code-runtime-node: binding task failed: {error}")),
                        None => continue,
                    }
                }
            }
        };
        if read == 0 {
            child.terminate();
            let _ = child.wait_for_exit(None).await;
            if request.signal.as_ref().is_some_and(|signal| signal()) {
                return Ok(failure(CodeRunFailureKind::Abort, "aborted"));
            }
            if !lifecycle.accepting() {
                return Ok(failure(CodeRunFailureKind::Abort, "runtime disposed"));
            }
            return Ok(failure(
                CodeRunFailureKind::WorkerExit,
                &format!(
                    "node runtime protocol closed; stderr: {}",
                    stderr_tail(&child)
                ),
            ));
        }
        let frame: Value = serde_json::from_str(line.trim())
            .map_err(|error| format!("code-runtime-node: invalid NDJSON: {error}"))?;
        match frame.get("type").and_then(Value::as_str) {
            Some("binding_call") => {
                let id = frame
                    .get("id")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "code-runtime-node: invalid binding id".to_string())?;
                let global = required_string(&frame, "global")?;
                let name = required_string(&frame, "name")?;
                let Some(function) = functions.get(&(global.to_string(), name.to_string())) else {
                    let mut output = output.lock().await;
                    write_frame(
                        &mut **output,
                        &json!({
                            "type": "binding_result", "id": id, "ok": false,
                            "name": "Error", "message": format!("unknown binding {global}.{name}"),
                        }),
                    )
                    .await?;
                    continue;
                };
                let function = function.clone();
                let args = frame.get("args").cloned().unwrap_or(Value::Null);
                let output = output.clone();
                binding_tasks.spawn(async move {
                    let value = function(args).await;
                    let mut output = output.lock().await;
                    write_frame(
                        &mut **output,
                        &json!({ "type": "binding_result", "id": id, "ok": true, "value": value }),
                    )
                    .await
                });
            }
            Some("complete") => {
                binding_tasks.abort_all();
                drop(output);
                let _ = child.done().await;
                let _ = child.wait_for_exit(None).await;
                child_guard.disarm();
                return parse_completion(&frame);
            }
            Some("worker_failure") | Some("protocol_failure") => {
                child.terminate();
                let _ = child.wait_for_exit(None).await;
                return Ok(failure(
                    CodeRunFailureKind::WorkerExit,
                    frame
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("node runtime failed"),
                ));
            }
            _ => {
                child.terminate();
                let _ = child.wait_for_exit(None).await;
                return Err("code-runtime-node: unknown protocol message".to_string());
            }
        }
    }
}

async fn write_frame(
    output: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    frame: &Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| format!("code-runtime-node: encode failed: {error}"))?;
    bytes.push(b'\n');
    output
        .write_all(&bytes)
        .await
        .map_err(|error| format!("code-runtime-node: stdin write failed: {error}"))?;
    output
        .flush()
        .await
        .map_err(|error| format!("code-runtime-node: stdin flush failed: {error}"))
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("code-runtime-node: missing {key}"))
}

fn parse_completion(frame: &Value) -> Result<CodeRunResult, String> {
    let error = frame.get("error").filter(|value| !value.is_null());
    let error = error.map(|error| CodeRunFailure {
        kind: match error.get("kind").and_then(Value::as_str) {
            Some("invalid-output") => CodeRunFailureKind::InvalidOutput,
            Some("output-limit") => CodeRunFailureKind::OutputLimit,
            Some("abort") => CodeRunFailureKind::Abort,
            Some("timeout") => CodeRunFailureKind::Timeout,
            Some("worker-exit") => CodeRunFailureKind::WorkerExit,
            _ => CodeRunFailureKind::Exception,
        },
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("node program failed")
            .to_string(),
    });
    let logs = frame
        .get("logs")
        .and_then(Value::as_array)
        .ok_or_else(|| "code-runtime-node: completion logs missing".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "code-runtime-node: non-string log".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let value = frame
        .get("has_value")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        .then(|| frame.get("value").cloned().unwrap_or(Value::Null));
    Ok(CodeRunResult { value, logs, error })
}

fn failure(kind: CodeRunFailureKind, message: &str) -> CodeRunResult {
    CodeRunResult {
        value: None,
        logs: Vec::new(),
        error: Some(CodeRunFailure {
            kind,
            message: message.to_string(),
        }),
    }
}

fn stderr_tail(child: &Arc<dyn dsh_subprocess::SubprocessHandle>) -> String {
    child
        .collected()
        .stderr
        .map(|reader| reader.read_from(0).text)
        .unwrap_or_default()
}
