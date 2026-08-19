use std::sync::Arc;

use cordis::Context;
use dsh_sandbox::{
    ConfinedSandboxMode, SandboxExecutionPolicy, SandboxMode, SandboxPolicy, SandboxProvider,
    SandboxUnavailableError,
};
use dsh_shell::{
    ShellExecRequest, ShellExecSpec, ShellExecutor, ShellProcess, ShellProcessRead,
    ShellProcessStatus, ShellRunResult, ShellSandboxInfo,
};
use dsh_subprocess::{
    CollectedOutput, SubprocessCollect, SubprocessHandle, SubprocessOutputMode,
    SubprocessOutputReader, SubprocessRuntime, SubprocessSpawnSpec, SubprocessSpill,
    SubprocessStdinMode, SubprocessStdio,
};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use parking_lot::Mutex;

pub const ENCODING_PREAMBLE: &str = "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [System.Text.UTF8Encoding]::new($false); ";

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    pub max_timeout_ms: Option<u64>,
    pub max_output_bytes: Option<u64>,
    pub max_spill_bytes: Option<u64>,
    pub grace_ms: Option<u64>,
    pub pwsh_path: Option<String>,
}

pub struct LocalPwshExecutor {
    subprocess: Arc<dyn SubprocessRuntime>,
    sandbox: Option<Arc<dyn SandboxProvider>>,
    config: Config,
}

struct PwshProcessState {
    status: Mutex<ShellProcessStatus>,
    exit_code: Mutex<Option<i32>>,
    signal: Mutex<Option<String>>,
    stdout_offset: Mutex<u64>,
    stderr_offset: Mutex<u64>,
}

struct PwshProcess {
    state: Arc<PwshProcessState>,
    handle: Arc<dyn SubprocessHandle>,
    stdout: Arc<dyn SubprocessOutputReader>,
    stderr: Arc<dyn SubprocessOutputReader>,
    done: Shared<BoxFuture<'static, ()>>,
}

struct FailedPwshProcess {
    note: Mutex<Option<String>>,
}

impl ShellProcess for FailedPwshProcess {
    fn status(&self) -> ShellProcessStatus {
        ShellProcessStatus::Killed
    }

    fn exit_code(&self) -> Option<i32> {
        None
    }

    fn signal(&self) -> Option<String> {
        None
    }

    fn done(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn sandbox(&self) -> Option<ShellSandboxInfo> {
        None
    }

    fn read_output(&self) -> ShellProcessRead {
        ShellProcessRead {
            delta: self.note.lock().take().unwrap_or_default(),
            lossy: false,
            stdout_spill_path: None,
            stderr_spill_path: None,
        }
    }

    fn kill(&self) -> bool {
        false
    }
}

fn failed_process(note: String) -> Arc<dyn ShellProcess> {
    Arc::new(FailedPwshProcess {
        note: Mutex::new(Some(note)),
    })
}

fn pwsh_argv(config: &Config, spec: &ShellExecSpec) -> Vec<String> {
    vec![
        config
            .pwsh_path
            .clone()
            .unwrap_or_else(|| "pwsh".to_string()),
        "-NoLogo".to_string(),
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-Command".to_string(),
        format!("{ENCODING_PREAMBLE}{}", spec.command),
    ]
}

fn apply_sandbox(
    sandbox: Option<&Arc<dyn SandboxProvider>>,
    argv: Vec<String>,
    execution: Option<&SandboxExecutionPolicy>,
) -> Result<Vec<String>, String> {
    let Some(execution) = execution else {
        return Ok(argv);
    };
    let mode = match execution.mode {
        SandboxMode::DangerFullAccess => return Ok(argv),
        SandboxMode::ReadOnly => ConfinedSandboxMode::ReadOnly,
        SandboxMode::WorkspaceWrite => ConfinedSandboxMode::WorkspaceWrite,
    };
    let provider = sandbox.ok_or_else(|| {
        let error = SandboxUnavailableError::new(mode, Some("sandbox service is not installed"));
        format!("[{}] {error}", error.code())
    })?;
    provider
        .confine(
            &argv,
            &SandboxPolicy {
                mode,
                workspace_root: execution.workspace_root.clone(),
                session_id: execution.session_id.clone(),
            },
        )
        .map(|confined| confined.argv)
        .map_err(|error| format!("[{}] {error}", error.code()))
}

impl ShellProcess for PwshProcess {
    fn status(&self) -> ShellProcessStatus {
        *self.state.status.lock()
    }
    fn exit_code(&self) -> Option<i32> {
        *self.state.exit_code.lock()
    }
    fn signal(&self) -> Option<String> {
        self.state.signal.lock().clone()
    }
    fn done(&self) -> BoxFuture<'static, ()> {
        self.done.clone().boxed()
    }
    fn sandbox(&self) -> Option<ShellSandboxInfo> {
        None
    }

    fn read_output(&self) -> ShellProcessRead {
        let out = self.stdout.read_from(*self.state.stdout_offset.lock());
        let err = self.stderr.read_from(*self.state.stderr_offset.lock());
        *self.state.stdout_offset.lock() = out.next_offset;
        *self.state.stderr_offset.lock() = err.next_offset;
        let separator = if !out.text.is_empty() && !out.text.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let delta = if err.text.is_empty() {
            out.text
        } else {
            format!("{}{}[stderr]\n{}", out.text, separator, err.text)
        };
        ShellProcessRead {
            delta,
            lossy: out.lossy || err.lossy,
            stdout_spill_path: out.spill_path,
            stderr_spill_path: err.spill_path,
        }
    }

    fn kill(&self) -> bool {
        let mut status = self.state.status.lock();
        if *status != ShellProcessStatus::Running {
            return false;
        }
        *status = ShellProcessStatus::Killed;
        drop(status);
        self.handle.terminate();
        true
    }
}

impl LocalPwshExecutor {
    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let subprocess = ctx
            .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
            .map(|slot| slot.as_ref().clone())
            .expect("pwsh-local requires the subprocess service");
        let sandbox = ctx
            .get_typed::<Arc<dyn SandboxProvider>>("sandbox", false)
            .map(|slot| slot.as_ref().clone());
        let executor = Arc::new(Self {
            subprocess,
            sandbox,
            config,
        });
        let erased: Arc<dyn ShellExecutor> = executor.clone();
        ctx.register_service(erased);
        executor
    }
}

impl ShellExecutor for LocalPwshExecutor {
    fn resolve(&self, request: ShellExecRequest) -> ShellExecSpec {
        ShellExecSpec {
            command: request.command,
            workdir: request
                .workdir
                .or_else(|| self.config.cwd.clone())
                .unwrap_or_else(|| {
                    std::env::current_dir()
                        .expect("current directory")
                        .to_string_lossy()
                        .into_owned()
                }),
            timeout_ms: request
                .timeout_ms
                .unwrap_or(self.config.timeout_ms.unwrap_or(120_000)),
            stdout_max_bytes: request
                .stdout_max_bytes
                .unwrap_or(self.config.max_output_bytes.unwrap_or(64_000)),
            signal: request.signal,
            stdin: request.stdin,
            env: request.env,
            dsh_env: request.dsh_env,
            sandbox_policy: request.sandbox_policy,
        }
    }

    fn run(&self, spec: ShellExecSpec) -> BoxFuture<'static, Result<ShellRunResult, String>> {
        let subprocess = self.subprocess.clone();
        let sandbox = self.sandbox.clone();
        let config = self.config.clone();
        Box::pin(async move {
            let max_output_bytes = config.max_output_bytes.unwrap_or(64_000);
            let spill = Some(SubprocessSpill {
                max_bytes: config.max_spill_bytes.unwrap_or(64 * 1024 * 1024),
            });
            let collect = |max_bytes| {
                SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes,
                    spill: spill.clone(),
                })
            };
            let mut env = vec![
                ("NO_COLOR".to_string(), Some("1".to_string())),
                ("PAGER".to_string(), Some("cat".to_string())),
                ("GIT_PAGER".to_string(), Some("cat".to_string())),
            ];
            if let Some(entries) = &spec.env {
                env.extend(
                    entries
                        .iter()
                        .cloned()
                        .map(|(key, value)| (key, Some(value))),
                );
            }
            if let Some(entries) = &spec.dsh_env {
                env.extend(
                    entries
                        .iter()
                        .cloned()
                        .map(|(key, value)| (key, Some(value))),
                );
            }
            let argv = apply_sandbox(
                sandbox.as_ref(),
                pwsh_argv(&config, &spec),
                spec.sandbox_policy.as_ref(),
            )?;
            let handle = subprocess.spawn(SubprocessSpawnSpec {
                argv,
                cwd: spec.workdir.clone(),
                stdio: SubprocessStdio {
                    stdin: spec
                        .stdin
                        .clone()
                        .map(SubprocessStdinMode::Data)
                        .unwrap_or(SubprocessStdinMode::Ignore),
                    stdout: collect(spec.stdout_max_bytes),
                    stderr: collect(max_output_bytes),
                },
                grace_ms: config.grace_ms.unwrap_or(3_000),
                signal: spec.signal.clone(),
                env: Some(env),
            })?;
            let outcome = handle.done().await?;
            let collected = handle.collected();
            let output = |reader: Arc<dyn dsh_subprocess::SubprocessOutputReader>| {
                let read = reader.read_from(0);
                CollectedOutput {
                    text: read.text,
                    truncated: read.lossy,
                    spill_path: read.spill_path,
                }
            };
            Ok(ShellRunResult {
                exit_code: outcome.exit_code,
                signal: outcome.signal,
                timed_out: false,
                aborted: false,
                timeout_ms: spec.timeout_ms,
                stdout: output(
                    collected
                        .stdout
                        .ok_or_else(|| "missing stdout collector".to_string())?,
                ),
                stderr: output(
                    collected
                        .stderr
                        .ok_or_else(|| "missing stderr collector".to_string())?,
                ),
                sandbox: None,
            })
        })
    }

    fn start(&self, spec: ShellExecSpec) -> Arc<dyn ShellProcess> {
        let max_output_bytes = self.config.max_output_bytes.unwrap_or(64_000);
        let collect = || {
            SubprocessOutputMode::Collect(SubprocessCollect {
                max_bytes: max_output_bytes,
                spill: Some(SubprocessSpill {
                    max_bytes: self.config.max_spill_bytes.unwrap_or(64 * 1024 * 1024),
                }),
            })
        };
        let mut env = vec![
            ("NO_COLOR".to_string(), Some("1".to_string())),
            ("PAGER".to_string(), Some("cat".to_string())),
            ("GIT_PAGER".to_string(), Some("cat".to_string())),
        ];
        if let Some(entries) = &spec.env {
            env.extend(
                entries
                    .iter()
                    .cloned()
                    .map(|(key, value)| (key, Some(value))),
            );
        }
        if let Some(entries) = &spec.dsh_env {
            env.extend(
                entries
                    .iter()
                    .cloned()
                    .map(|(key, value)| (key, Some(value))),
            );
        }
        let argv = match apply_sandbox(
            self.sandbox.as_ref(),
            pwsh_argv(&self.config, &spec),
            spec.sandbox_policy.as_ref(),
        ) {
            Ok(argv) => argv,
            Err(error) => return failed_process(error),
        };
        let handle = match self.subprocess.spawn(SubprocessSpawnSpec {
            argv,
            cwd: spec.workdir,
            stdio: SubprocessStdio {
                stdin: spec
                    .stdin
                    .map(SubprocessStdinMode::Data)
                    .unwrap_or(SubprocessStdinMode::Ignore),
                stdout: collect(),
                stderr: collect(),
            },
            grace_ms: self.config.grace_ms.unwrap_or(3_000),
            signal: spec.signal,
            env: Some(env),
        }) {
            Ok(handle) => handle,
            Err(error) => {
                return failed_process(format!("pwsh-local background spawn failed: {error}"));
            }
        };
        let collected = handle.collected();
        let stdout = collected.stdout.expect("requested stdout collector");
        let stderr = collected.stderr.expect("requested stderr collector");
        let state = Arc::new(PwshProcessState {
            status: Mutex::new(ShellProcessStatus::Running),
            exit_code: Mutex::new(None),
            signal: Mutex::new(None),
            stdout_offset: Mutex::new(0),
            stderr_offset: Mutex::new(0),
        });
        let done: Shared<BoxFuture<'static, ()>> = {
            let handle = handle.clone();
            let state = state.clone();
            async move {
                match handle.done().await {
                    Ok(outcome) => {
                        let _ = handle.wait_for_exit(None).await;
                        let mut status = state.status.lock();
                        if *status == ShellProcessStatus::Running {
                            *status = if outcome.signal.is_some() {
                                ShellProcessStatus::Killed
                            } else {
                                ShellProcessStatus::Completed
                            };
                        }
                        drop(status);
                        *state.exit_code.lock() = outcome.exit_code;
                        *state.signal.lock() = outcome.signal;
                    }
                    Err(_) => *state.status.lock() = ShellProcessStatus::Killed,
                }
            }
            .boxed()
            .shared()
        };
        let driven = done.clone();
        tokio::spawn(driven);
        Arc::new(PwshProcess {
            state,
            handle,
            stdout,
            stderr,
            done,
        })
    }
}
