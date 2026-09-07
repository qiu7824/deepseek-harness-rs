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
//! - Native PTYs use `portable-pty` (ConPTY on Windows) rather than node-pty.
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

use crate::portable_terminal::PortableTerminalHandle;
use crate::spawn::{LocalHandle, SpawnInternals, child_env, spawn_subprocess};

/// Local subprocess service: detached process trees, per-stream stdio
/// dispositions (raw pipes, inherit, bounded tail-keep collection with spill
/// files), credential-scrubbed environment, and tree-scoped signalling with
/// SIGTERM→grace→SIGKILL escalation (TS `LocalSubprocessRuntime`).
pub struct LocalSubprocessRuntime {
    /// Serializes close-vs-spawn admission. Spawns hold this gate through
    /// native creation and owner-list publication; teardown closes it before
    /// snapshotting either list.
    closing: Arc<Mutex<bool>>,
    /// Live handles retained for normal disposal and finalization.
    live: Arc<Mutex<Vec<Arc<LocalHandle>>>>,
    /// Live PTYs retained until their full native session cleanup settles.
    terminals: Arc<Mutex<Vec<Arc<PortableTerminalHandle>>>>,
    /// Test hook: spill and platform knobs forwarded to `spawn_subprocess`.
    internals: Mutex<SpawnInternals>,
    resources: Mutex<Option<ResourceProvider>>,
    path_prefixes: Mutex<Vec<std::path::PathBuf>>,
}

pub type ResourceProvider = Arc<
    dyn Fn(&str, &[(String, String)]) -> Result<Option<Arc<dsh_workspace_resources::Store>>, String>
        + Send
        + Sync,
>;

impl Drop for LocalSubprocessRuntime {
    fn drop(&mut self) {
        // A caller may drop the runtime without driving its async Cordis
        // disposer (test panic, process-exit fallback, cancelled owner). The
        // observer tasks retain handles, so handle Drop alone is not an
        // ownership boundary. Synchronously close every native process/PTY
        // here; normal async disposal remains the graceful path.
        self.terminate_for_host_exit();
    }
}

impl LocalSubprocessRuntime {
    /// Construct an unregistered runtime (test hook); `install` registers one
    /// as `ctx.subprocess`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            closing: Arc::new(Mutex::new(false)),
            live: Arc::new(Mutex::new(Vec::new())),
            terminals: Arc::new(Mutex::new(Vec::new())),
            internals: Mutex::new(SpawnInternals::default()),
            resources: Mutex::new(None),
            path_prefixes: Mutex::new(Vec::new()),
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

    pub fn set_resource_provider(&self, provider: ResourceProvider) {
        *self.resources.lock() = Some(provider);
    }
    pub fn set_path_prefixes(&self, paths: Vec<std::path::PathBuf>) {
        *self.path_prefixes.lock() = paths;
    }
    fn execution_path(&self) -> Option<String> {
        let prefixes = self.path_prefixes.lock();
        if prefixes.is_empty() {
            return None;
        }
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(
            prefixes
                .iter()
                .cloned()
                .chain(std::env::split_paths(&inherited)),
        )
        .ok()
        .map(|value| value.to_string_lossy().into_owned())
    }

    /// Synchronous final termination of every live tree without starting
    /// timers or waits — the last fallback after failed normal disposal.
    fn terminate_for_host_exit(&self) {
        for handle in self.live.lock().iter() {
            handle.terminate_for_host_exit();
        }
        for terminal in self.terminals.lock().iter() {
            terminal.terminate_for_host_exit();
        }
    }

    /// Terminate (escalating), then await WHOLE-TREE exit — not just the
    /// direct child's settlement — so even a TERM-trapping descendant cannot
    /// outlive the fiber.
    async fn dispose_managed_processes(&self) -> Result<(), String> {
        let (handles, terminals) = {
            let mut closing = self.closing.lock();
            *closing = true;
            let handles = self.live.lock().iter().cloned().collect::<Vec<_>>();
            let terminals = self.terminals.lock().iter().cloned().collect::<Vec<_>>();
            (handles, terminals)
        };
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
        let terminal_results =
            futures::future::join_all(terminals.iter().map(|terminal| terminal.terminate())).await;
        let failures: Vec<String> = results
            .into_iter()
            .chain(terminal_results)
            .filter_map(Result::err)
            .collect();
        if !failures.is_empty() {
            self.terminate_for_host_exit();
        }
        self.live.lock().clear();
        self.terminals.lock().clear();
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
                extensions.iter().map(move |extension| {
                    directory
                        .join(format!("{command}{extension}"))
                        .to_string_lossy()
                        .into_owned()
                })
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

    fn spawn(&self, mut spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
        if !spec
            .env
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        {
            if let Some(path) = self.execution_path() {
                spec.env
                    .get_or_insert_default()
                    .push(("PATH".into(), Some(path)));
            }
        }
        let closing = self.closing.lock();
        if *closing {
            return Err("subprocess-local: runtime is closing".to_string());
        }
        let mut internals = self.internals.lock().clone();
        let mut resources = self
            .resources
            .lock()
            .clone()
            .map(|provider| {
                let env = spec
                    .env
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_ref().map(|value| (key.clone(), value.clone()))
                    })
                    .collect::<Vec<_>>();
                provider(&spec.cwd, &env)
            })
            .transpose()?
            .flatten()
            .map(|store| {
                store.prepare_execution(
                    &spec.cwd,
                    &spec.argv,
                    spec.env.as_deref().unwrap_or_default(),
                )
            })
            .transpose()?;
        if let Some(resources) = resources.as_ref() {
            spec.env.get_or_insert_default().extend(
                resources
                    .environment
                    .iter()
                    .cloned()
                    .map(|(name, value)| (name, Some(value))),
            );
            internals.spill_dir = Some(resources.spill_directory());
        }
        let handle = Arc::new(spawn_subprocess(spec, internals)?);
        let resource_error = resources
            .as_mut()
            .and_then(|resources| resources.attach_process(handle.pid() as u32).err());
        if resource_error.is_some() {
            handle.terminate();
        }
        self.live.lock().push(handle.clone());
        drop(closing);
        // Release ownership only once the whole TREE is gone, not at
        // direct-child settlement — a TERM-trapping helper that outlives the
        // leader must stay owned so teardown can still escalate it. For the
        // common no-survivor case waitForExit resolves immediately after
        // settlement.
        let live = self.live.clone();
        let owned = handle.clone();
        tokio::spawn(async move {
            let outcome = owned.done().await;
            let exited = owned.wait_for_exit(None).await;
            if let Some(resources) = resources.as_mut() {
                if exited {
                    resources.finish(outcome.is_ok_and(|outcome| {
                        outcome.exit_code == Some(0) && outcome.signal.is_none()
                    }));
                } else {
                    resources.protect();
                }
            }
            live.lock()
                .retain(|candidate| !Arc::ptr_eq(candidate, &owned));
        });
        if let Some(error) = resource_error {
            return Err(format!(
                "cannot register process resource ownership: {error}"
            ));
        }
        Ok(handle)
    }

    fn spawn_terminal(
        &self,
        mut spec: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        let closing = self.closing.clone();
        let terminals = self.terminals.clone();
        let resource_provider = self.resources.lock().clone();
        if !spec
            .env
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        {
            if let Some(path) = self.execution_path() {
                spec.env.get_or_insert_default().push(("PATH".into(), path));
            }
        }
        Box::pin(async move {
            let signal = spec.signal.clone();
            if signal.as_ref().is_some_and(|signal| signal()) {
                return Err("subprocess-local: terminal allocation aborted".to_string());
            }
            let extra = spec
                .env
                .as_deref()
                .unwrap_or_default()
                .iter()
                .cloned()
                .map(|(name, value)| (name, Some(value)))
                .collect::<Vec<_>>();
            let mut resources = resource_provider
                .map(|provider| provider(&spec.cwd, spec.env.as_deref().unwrap_or_default()))
                .transpose()?
                .flatten()
                .map(|store| store.prepare_execution(&spec.cwd, &spec.argv, &extra))
                .transpose()?;
            if let Some(resources) = resources.as_ref() {
                spec.env
                    .get_or_insert_default()
                    .extend(resources.environment.clone());
            }
            let terminal = {
                let closing = closing.lock();
                if *closing {
                    return Err("subprocess-local: runtime is closing".to_string());
                }
                if signal.as_ref().is_some_and(|signal| signal()) {
                    return Err("subprocess-local: terminal allocation aborted".to_string());
                }
                let terminal = PortableTerminalHandle::spawn(spec)?;
                terminals.lock().push(terminal.clone());
                drop(closing);
                terminal
            };
            let owned = terminal.clone();
            let live = terminals.clone();
            let resource_error = resources
                .as_mut()
                .and_then(|resources| resources.attach_process(terminal.pid()).err());
            tokio::spawn(async move {
                let outcome = owned.done().await;
                if owned.terminate().await.is_ok() {
                    if let Some(resources) = resources.as_mut() {
                        resources.finish(outcome.is_ok_and(|outcome| {
                            outcome.exit_code == Some(0) && outcome.signal.is_none()
                        }));
                    }
                    live.lock()
                        .retain(|candidate| !Arc::ptr_eq(candidate, &owned));
                } else if let Some(resources) = resources.as_mut() {
                    resources.protect();
                }
            });
            if signal.as_ref().is_some_and(|signal| signal()) {
                terminal.terminate().await?;
                return Err("subprocess-local: terminal allocation aborted".to_string());
            }
            if let Some(error) = resource_error {
                terminal.terminate().await?;
                return Err(format!(
                    "cannot register terminal resource ownership: {error}"
                ));
            }
            Ok(terminal as Arc<dyn SubprocessTerminalHandle>)
        })
    }
}
