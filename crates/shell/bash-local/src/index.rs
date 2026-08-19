//! Local Service Provider for the bash capability seam over the subprocess
//! capability seam. Public commands run as `bash -c` in a managed process
//! group spawned through `ctx.subprocess`. This executor owns command
//! defaulting, deadlines and cause classification, the model-friendly
//! terminal environment, and the model-facing stdout/stderr merge for
//! background reads. Execution policy belongs in `tools/pre-execute` or a
//! sandboxing executor. Rust port of
//! `packages/shell/bash-local/src/index.ts`.
//!
//! # Deviations
//!
//! - The TS `protected onProcessDone` settlement hook becomes the injectable
//!   [`LocalBashExecutor::set_on_process_done`] hook (Rust has no
//!   subclassing; a sandboxing wrapper installs its own).
//! - The abort predicate is polled every 15 ms (no event target in Rust) and
//!   cancels the fused deadline signal directly.

use std::sync::Arc;
use std::time::Duration;

use cordis::{Context, FiberCore, Service};
use dsh_schemastery::{Data, Schema};
use dsh_settings::{SettingsSectionHooks, install_settings_section};
use dsh_shell::{
    ShellExecRequest, ShellExecSpec, ShellExecutor, ShellProcess, ShellProcessRead,
    ShellProcessStatus, ShellRunResult, ShellSandboxInfo, shell_settings_namespace,
};
use dsh_subprocess::{
    CollectedOutput, DSH_ENV_PREFIX, SubprocessAbort, SubprocessCollect, SubprocessHandle,
    SubprocessOutputMode, SubprocessOutputReader, SubprocessRuntime, SubprocessSpawnSpec,
    SubprocessSpill, SubprocessStdinMode, SubprocessStdio,
};
use dsh_timeout::{DeadlineSignal, MAX_TIMER_DELAY_MS, clamp_timeout, deadline, timeout_of};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use indexmap::IndexMap;
use parking_lot::Mutex;

/// Model-friendly environment overrides: disable colors, pagers, and
/// interactive terminal features that would garble tool output (TS
/// `ENV_OVERRIDES`). Merged first into the spawn's explicit env, so a trusted
/// caller's own entry still wins.
pub const ENV_OVERRIDES: [(&str, &str); 4] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
];

/// Default SIGTERM鈫扴IGKILL grace period (the `graceMs` config).
pub const DEFAULT_GRACE_MS: u64 = 3_000;

/// Default per-stream spill cap (the `maxSpillBytes` config).
pub const DEFAULT_MAX_SPILL_BYTES: u64 = 64 * 1024 * 1024;

/// Plugin config (all optional 鈥?defaults below).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Default working directory for commands (default: process cwd).
    pub cwd: Option<String>,
    /// Default foreground timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Upper bound for per-call timeout overrides.
    pub max_timeout_ms: Option<u64>,
    /// Per-stream in-memory output cap; overflow spills to a temp file.
    pub max_output_bytes: Option<u64>,
    /// Per-stream spill-file cap; larger streams retain only their in-memory
    /// tail.
    pub max_spill_bytes: Option<u64>,
    /// Grace period for kill escalation and inherited pipes; at most
    /// `MAX_TIMER_DELAY_MS`.
    pub grace_ms: Option<u64>,
}

/// The defaults-applied config shape (TS `ResolvedConfig`).
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub cwd: Option<String>,
    pub timeout_ms: u64,
    pub max_timeout_ms: u64,
    pub max_output_bytes: u64,
    pub max_spill_bytes: u64,
    pub grace_ms: u64,
}

impl ResolvedConfig {
    /// Apply defaults to a composition entry and validate it (the TS
    /// schemastery-defaults + `assertServiceableBashConfig` constructor
    /// steps collapsed).
    pub fn resolve(config: &Config) -> Result<Self, String> {
        let resolved = Self {
            cwd: config.cwd.clone(),
            timeout_ms: config.timeout_ms.unwrap_or(120_000),
            max_timeout_ms: config.max_timeout_ms.unwrap_or(600_000),
            max_output_bytes: config.max_output_bytes.unwrap_or(64_000),
            max_spill_bytes: config.max_spill_bytes.unwrap_or(DEFAULT_MAX_SPILL_BYTES),
            grace_ms: config.grace_ms.unwrap_or(DEFAULT_GRACE_MS),
        };
        assert_serviceable_bash_config(&resolved)?;
        Ok(resolved)
    }

    /// Project a resolved settings section (schema-valid, defaults applied
    /// by the base layer) back into the typed shape.
    fn from_data(value: &Data) -> Option<Self> {
        let Data::Object(object) = value else {
            return None;
        };
        let number = |key: &str| match object.get(key) {
            Some(Data::Number(value)) => Some(*value as u64),
            _ => None,
        };
        let string = |key: &str| match object.get(key) {
            Some(Data::String(value)) => Some(value.clone()),
            _ => None,
        };
        let resolved = Self {
            cwd: string("cwd"),
            timeout_ms: number("timeoutMs")?,
            max_timeout_ms: number("maxTimeoutMs")?,
            max_output_bytes: number("maxOutputBytes")?,
            max_spill_bytes: number("maxSpillBytes")?,
            grace_ms: number("graceMs")?,
        };
        Some(resolved)
    }

    fn to_data(&self) -> Data {
        let mut object = IndexMap::new();
        if let Some(cwd) = &self.cwd {
            object.insert("cwd".to_string(), Data::String(cwd.clone()));
        }
        object.insert(
            "timeoutMs".to_string(),
            Data::Number(self.timeout_ms as f64),
        );
        object.insert(
            "maxTimeoutMs".to_string(),
            Data::Number(self.max_timeout_ms as f64),
        );
        object.insert(
            "maxOutputBytes".to_string(),
            Data::Number(self.max_output_bytes as f64),
        );
        object.insert(
            "maxSpillBytes".to_string(),
            Data::Number(self.max_spill_bytes as f64),
        );
        object.insert("graceMs".to_string(), Data::Number(self.grace_ms as f64));
        Data::Object(object)
    }
}

fn assert_positive_finite(name: &str, value: u64) -> Result<(), String> {
    if value == 0 {
        return Err(format!(
            "bash-local: {name} must be a positive finite number"
        ));
    }
    Ok(())
}

/// Reject a resolved section this executor could not run with (TS
/// `assertServiceableBashConfig`).
pub fn assert_serviceable_bash_config(config: &ResolvedConfig) -> Result<(), String> {
    assert_positive_finite("timeoutMs", config.timeout_ms)?;
    assert_positive_finite("maxTimeoutMs", config.max_timeout_ms)?;
    assert_positive_finite("maxOutputBytes", config.max_output_bytes)?;
    assert_positive_finite("maxSpillBytes", config.max_spill_bytes)?;
    assert_positive_finite("graceMs", config.grace_ms)?;
    if config.grace_ms > MAX_TIMER_DELAY_MS {
        return Err(format!(
            "bash-local: graceMs must be no greater than {MAX_TIMER_DELAY_MS}"
        ));
    }
    Ok(())
}

/// The settings section schema (every field optional; the composition entry
/// carries the defaults as the base layer 鈥?the TS `z.number().default(...)`
/// collapse).
pub fn bash_config_schema() -> Schema {
    let mut properties = IndexMap::new();
    properties.insert("cwd".to_string(), Schema::string());
    properties.insert("timeoutMs".to_string(), Schema::number());
    properties.insert("maxTimeoutMs".to_string(), Schema::number());
    properties.insert("maxOutputBytes".to_string(), Schema::number());
    properties.insert("maxSpillBytes".to_string(), Schema::number());
    properties.insert("graceMs".to_string(), Schema::number());
    Schema::object(properties)
}

/// Project a settled collect-mode reader into the final `CollectedOutput`
/// shape (TS `finalOutput`).
fn final_output(reader: &dyn SubprocessOutputReader) -> CollectedOutput {
    let read = reader.read_from(0);
    CollectedOutput {
        text: read.text,
        truncated: read.lossy,
        spill_path: read.spill_path,
    }
}

/// Settlement facts a `on_process_done` hook may stamp (the TS `proc` view
/// handed to the protected hook).
pub struct BashProcessFacts {
    state: Arc<BashProcessState>,
}

impl BashProcessFacts {
    /// Stamp sandbox execution facts for the settled process (absent for the
    /// unsandboxed base executor).
    pub fn set_sandbox(&self, info: ShellSandboxInfo) {
        *self.state.sandbox.lock() = Some(info);
    }
}

/// Shared mutable state of one background process handle.
struct BashProcessState {
    status: Mutex<ShellProcessStatus>,
    exit_code: Mutex<Option<i32>>,
    signal: Mutex<Option<String>>,
    spawn_failure_note: Mutex<Option<String>>,
    stdout_offset: Mutex<u64>,
    stderr_offset: Mutex<u64>,
    sandbox: Mutex<Option<ShellSandboxInfo>>,
}

impl BashProcessState {
    fn new() -> Self {
        Self {
            status: Mutex::new(ShellProcessStatus::Running),
            exit_code: Mutex::new(None),
            signal: Mutex::new(None),
            spawn_failure_note: Mutex::new(None),
            stdout_offset: Mutex::new(0),
            stderr_offset: Mutex::new(0),
            sandbox: Mutex::new(None),
        }
    }
}

/// A live background process handle (TS `ShellProcess`).
struct BashShellProcess {
    state: Arc<BashProcessState>,
    subprocess: Arc<dyn SubprocessHandle>,
    stdout: Arc<dyn SubprocessOutputReader>,
    stderr: Arc<dyn SubprocessOutputReader>,
    done: Shared<BoxFuture<'static, ()>>,
    abort_predicate: Option<SubprocessAbort>,
}

impl BashShellProcess {
    /// Build the consuming stdout/stderr merge of one read (shared by the
    /// live and failed-spawn handles).
    fn merged_read(
        state: &Arc<BashProcessState>,
        stdout: Option<&dyn SubprocessOutputReader>,
        stderr: Option<&dyn SubprocessOutputReader>,
    ) -> ShellProcessRead {
        let (out, err) = match (stdout, stderr) {
            (Some(stdout), Some(stderr)) => (
                stdout.read_from(*state.stdout_offset.lock()),
                stderr.read_from(*state.stderr_offset.lock()),
            ),
            // A failed spawn never produced process output.
            _ => (
                dsh_subprocess::SubprocessOutputRead {
                    text: String::new(),
                    next_offset: 0,
                    lossy: false,
                    spill_path: None,
                },
                dsh_subprocess::SubprocessOutputRead {
                    text: String::new(),
                    next_offset: 0,
                    lossy: false,
                    spill_path: None,
                },
            ),
        };
        *state.stdout_offset.lock() = out.next_offset;
        *state.stderr_offset.lock() = err.next_offset;
        // A failed spawn never produced process output, so the note and real
        // stderr text are mutually exclusive.
        let err_text = if !err.text.is_empty() {
            err.text
        } else {
            state.spawn_failure_note.lock().take().unwrap_or_default()
        };
        // Single newline between sections: stdout chunks usually end with one
        // already; add it only when missing.
        let separator = if !out.text.is_empty() && !out.text.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let delta = if err_text.is_empty() {
            out.text
        } else {
            format!("{}{}[stderr]\n{}", out.text, separator, err_text)
        };
        ShellProcessRead {
            delta,
            lossy: out.lossy || err.lossy,
            stdout_spill_path: out.spill_path,
            stderr_spill_path: err.spill_path,
        }
    }
}

impl ShellProcess for BashShellProcess {
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
        self.state.sandbox.lock().clone()
    }

    fn read_output(&self) -> ShellProcessRead {
        Self::merged_read(
            &self.state,
            Some(self.stdout.as_ref()),
            Some(self.stderr.as_ref()),
        )
    }

    fn kill(&self) -> bool {
        {
            let mut status = self.state.status.lock();
            if *status != ShellProcessStatus::Running {
                return false;
            }
            *status = ShellProcessStatus::Killed;
        }
        self.subprocess.terminate();
        true
    }
}

/// The handle for a spawn that failed before any process ran (the TS
/// rejected-`done` branch collapsed into a settled `killed` handle).
struct FailedShellProcess {
    state: Arc<BashProcessState>,
    done: Shared<BoxFuture<'static, ()>>,
}

impl ShellProcess for FailedShellProcess {
    fn status(&self) -> ShellProcessStatus {
        *self.state.status.lock()
    }

    fn exit_code(&self) -> Option<i32> {
        None
    }

    fn signal(&self) -> Option<String> {
        None
    }

    fn done(&self) -> BoxFuture<'static, ()> {
        self.done.clone().boxed()
    }

    fn sandbox(&self) -> Option<ShellSandboxInfo> {
        None
    }

    fn read_output(&self) -> ShellProcessRead {
        BashShellProcess::merged_read(&self.state, None, None)
    }

    fn kill(&self) -> bool {
        false
    }
}

/// A settled `killed` handle carrying a one-shot note (spawn failures and
/// dropped collect streams 鈥?the TS rejected-`done` branch collapse).
fn failed_process(note: String) -> Arc<dyn ShellProcess> {
    let state = Arc::new(BashProcessState::new());
    *state.status.lock() = ShellProcessStatus::Killed;
    *state.spawn_failure_note.lock() = Some(note);
    let done: Shared<BoxFuture<'static, ()>> = Box::pin(async {}).boxed().shared();
    Arc::new(FailedShellProcess { state, done })
}

/// Local bash executor over `ctx.subprocess` (TS `LocalBashExecutor`).
pub struct LocalBashExecutor {
    pub ctx: Context,
    subprocess: Arc<dyn SubprocessRuntime>,
    /// The composition entry the settings section layers over.
    fallback: ResolvedConfig,
    /// The authoritative settings source thunk (None until the wiring
    /// attaches; the provider detach falls back to the entry).
    source: Arc<Mutex<Option<Arc<dyn Fn() -> Data + Send + Sync>>>>,
    /// The TS `protected onProcessDone` settlement hook (injectable).
    on_process_done:
        Mutex<Arc<dyn Fn(&BashProcessFacts, String, bool, Option<String>) + Send + Sync>>,
    /// The settings-wiring inject fiber; `ready()` awaits its settle.
    wiring: Mutex<Option<Arc<FiberCore>>>,
}

impl Service for LocalBashExecutor {
    fn service_name(&self) -> &'static str {
        "shell"
    }
}

impl LocalBashExecutor {
    /// Construct, validate, register as `ctx.shell`, and wire the optional
    /// settings section (the TS constructor collapse). Panics on an invalid
    /// entry or a missing `subprocess` service (TS constructor throw + static
    /// inject).
    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let resolved = ResolvedConfig::resolve(&config).unwrap_or_else(|error| panic!("{error}"));
        let subprocess = ctx
            .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
            .map(|slot| slot.as_ref().clone())
            .expect("bash-local requires the subprocess service");
        let executor = Arc::new(Self {
            ctx: ctx.clone(),
            subprocess,
            fallback: resolved.clone(),
            source: Arc::new(Mutex::new(None)),
            on_process_done: Mutex::new(Arc::new(|_facts, _stderr, _spawn_failed, _error| {})),
            wiring: Mutex::new(None),
        });
        let erased: Arc<dyn ShellExecutor> = executor.clone();
        ctx.register_service(erased);

        let entry = resolved.to_data();
        let source_sink = Arc::clone(&executor.source);
        let wiring = install_settings_section(
            ctx,
            shell_settings_namespace().clone(),
            bash_config_schema(),
            entry,
            SettingsSectionHooks {
                set_source: Arc::new(move |source| {
                    // Every field is read through the getter at each command,
                    // so nothing derived from the source needs rebuilding.
                    *source_sink.lock() = Some(source);
                }),
                on_change: Arc::new(|| {}),
                validate: Some(Arc::new(|data: &Data| {
                    let resolved = ResolvedConfig::from_data(data).ok_or_else(|| {
                        "bash-local: settings section is missing required fields".to_string()
                    })?;
                    assert_serviceable_bash_config(&resolved)
                })),
            },
        );
        *executor.wiring.lock() = Some(wiring);
        executor
    }

    /// Await the settings-section wiring (TS plugin-load timing completes
    /// the wiring synchronously; Rust attaches it through an inject fiber).
    pub async fn ready(&self) -> Result<(), cordis::PluginError> {
        let wiring = { self.wiring.lock().clone() };
        if let Some(wiring) = wiring {
            wiring.settle().await?;
        }
        Ok(())
    }

    /// Install the settlement hook (the TS `protected onProcessDone`
    /// override).
    pub fn set_on_process_done(
        &self,
        hook: Arc<dyn Fn(&BashProcessFacts, String, bool, Option<String>) + Send + Sync>,
    ) {
        *self.on_process_done.lock() = hook;
    }

    /// The currently authoritative config: the settings section, or the
    /// composition entry (TS `get config`).
    pub fn config(&self) -> ResolvedConfig {
        let source = { self.source.lock().clone() };
        match source {
            Some(source) => {
                ResolvedConfig::from_data(&source()).unwrap_or_else(|| self.fallback.clone())
            }
            None => self.fallback.clone(),
        }
    }

    /// Map one resolved bash spec and explicit argv onto a fully-specified
    /// subprocess spawn (TS `spawnSpec`).
    fn spawn_spec(
        config: &ResolvedConfig,
        spec: &ShellExecSpec,
        argv: Vec<String>,
        stdout_max_bytes: u64,
        signal: Option<SubprocessAbort>,
    ) -> SubprocessSpawnSpec {
        let collect = |max_bytes: u64| SubprocessCollect {
            max_bytes,
            spill: Some(SubprocessSpill {
                max_bytes: config.max_spill_bytes,
            }),
        };
        // One explicit env map for the seam, layered so the trusted dshEnv
        // snapshot beats both the caller's env and the terminal overrides.
        let mut env: Vec<(String, Option<String>)> = ENV_OVERRIDES
            .iter()
            .map(|(key, value)| (key.to_string(), Some(value.to_string())))
            .collect();
        if let Some(entries) = &spec.env {
            env.extend(
                entries
                    .iter()
                    .cloned()
                    .map(|(key, value)| (key, Some(value))),
            );
        }
        if let Some(dsh_env) = &spec.dsh_env {
            env.extend(
                dsh_env
                    .iter()
                    .cloned()
                    .map(|(key, value)| (key, Some(value))),
            );
        }
        SubprocessSpawnSpec {
            argv,
            cwd: spec.workdir.clone(),
            stdio: SubprocessStdio {
                stdin: match &spec.stdin {
                    Some(data) => SubprocessStdinMode::Data(data.clone()),
                    None => SubprocessStdinMode::Ignore,
                },
                stdout: SubprocessOutputMode::Collect(collect(stdout_max_bytes)),
                stderr: SubprocessOutputMode::Collect(collect(config.max_output_bytes)),
            },
            grace_ms: config.grace_ms,
            signal,
            env: Some(env),
        }
    }

    /// Run an explicit argv with the foreground lifecycle (TS `runArgv`).
    pub fn run_argv(
        &self,
        spec: ShellExecSpec,
        argv: Vec<String>,
    ) -> BoxFuture<'static, Result<ShellRunResult, String>> {
        let config = self.config();
        let subprocess = self.subprocess.clone();
        Box::pin(async move {
            // One deadline combines timeout and upstream cancellation;
            // disposal clears its timer.
            let upstream = DeadlineSignal::never();
            let mut deadline = deadline(Some(&upstream), spec.timeout_ms, "BASH_TIMEOUT");
            let fused = Arc::new(std::mem::replace(
                &mut deadline.signal,
                DeadlineSignal::never(),
            ));
            // Bridge the caller's abort predicate onto the fused signal.
            let poller = spec.signal.as_ref().map(|abort| {
                let abort = abort.clone();
                let fused_for_poll = fused.clone();
                tokio::spawn(async move {
                    loop {
                        if abort() {
                            fused_for_poll.cancel(None);
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(15)).await;
                    }
                })
            });
            let spawn_signal: SubprocessAbort = {
                let fused = fused.clone();
                Arc::new(move || fused.is_cancelled())
            };
            let spawn = Self::spawn_spec(
                &config,
                &spec,
                argv,
                spec.stdout_max_bytes,
                Some(spawn_signal),
            );
            let handle = subprocess.spawn(spawn)?;
            let outcome = handle.done().await?;
            if let Some(poller) = poller {
                poller.abort();
            }
            let collected = handle.collected();
            let stdout = collected.stdout.ok_or_else(|| {
                "bash-local: subprocess implementation dropped a requested collect stream"
                    .to_string()
            })?;
            let stderr = collected.stderr.ok_or_else(|| {
                "bash-local: subprocess implementation dropped a requested collect stream"
                    .to_string()
            })?;
            // Only this executor's timeout reason counts as timedOut; outer
            // deadlines count as aborts.
            let timed_out = timeout_of(fused.reason().as_ref(), Some("BASH_TIMEOUT")).is_some();
            let aborted = fused.is_cancelled() && !timed_out;
            Ok(ShellRunResult {
                exit_code: outcome.exit_code,
                signal: outcome.signal,
                timed_out,
                aborted,
                timeout_ms: spec.timeout_ms,
                stdout: final_output(stdout.as_ref()),
                stderr: final_output(stderr.as_ref()),
                sandbox: None,
            })
        })
    }

    /// Start an explicit argv with the background lifecycle (TS
    /// `startArgv`).
    pub fn start_argv(&self, spec: ShellExecSpec, argv: Vec<String>) -> Arc<dyn ShellProcess> {
        // Background runs ignore timeoutMs; callers stop them through kill()
        // or spec.signal.
        let config = self.config();
        let abort_predicate = spec.signal.clone();
        let spawn = Self::spawn_spec(
            &config,
            &spec,
            argv,
            config.max_output_bytes,
            spec.signal.clone(),
        );
        let handle = match self.subprocess.spawn(spawn) {
            Ok(handle) => handle,
            Err(error) => {
                return failed_process(format!("spawn failed: {error}"));
            }
        };

        let state = Arc::new(BashProcessState::new());
        let collected = handle.collected();
        let stdout: Arc<dyn SubprocessOutputReader> = match collected.stdout {
            Some(stdout) => stdout,
            None => {
                return failed_process(
                    "bash-local: subprocess implementation dropped a requested collect stream"
                        .to_string(),
                );
            }
        };
        let stderr: Arc<dyn SubprocessOutputReader> = match collected.stderr {
            Some(stderr) => stderr,
            None => {
                return failed_process(
                    "bash-local: subprocess implementation dropped a requested collect stream"
                        .to_string(),
                );
            }
        };

        let facts = Arc::new(BashProcessFacts {
            state: state.clone(),
        });
        let hook = self.on_process_done.lock().clone();
        let done: Shared<BoxFuture<'static, ()>> = {
            let state = state.clone();
            let subprocess = handle.clone();
            let stderr_for_done = stderr.clone();
            let abort_predicate = abort_predicate.clone();
            async move {
                match subprocess.done().await {
                    Ok(outcome) => {
                        // Any signal termination is killed, including a
                        // command signaling itself.
                        {
                            let mut status = state.status.lock();
                            if *status == ShellProcessStatus::Running {
                                *status = if abort_predicate.as_ref().is_some_and(|abort| abort())
                                    || outcome.signal.is_some()
                                {
                                    ShellProcessStatus::Killed
                                } else {
                                    ShellProcessStatus::Completed
                                };
                            }
                        }
                        *state.exit_code.lock() = outcome.exit_code;
                        *state.signal.lock() = outcome.signal;
                        // The settlement hook receives the retained stderr
                        // tail (a lossy-aware read-from-0 probe that never
                        // consumes the incremental cursor).
                        let stderr_text = collect_tail(stderr_for_done.clone());
                        (hook)(&facts, stderr_text, false, None);
                    }
                    Err(error) => {
                        // Background spawn failures settle as killed and
                        // surface through the read path.
                        *state.status.lock() = ShellProcessStatus::Killed;
                        let note = format!("spawn failed: {error}");
                        *state.spawn_failure_note.lock() = Some(note.clone());
                        (hook)(&facts, note, true, Some(error));
                    }
                }
            }
            .boxed()
            .shared()
        };
        // Drive settlement without a consumer (the seam contract: `done`
        // settles at process close).
        {
            let driven = done.clone();
            tokio::spawn(async move {
                driven.await;
            });
        }

        Arc::new(BashShellProcess {
            state,
            subprocess: handle,
            stdout,
            stderr,
            done,
            abort_predicate,
        })
    }
}

/// The retained stderr tail for the settlement hook, read without consuming
/// the process's incremental cursor (a lossy read-from-0 mirrors the TS
/// settlement probe).
fn collect_tail(reader: Arc<dyn SubprocessOutputReader>) -> String {
    reader.read_from(0).text
}

impl ShellExecutor for LocalBashExecutor {
    fn resolve(&self, request: ShellExecRequest) -> ShellExecSpec {
        let config = self.config();
        let timeout_ms = clamp_timeout(
            request.timeout_ms,
            config.timeout_ms,
            config.max_timeout_ms,
            "bash-local: request.timeoutMs",
        );
        let stdout_max_bytes = request.stdout_max_bytes.unwrap_or(config.max_output_bytes);
        if stdout_max_bytes == 0 {
            panic!("bash-local: request.stdoutMaxBytes must be a positive finite number");
        }
        ShellExecSpec {
            command: request.command,
            workdir: request.workdir.or(config.cwd).unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|cwd| cwd.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| ".".to_string())
            }),
            timeout_ms,
            stdout_max_bytes,
            signal: request.signal,
            stdin: request.stdin,
            env: request.env,
            dsh_env: request.dsh_env,
            sandbox_policy: request.sandbox_policy,
        }
    }

    fn run(&self, spec: ShellExecSpec) -> BoxFuture<'static, Result<ShellRunResult, String>> {
        let argv = vec!["bash".to_string(), "-c".to_string(), spec.command.clone()];
        self.run_argv(spec, argv)
    }

    fn start(&self, spec: ShellExecSpec) -> Arc<dyn ShellProcess> {
        let argv = vec!["bash".to_string(), "-c".to_string(), spec.command.clone()];
        self.start_argv(spec, argv)
    }
}

/// The managed `DSH_*` snapshot an executor hands to spawned children 鈥?the
/// executor carries caller-supplied `dshEnv` verbatim; this helper is the
/// vocabulary anchor for subclasses (kept for the seam's consumers).
pub fn dsh_env_prefix() -> &'static str {
    DSH_ENV_PREFIX
}
