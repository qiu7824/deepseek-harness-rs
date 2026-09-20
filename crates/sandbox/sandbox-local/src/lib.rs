use std::sync::Arc;

#[cfg(windows)]
mod installed_runtime;

#[cfg(windows)]
static EMBEDDED_RUNNER: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Register the current host only when its entry point implements the sandbox subcommand.
/// Executable relocation and renaming do not change this capability.
#[cfg(windows)]
pub fn register_embedded_windows_runner() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    if let Some(registered) = EMBEDDED_RUNNER.get() {
        return if registered == &executable {
            Ok(())
        } else {
            Err("embedded sandbox runner was already registered for a different executable".into())
        };
    }
    EMBEDDED_RUNNER
        .set(executable)
        .map_err(|_| "embedded sandbox runner registration raced".to_string())
}

fn embedded_runner_path() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        EMBEDDED_RUNNER.get().cloned()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

use cordis::Context;
use dsh_sandbox::{
    ConfinedArgv, ConfinedSandboxMode, RunnerFailureRule, SandboxEnforcement,
    SandboxExecutionPolicy, SandboxMode, SandboxPolicy, SandboxProvider, SandboxUnavailableError,
    writable_roots,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxCapability {
    Full,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SandboxLaunch {
    Direct(Vec<String>),
    Confined(ConfinedArgv),
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub platform: Option<String>,
}

pub struct LocalSandboxProvider {
    platform: String,
    runtime_roots: Vec<std::path::PathBuf>,
    runtime_cache: Option<std::path::PathBuf>,
    #[cfg(windows)]
    preparation: Arc<std::sync::Mutex<RuntimePreparation>>,
    #[cfg(windows)]
    preparation_gate: Arc<tokio::sync::Semaphore>,
}

#[cfg(windows)]
#[derive(Default)]
struct RuntimePreparation {
    generation: u64,
    active: std::collections::HashMap<
        String,
        (
            u64,
            futures::future::Shared<futures::future::BoxFuture<'static, Result<(), String>>>,
        ),
    >,
}

#[cfg(windows)]
impl RuntimePreparation {
    fn evict_failed(&mut self, key: &str) {
        if self
            .active
            .get(key)
            .is_some_and(|(_, flight)| matches!(flight.peek(), Some(Err(_))))
        {
            self.active.remove(key);
        }
    }
}

#[cfg(all(test, windows))]
mod preparation_tests {
    use super::*;
    use futures::FutureExt;

    #[tokio::test]
    async fn failed_shared_preparation_is_retryable_without_evicting_live_or_successful_work() {
        let mut state = RuntimePreparation::default();
        let failed = async { Err::<(), String>("ACL preparation failed".into()) }
            .boxed()
            .shared();
        let success = async { Ok::<(), String>(()) }.boxed().shared();
        let pending = futures::future::pending::<Result<(), String>>()
            .boxed()
            .shared();
        state.active.insert("failed".into(), (1, failed.clone()));
        state.active.insert("success".into(), (2, success.clone()));
        state.active.insert("pending".into(), (3, pending));
        assert!(failed.await.is_err());
        success.await.unwrap();
        for key in ["failed", "success", "pending"] {
            state.evict_failed(key);
        }
        assert!(!state.active.contains_key("failed"));
        assert!(state.active.contains_key("success"));
        assert!(state.active.contains_key("pending"));
    }
}

impl LocalSandboxProvider {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self {
            platform: config.platform.unwrap_or_else(host_platform),
            runtime_roots: Vec::new(),
            runtime_cache: None,
            #[cfg(windows)]
            preparation: Default::default(),
            #[cfg(windows)]
            preparation_gate: Arc::new(tokio::sync::Semaphore::new(2)),
        })
    }

    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let provider = Self::new(config);
        let erased: Arc<dyn SandboxProvider> = provider.clone();
        ctx.register_service(erased);
        provider
    }

    pub fn install_with_runtimes(
        ctx: &Context,
        config: Config,
        roots: Vec<std::path::PathBuf>,
        cache: std::path::PathBuf,
    ) -> Arc<Self> {
        let provider = Arc::new(Self {
            platform: config.platform.unwrap_or_else(host_platform),
            runtime_roots: roots,
            runtime_cache: Some(cache),
            #[cfg(windows)]
            preparation: Default::default(),
            #[cfg(windows)]
            preparation_gate: Arc::new(tokio::sync::Semaphore::new(2)),
        });
        let erased: Arc<dyn SandboxProvider> = provider.clone();
        ctx.register_service(erased);
        provider
    }

    pub fn capability(&self) -> SandboxCapability {
        match self.platform.as_str() {
            "linux" | "darwin" => SandboxCapability::Full,
            "win32" if embedded_runner_path().is_some() => SandboxCapability::Full,
            _ => SandboxCapability::Unavailable,
        }
    }

    pub fn wrap_execution(
        &self,
        argv: &[String],
        policy: &SandboxExecutionPolicy,
    ) -> Result<SandboxLaunch, SandboxUnavailableError> {
        let Some(mode) = confined_mode(policy.mode) else {
            return Ok(SandboxLaunch::Direct(argv.to_vec()));
        };
        self.confine(
            argv,
            &SandboxPolicy {
                read_only_roots: policy.read_only_roots.clone(),
                mode,
                workspace_root: policy.workspace_root.clone(),
                session_id: policy.session_id.clone(),
            },
        )
        .map(SandboxLaunch::Confined)
    }
}

impl SandboxProvider for LocalSandboxProvider {
    fn confine_with_startup(
        &self,
        argv: &[String],
        policy: &SandboxPolicy,
    ) -> Result<ConfinedArgv, SandboxUnavailableError> {
        let mut confined = self.confine(argv, policy)?;
        #[cfg(windows)]
        if self.platform == "win32"
            && embedded_runner_path().is_some_and(|runner| {
                confined
                    .argv
                    .first()
                    .is_some_and(|program| std::path::Path::new(program) == runner)
            })
        {
            let (name, startup) = windows_startup_signal()
                .map_err(|error| SandboxUnavailableError::new(policy.mode, Some(&error)))?;
            let separator = confined.argv.len() - argv.len() - 1;
            confined
                .argv
                .splice(separator..separator, ["--ready-event".into(), name]);
            confined.startup = Some(startup);
        }
        Ok(confined)
    }
    fn prepare(
        &self,
        policy: &SandboxExecutionPolicy,
    ) -> futures::future::BoxFuture<'static, Result<(), String>> {
        #[cfg(windows)]
        if self.platform == "win32"
            && policy.mode != SandboxMode::DangerFullAccess
            && self.runtime_cache.is_some()
        {
            use futures::FutureExt;
            let roots = self.runtime_roots.clone();
            let cache = self.runtime_cache.clone();
            let workspace = std::path::PathBuf::from(&policy.workspace_root);
            let writable = policy.mode == SandboxMode::WorkspaceWrite;
            let preparation_key =
                format!("{}:{writable}", policy.workspace_root.to_ascii_lowercase());
            let gate = self.preparation_gate.clone();
            let candidate: futures::future::BoxFuture<'static, Result<(), String>> =
                Box::pin(async move {
                    let permit = gate
                        .acquire_owned()
                        .await
                        .map_err(|_| "[SANDBOX_SETUP_FAILED] permission worker closed")?;
                    let cache = cache.ok_or(
                        "[SANDBOX_SETUP_FAILED] runtime permission state is not configured",
                    )?;
                    // A cold installed Python tree can take longer than the PTY
                    // prompt budget. Complete its cached read-only preparation once
                    // outside that budget; cancellation never launches user code.
                    let preparation = tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        embedded_windows_runner::windows_runner::prepare_workspace_permissions(
                            &workspace, &cache, writable,
                        )?;
                        embedded_windows_runner::windows_runner::prepare_runtime_permissions(
                            &roots, &cache,
                        )
                    });
                    match preparation.await {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(error)) => Err(format!("[SANDBOX_SETUP_FAILED] {error}")),
                        Err(error) => Err(format!(
                            "[SANDBOX_SETUP_FAILED] runtime preparation task failed: {error}"
                        )),
                    }
                });
            let state = self.preparation.clone();
            let (generation, preparation) = {
                let mut state = state.lock().unwrap();
                // A shared attempt that already settled with an error must not
                // be served again: the only caller that could have evicted it
                // was the one that timed out (120 s) before the failure landed,
                // so the poisoned entry would otherwise fail every later prepare
                // for this workspace until restart.
                state.evict_failed(&preparation_key);
                if let Some(active) = state.active.get(&preparation_key) {
                    active.clone()
                } else {
                    if state.active.len() >= 64 {
                        state
                            .active
                            .retain(|_, (_, flight)| flight.peek().is_none());
                    }
                    if state.active.len() >= 64 {
                        return Box::pin(async {
                            Err("[SANDBOX_SETUP_FAILED] workspace preparation limit reached".into())
                        });
                    }
                    state.generation = state.generation.wrapping_add(1);
                    let active = (state.generation, candidate.shared());
                    state.active.insert(preparation_key.clone(), active.clone());
                    active
                }
            };
            return Box::pin(async move {
                // Retaining one shared attempt prevents cancelled or concurrent
                // callers from creating an unbounded set of blocking ACL workers.
                // A timed-out spawn_blocking job keeps running. Retain its shared
                // join handle so retries cannot start duplicate ACL walks.
                let result = match tokio::time::timeout(std::time::Duration::from_secs(120), preparation).await {
                    Ok(result) => result,
                    Err(_) => return Err("[SANDBOX_SETUP_TIMEOUT] phase=permission_preparation; the shared workspace/runtime ACL preparation is still tracked; command not started. Subsequent checks join this preparation rather than restarting it.".into()),
                };
                if result.is_err() {
                    let mut state = state.lock().unwrap();
                    if state
                        .active
                        .get(&preparation_key)
                        .is_some_and(|active| active.0 == generation)
                    {
                        state.active.remove(&preparation_key);
                    }
                }
                result
            });
        }
        let _ = policy;
        Box::pin(async { Ok(()) })
    }

    fn confine(
        &self,
        argv: &[String],
        policy: &SandboxPolicy,
    ) -> Result<ConfinedArgv, SandboxUnavailableError> {
        let (mut wrapped, denial_signatures, runner_failure_rules) = match self.platform.as_str() {
            "linux" => (
                bwrap_profile_args(policy),
                vec!["read-only file system".to_string()],
                vec![RunnerFailureRule {
                    allowed_exit_codes: None,
                    fatal_signatures: vec!["bwrap: ".to_string()],
                    informational_lines: None,
                }],
            ),
            "darwin" => (
                seatbelt_profile_args(policy),
                vec!["operation not permitted".to_string()],
                vec![RunnerFailureRule {
                    allowed_exit_codes: None,
                    fatal_signatures: vec!["sandbox-exec: ".to_string()],
                    informational_lines: None,
                }],
            ),
            "win32" => (
                windows_profile_args(policy)?,
                vec![
                    "access is denied".to_string(),
                    "permission denied".to_string(),
                    "unauthorizedaccessexception".to_string(),
                ],
                vec![RunnerFailureRule {
                    allowed_exit_codes: None,
                    fatal_signatures: vec!["dsh-sandbox-windows:".to_string()],
                    informational_lines: None,
                }],
            ),
            _ => return Err(SandboxUnavailableError::new(policy.mode, None)),
        };
        if self.platform == "win32" {
            if let Some(cache) = &self.runtime_cache {
                wrapped.extend([
                    "--runtime-cache".into(),
                    cache.to_string_lossy().into_owned(),
                ]);
                for root in &self.runtime_roots {
                    wrapped.extend(["--runtime-root".into(), root.to_string_lossy().into_owned()]);
                }
            }
        }
        #[cfg(windows)]
        if self.platform == "win32"
            && embedded_runner_path().is_some_and(|runner| {
                wrapped
                    .first()
                    .is_some_and(|program| std::path::Path::new(program) == runner)
            })
        {
            if let Some(cache) = self.runtime_cache.as_ref().and_then(|cache| cache.parent()) {
                wrapped.extend([
                    "--cleanup-state".into(),
                    cache.join("sandbox-cleanup").to_string_lossy().into_owned(),
                ]);
            }
        }
        #[cfg(windows)]
        if self.platform == "win32" {
            if let Some(root) = argv
                .first()
                .and_then(|program| installed_runtime::powershell_root(program))
            {
                if !self
                    .runtime_roots
                    .iter()
                    .any(|existing| std::fs::canonicalize(existing).ok().as_ref() == Some(&root))
                {
                    wrapped.extend([
                        if self.runtime_cache.is_some() {
                            "--runtime-root"
                        } else {
                            "--read-root"
                        }
                        .into(),
                        root.to_string_lossy().into_owned(),
                    ]);
                }
            }
        }
        wrapped.push("--".to_string());
        wrapped.extend_from_slice(argv);
        Ok(ConfinedArgv {
            argv: wrapped,
            enforcement: SandboxEnforcement::Full,
            denial_signatures,
            runner_failure_rules,
            startup: None,
        })
    }
}

#[cfg(windows)]
fn windows_startup_signal() -> Result<(String, dsh_sandbox::SandboxStartup), String> {
    use windows_sys::Win32::{Foundation::*, System::Threading::*};
    struct Event(HANDLE);
    // Event handles support concurrent waits; the last Arc closes the handle.
    unsafe impl Send for Event {}
    unsafe impl Sync for Event {}
    impl Drop for Event {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let name = format!(
        "Local\\DSH-Sandbox-Ready-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    fn event(name: &str) -> Result<Arc<Event>, String> {
        let wide = name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) };
        if handle.is_null() {
            return Err(format!(
                "create sandbox startup event: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Arc::new(Event(handle)))
    }
    fn poll(event: &Event) -> Result<bool, String> {
        match unsafe { WaitForSingleObject(event.0, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(format!(
                "read sandbox startup event: {}",
                std::io::Error::last_os_error()
            )),
        }
    }
    let ready = event(&name)?;
    let timed_out = event(&format!("{name}-timeout"))?;
    let phases = [
        "ancestors",
        "workspace_permissions",
        "read_permissions",
        "runtime_permissions",
        "process_creation",
        "cleanup",
    ]
    .into_iter()
    .map(|phase| Ok((phase, event(&format!("{name}-stage-{phase}"))?)))
    .collect::<Result<Vec<_>, String>>()?;
    let signal = dsh_sandbox::SandboxStartup::new(move || poll(&ready), move || poll(&timed_out))
        .with_phase(move || {
            for (phase, event) in phases.iter().rev() {
                if poll(event)? {
                    return Ok((*phase).into());
                }
            }
            Ok("runner_initialization".into())
        });
    Ok((name, signal))
}

fn host_platform() -> String {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    }
    .to_string()
}

fn confined_mode(mode: SandboxMode) -> Option<ConfinedSandboxMode> {
    match mode {
        SandboxMode::ReadOnly => Some(ConfinedSandboxMode::ReadOnly),
        SandboxMode::WorkspaceWrite => Some(ConfinedSandboxMode::WorkspaceWrite),
        SandboxMode::DangerFullAccess => None,
    }
}

fn bwrap_profile_args(policy: &SandboxPolicy) -> Vec<String> {
    let mut args = vec![
        "bwrap".to_string(),
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--die-with-parent".to_string(),
    ];
    if policy.mode == ConfinedSandboxMode::WorkspaceWrite {
        args.extend([
            "--tmpfs".to_string(),
            "/tmp".to_string(),
            "--bind".to_string(),
            policy.workspace_root.clone(),
            policy.workspace_root.clone(),
        ]);
        for root in dsh_sandbox::roots::managed_temp_roots() {
            args.extend(["--bind".into(), root.clone(), root]);
        }
    }
    args
}

fn sbpl_string(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

fn seatbelt_profile_args(policy: &SandboxPolicy) -> Vec<String> {
    let mut forms = vec![
        "(version 1)".to_string(),
        "(allow default)".to_string(),
        "(deny file-write*)".to_string(),
        format!("(allow file-write* (literal {}))", sbpl_string("/dev/null")),
    ];
    let roots = writable_roots(&SandboxExecutionPolicy {
        read_only_roots: policy.read_only_roots.clone(),
        mode: match policy.mode {
            ConfinedSandboxMode::ReadOnly => SandboxMode::ReadOnly,
            ConfinedSandboxMode::WorkspaceWrite => SandboxMode::WorkspaceWrite,
        },
        workspace_root: policy.workspace_root.clone(),
        session_id: policy.session_id.clone(),
    });
    if !roots.is_empty() {
        forms.push(format!(
            "(allow file-write* {})",
            roots
                .iter()
                .map(|root| format!("(subpath {})", sbpl_string(root)))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    vec![
        "sandbox-exec".to_string(),
        "-p".to_string(),
        forms.join(" "),
    ]
}

fn windows_profile_args(policy: &SandboxPolicy) -> Result<Vec<String>, SandboxUnavailableError> {
    let current = std::env::current_exe().ok();
    let runner = match std::env::var_os("DSH_SANDBOX_WINDOWS_RUNNER") {
        Some(configured) => {
            let configured = std::path::PathBuf::from(configured);
            if !configured.is_file() {
                return Err(SandboxUnavailableError::new(
                    policy.mode,
                    Some("the configured Windows sandbox runner does not exist"),
                ));
            }
            configured
        }
        None => embedded_runner_path()
            .filter(|candidate| candidate.is_file())
            .or_else(|| {
                current.clone().filter(|candidate| {
                    candidate
                        .file_stem()
                        .is_some_and(|stem| stem.eq_ignore_ascii_case("dsh"))
                })
            })
            .or_else(|| {
                current
                    .as_ref()
                    .and_then(|executable| executable.parent())
                    .map(|parent| parent.join("dsh-sandbox-windows.exe"))
                    .filter(|candidate| candidate.is_file())
            })
            // Cargo integration binaries live under target/<profile>/deps,
            // while the sandbox runner is emitted one directory above.
            .or_else(|| {
                current
                    .as_ref()
                    .and_then(|executable| executable.parent())
                    .and_then(std::path::Path::parent)
                    .map(|parent| parent.join("dsh-sandbox-windows.exe"))
                    .filter(|candidate| candidate.is_file())
            })
            .ok_or_else(|| {
                SandboxUnavailableError::new(
                    policy.mode,
                    Some("no Windows sandbox runner is installed or embedded in dsh.exe"),
                )
            })?,
    };
    let embedded = embedded_runner_path().as_ref() == Some(&runner)
        || runner
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("dsh"));
    let mut args = vec![runner.to_string_lossy().into_owned()];
    if embedded {
        args.push("__dsh-sandbox-windows".to_string());
    }
    args.extend([
        "--mode".to_string(),
        policy.mode.as_str().to_string(),
        "--workspace".to_string(),
        policy.workspace_root.clone(),
    ]);
    if policy.mode == ConfinedSandboxMode::WorkspaceWrite {
        for root in dsh_sandbox::roots::managed_temp_roots() {
            args.extend(["--temp-root".to_string(), root]);
        }
    }
    for root in &policy.read_only_roots {
        args.extend(["--read-root".into(), root.clone()]);
    }
    Ok(args)
}

#[cfg(windows)]
#[allow(dead_code)]
#[path = "bin/dsh-sandbox-windows.rs"]
mod embedded_windows_runner;

#[cfg(windows)]
pub fn run_windows_sandbox(args: impl IntoIterator<Item = String>) -> Result<i32, String> {
    embedded_windows_runner::windows_runner::run_args(args.into_iter())
}
