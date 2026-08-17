//! E2B subprocess runtime: the seam service registered as
//! `ctx.subprocess`, its executable resolution, and its spawn/disposal
//! policy. Rust port of `index.ts` (the terminal half arrives with the
//! terminal milestone).
//!
//! # Deviations
//!
//! - `spawnTerminal` is absent until the terminal milestone (documented
//!   in the crate root).
//! - The disposal effect collapses into a teardown method the owner
//!   fiber calls (`dispose`); cordis effect wiring stays with the caller.

use std::sync::Arc;

use cordis::Context;
use dsh_e2b::{
    E2bCommandOptions, E2bRuntime, E2bSandbox, quote_e2b_shell_arg,
};
use dsh_subprocess::{
    SubprocessAbort, SubprocessHandle, SubprocessRuntime, SubprocessSpawnSpec,
    SubprocessTerminalHandle, SubprocessTerminalSpawnSpec,
};
use dsh_timeout::MAX_TIMER_DELAY_MS;
use futures::FutureExt;
use futures::future::BoxFuture;
use parking_lot::Mutex;

use crate::process::E2bSubprocessHandle;

/// Configuration for the E2B subprocess adapter (TS `Config`).
#[derive(Debug, Clone)]
pub struct Config {
    /// Remote status/liveness poll cadence in milliseconds; each tick is
    /// one control-plane request.
    pub poll_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self { poll_ms: 20 }
    }
}

/// E2B command manager registered as `ctx.subprocess` (TS
/// `E2BSubprocessRuntime`).
pub struct E2bSubprocessRuntime {
    pub ctx: Context,
    pub config: Config,
    e2b: Arc<E2bRuntime>,
    live: Mutex<Vec<Arc<dyn SubprocessHandle>>>,
    disposing: Mutex<bool>,
}

/// Enforce the seam's documented grace bound (TS
/// `requireRepresentableGrace`).
pub fn require_representable_grace(grace_ms: u64) -> Result<(), String> {
    if grace_ms == 0 || grace_ms > MAX_TIMER_DELAY_MS {
        return Err(format!(
            "subprocess graceMs must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
        ));
    }
    Ok(())
}

impl E2bSubprocessRuntime {
    /// Create the service and register it as `ctx.subprocess` (TS
    /// constructor + `super(ctx)`).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        if config.poll_ms == 0 {
            return Err("subprocess-e2b: pollMs must be a positive safe integer".to_string());
        }
        let e2b = ctx
            .get_typed::<Arc<E2bRuntime>>("e2b", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "subprocess-e2b: the e2b service is not composed".to_string())?;
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            config,
            e2b,
            live: Mutex::new(Vec::new()),
            disposing: Mutex::new(false),
        });
        ctx.register_service(service.clone());
        Ok(service)
    }

    /// The shared sandbox owner (test seam).
    pub fn e2b(&self) -> Arc<E2bRuntime> {
        self.e2b.clone()
    }

    /// Run the disposal policy: terminate every live handle and await its
    /// settlement (TS constructor's effect body).
    pub async fn dispose(&self) -> Result<(), String> {
        *self.disposing.lock() = true;
        let handles = self.live.lock().clone();
        let mut failures = Vec::new();
        for handle in &handles {
            handle.terminate();
            if let Err(error) = handle.done().await {
                failures.push(error);
            }
        }
        self.live.lock().clear();
        if failures.len() == 1 {
            return Err(failures.remove(0));
        }
        if failures.len() > 1 {
            return Err(format!(
                "subprocess-e2b: teardown failed: {}",
                failures.join("; ")
            ));
        }
        Ok(())
    }
}

impl cordis::Service for E2bSubprocessRuntime {
    fn service_name(&self) -> &'static str {
        "subprocess"
    }
}

#[async_trait::async_trait]
impl SubprocessRuntime for E2bSubprocessRuntime {
    /// Resolve a bare PATH name or validate an absolute program inside the
    /// remote sandbox (TS `resolveExecutable`).
    fn resolve_executable(
        &self,
        command: &str,
        env: Option<&[(String, String)]>,
        signal: Option<SubprocessAbort>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let e2b = self.e2b.clone();
        let command = command.to_string();
        let env: Vec<(String, String)> = env.unwrap_or_default().to_vec();
        async move {
            if command.is_empty() {
                return Err("subprocess-e2b: executable name must be non-empty".to_string());
            }
            if signal.as_ref().is_some_and(|signal| signal()) {
                return Err("aborted".to_string());
            }
            let sandbox = e2b
                .get_sandbox()
                .await
                .map_err(|error| format!("subprocess-e2b: {error}"))?;
            if command.starts_with('/') {
                sandbox
                    .run(
                        &format!(
                            "test -f {} -a -x {}",
                            quote_e2b_shell_arg(&command),
                            quote_e2b_shell_arg(&command)
                        ),
                        &E2bCommandOptions {
                            signal: signal.clone(),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|error| format!("subprocess-e2b: {error}"))?;
                return Ok(command);
            }
            if command.contains('/') {
                return Err(format!(
                    "subprocess-e2b: command {command:?} is a relative path; use an absolute path or a bare PATH name"
                ));
            }
            let path = env
                .iter()
                .find(|(name, _value)| name == "PATH")
                .map(|(_name, value)| value.clone());
            let prefix = match &path {
                Some(path) => format!("PATH={} ", quote_e2b_shell_arg(path)),
                None => String::new(),
            };
            let result = sandbox
                .run(
                    &format!("{prefix}command -v -- {}", quote_e2b_shell_arg(&command)),
                    &E2bCommandOptions {
                        cwd: Some(e2b.cwd().to_string()),
                        signal: signal.clone(),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| format!("subprocess-e2b: {error}"))?;
            if signal.as_ref().is_some_and(|signal| signal()) {
                return Err("aborted".to_string());
            }
            let executable = result.stdout.trim().to_string();
            if executable.contains('\n')
                || (!executable.starts_with('/') && !executable.contains('/'))
            {
                return Err(format!(
                    "subprocess-e2b: executable {command:?} did not resolve to one absolute path"
                ));
            }
            // A relative result comes from a relative PATH entry; the
            // lookup ran with the shared cwd.
            if executable.starts_with('/') {
                Ok(executable)
            } else {
                Ok(format!(
                    "{}/{}",
                    e2b.cwd().trim_end_matches('/'),
                    executable
                ))
            }
        }
        .boxed()
    }

    /// Start one E2B-backed process tree (TS `spawn`).
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
        if *self.disposing.lock() {
            return Err("subprocess-e2b: service is disposing".to_string());
        }
        let program = spec
            .argv
            .first()
            .ok_or_else(|| "invalid argv: expected a non-empty program name at argv[0]".to_string())?;
        if program.is_empty() {
            return Err("invalid argv: expected a non-empty program name at argv[0]".to_string());
        }
        require_representable_grace(spec.grace_ms)?;
        if spec.signal.as_ref().is_some_and(|signal| signal()) {
            return Err("aborted before spawn".to_string());
        }
        let state_dir = format!(
            "{}/processes/{}",
            self.e2b.runtime_root(),
            uuid::Uuid::new_v4()
        );
        let handle: Arc<dyn SubprocessHandle> = Arc::new(E2bSubprocessHandle::new(
            self.e2b.clone(),
            spec,
            state_dir,
            self.config.poll_ms,
        ));
        self.live.lock().push(handle.clone());
        Ok(handle)
    }

    /// Terminal allocation arrives with the terminal milestone (TS
    /// `spawnTerminal`; a fail-loud refusal until then).
    fn spawn_terminal(
        &self,
        _spec: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        async move {
            Err(
                "subprocess-e2b: terminal allocation is not implemented (terminal milestone)"
                    .to_string(),
            )
        }
        .boxed()
    }
}

/// One E2B-backed process tree handle's automatic live-set release
/// (TS `spawn`'s `release` helper; wired by callers that hold the runtime).
pub async fn release_on_exit(
    runtime: &Arc<E2bSubprocessRuntime>,
    handle: Arc<dyn SubprocessHandle>,
) {
    let _ = handle.done().await;
    runtime.live.lock().retain(|live| !Arc::ptr_eq(live, &handle));
}

/// The shared sandbox handle (test seam; the runtime owns one).
pub async fn sandbox_for(runtime: &E2bSubprocessRuntime) -> Result<Arc<dyn E2bSandbox>, String> {
    runtime
        .e2b
        .get_sandbox()
        .await
        .map_err(|error| error.to_string())
}
