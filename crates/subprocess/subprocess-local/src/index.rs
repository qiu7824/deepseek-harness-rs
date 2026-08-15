//! Local Service Provider for the subprocess capability seam. Each spawn is
//! a detached process tree with the spec's per-stream stdio dispositions.
//! Normal disposal terminates and joins live trees; the disposer's
//! synchronous fallback force-stops any trees the service still owns. It has
//! no config: every disposition and limit arrives on the spec, so the
//! deployment-varying choices stay with the caller's config (the bash
//! executor's, the LSP host's, …). Rust port of
//! `packages/subprocess/subprocess-local/src/index.ts`.
//!
//! # Deviations
//!
//! - `spawnTerminal` (node-pty) is deferred to the PTY milestone and
//!   rejects; the terminal inspector (`process-inspector.ts`) ships with it.
//! - There is no synchronous host-exit phase in Rust; the disposal effect's
//!   synchronous fallback (`terminate_for_host_exit` on every live handle)
//!   is the last-resort equivalent.

use std::path::Path;
use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::Mutex;

use cordis::{Context, make_disposer};
use dsh_subprocess::{
    SubprocessAbort, SubprocessHandle, SubprocessRuntime, SubprocessSpawnSpec,
    SubprocessTerminalHandle, SubprocessTerminalSpawnSpec,
};

use crate::spawn::{LocalHandle, SpawnInternals, child_env, spawn_subprocess};

/// Local subprocess service: detached process trees, per-stream stdio
/// dispositions (raw pipes, inherit, bounded tail-keep collection with spill
/// files), credential-scrubbed environment, and tree-scoped signalling with
/// SIGTERM→grace→SIGKILL escalation (TS `LocalSubprocessRuntime`).
pub struct LocalSubprocessRuntime {
    /// Live handles retained for normal disposal and finalization.
    live: Arc<Mutex<Vec<Arc<LocalHandle>>>>,
    /// Test hook: spill and platform knobs forwarded to `spawn_subprocess`.
    internals: Mutex<SpawnInternals>,
}

impl LocalSubprocessRuntime {
    /// Construct an unregistered runtime (test hook); `install` registers one
    /// as `ctx.subprocess`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            live: Arc::new(Mutex::new(Vec::new())),
            internals: Mutex::new(SpawnInternals::default()),
        })
    }

    /// Construct, register as `ctx.subprocess`, and attach the teardown
    /// effect (the TS constructor + `super(ctx)` collapse).
    pub fn install(ctx: &Context) -> Arc<Self> {
        let runtime = Self::new();
        let teardown = runtime.clone();
        let _ = ctx.effect(
            "local subprocess teardown",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let runtime = teardown.clone();
                    Box::pin(async move {
                        let _ = runtime.dispose_managed_processes().await;
                    })
                }))
            }),
        );
        let erased: Arc<dyn SubprocessRuntime> = runtime.clone();
        ctx.register_service(erased);
        runtime
    }

    /// Set the spawn knobs (spill dir, platform, taskkill, group probe) —
    /// the TS public `internals` test hook.
    pub fn set_internals(&self, internals: SpawnInternals) {
        *self.internals.lock() = internals;
    }

    /// Synchronous final termination of every live tree without starting
    /// timers or waits — the last fallback after failed normal disposal.
    fn terminate_for_host_exit(&self) {
        for handle in self.live.lock().iter() {
            handle.terminate_for_host_exit();
        }
    }

    /// Terminate (escalating), then await WHOLE-TREE exit — not just the
    /// direct child's settlement — so even a TERM-trapping descendant cannot
    /// outlive the fiber.
    async fn dispose_managed_processes(&self) -> Result<(), String> {
        let handles: Vec<Arc<LocalHandle>> = self.live.lock().iter().cloned().collect();
        let pending = handles.iter().map(|handle| {
            handle.terminate();
            let handle = handle.clone();
            async move {
                // Spawn-failure rejections already settled and left the live
                // set.
                handle.done().await?;
                let _ = handle.wait_for_exit(None).await;
                Ok::<(), String>(())
            }
        });
        let results = futures::future::join_all(pending).await;
        let failures: Vec<String> = results.into_iter().filter_map(Result::err).collect();
        if !failures.is_empty() {
            self.terminate_for_host_exit();
        }
        self.live.lock().clear();
        match failures.len() {
            0 => Ok(()),
            1 => Err(failures[0].clone()),
            _ => Err(format!(
                "local subprocess teardown failed: {}",
                failures.join("; ")
            )),
        }
    }

    /// Windows environment keys use case-insensitive semantics (TS
    /// `environmentValue`).
    fn environment_value<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
        if let Some((_, value)) = env.iter().find(|(key, _)| key == name) {
            return Some(value);
        }
        #[cfg(windows)]
        {
            let normalized = name.to_uppercase();
            if let Some((_, value)) = env.iter().find(|(key, _)| key.to_uppercase() == normalized) {
                return Some(value);
            }
        }
        None
    }

    /// PATH candidates for a bare command name, honoring PATHEXT on Windows
    /// (TS `executableCandidates`).
    fn executable_candidates(command: &str, env: &[(String, String)]) -> Vec<String> {
        let path = Self::environment_value(env, "PATH").unwrap_or("");
        let extensions: Vec<String> = if cfg!(windows) && Path::new(command).extension().is_none() {
            Self::environment_value(env, "PATHEXT")
                .unwrap_or(".COM;.EXE;.BAT;.CMD")
                .split(';')
                .map(str::to_string)
                .collect()
        } else {
            vec![String::new()]
        };
        std::env::split_paths(path)
            .flat_map(|directory| {
                extensions
                    .iter()
                    .map(move |extension| directory.join(format!("{command}{extension}")).to_string_lossy().into_owned())
            })
            .collect()
    }

    /// A stable abort message for the `signal` predicate (TS
    /// `signal.throwIfAborted`).
    fn aborted_error() -> String {
        "subprocess-local: aborted".to_string()
    }
}

impl SubprocessRuntime for LocalSubprocessRuntime {
    fn resolve_executable(
        &self,
        command: &str,
        env: Option<&[(String, String)]>,
        signal: Option<SubprocessAbort>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let command = command.to_string();
        let env: Option<Vec<(String, String)>> = env.map(|entries| entries.to_vec());
        Box::pin(async move {
            if command.is_empty() {
                return Err("subprocess-local: executable must be non-empty".to_string());
            }
            if signal.as_ref().is_some_and(|signal| signal()) {
                return Err(Self::aborted_error());
            }
            // Explicit resolve environments carry no tombstones (the TS
            // `Record<string, string>` shape).
            let tombstones: Vec<(String, Option<String>)> = env
                .as_deref()
                .map(|entries| {
                    entries
                        .iter()
                        .map(|(key, value)| (key.clone(), Some(value.clone())))
                        .collect()
                })
                .unwrap_or_default();
            let environment = child_env(Some(&tombstones));
            let absolute = Path::new(&command).is_absolute();
            if !absolute && (command.contains('/') || command.contains('\\')) {
                return Err(format!(
                    "subprocess-local: command {command:?} is a relative path; use an absolute path or a bare PATH name"
                ));
            }
            let candidates = if absolute {
                vec![command.clone()]
            } else {
                Self::executable_candidates(&command, &environment)
            };
            for candidate in &candidates {
                if signal.as_ref().is_some_and(|signal| signal()) {
                    return Err(Self::aborted_error());
                }
                let Ok(metadata) = tokio::fs::metadata(candidate).await else {
                    continue;
                };
                if !metadata.is_file() {
                    continue;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o111 != 0 {
                        if signal.as_ref().is_some_and(|signal| signal()) {
                            return Err(Self::aborted_error());
                        }
                        return Ok(candidate.clone());
                    }
                    continue;
                }
                #[cfg(not(unix))]
                {
                    if signal.as_ref().is_some_and(|signal| signal()) {
                        return Err(Self::aborted_error());
                    }
                    return Ok(candidate.clone());
                }
            }
            if signal.as_ref().is_some_and(|signal| signal()) {
                return Err(Self::aborted_error());
            }
            Err(if absolute {
                format!("subprocess-local: command {command:?} is not an executable file")
            } else {
                format!("subprocess-local: command {command:?} was not found on PATH")
            })
        })
    }

    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
        let internals = self.internals.lock().clone();
        let handle = Arc::new(spawn_subprocess(spec, internals)?);
        self.live.lock().push(handle.clone());
        // Release ownership only once the whole TREE is gone, not at
        // direct-child settlement — a TERM-trapping helper that outlives the
        // leader must stay owned so teardown can still escalate it. For the
        // common no-survivor case waitForExit resolves immediately after
        // settlement.
        let live = self.live.clone();
        let owned = handle.clone();
        tokio::spawn(async move {
            let _ = owned.done().await;
            let _ = owned.wait_for_exit(None).await;
            live.lock().retain(|candidate| !Arc::ptr_eq(candidate, &owned));
        });
        Ok(handle)
    }

    fn spawn_terminal(
        &self,
        _spec: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        Box::pin(async {
            Err(
                "subprocess-local: terminal allocation is not yet implemented (deferred to the PTY milestone)"
                    .to_string(),
            )
        })
    }
}
