#[cfg(windows)]
mod native_backend;
#[cfg(windows)]
pub use native_backend::{windows_backend_configuration, windows_backend_manage};

use std::sync::Arc;

/// Retained for older callers; Windows native helpers are discovered from the
/// installation and no embedded AppContainer runner is registered.
#[cfg(windows)]
pub fn register_embedded_windows_runner() -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
pub fn run_windows_sandbox(_args: impl IntoIterator<Item = String>) -> Result<i32, String> {
    Err(
        "AppContainer execution is retired; use the elevated or unelevated native Windows backend"
            .into(),
    )
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
    #[cfg(windows)]
    native: Result<Option<native_backend::NativeBackend>, String>,
    #[cfg(windows)]
    native_home: Option<std::path::PathBuf>,
    platform: String,
    runtime_roots: Vec<std::path::PathBuf>,
}

impl LocalSandboxProvider {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self {
            #[cfg(windows)]
            native: Ok(None),
            #[cfg(windows)]
            native_home: Some(dsh_home_paths::resolve_dsh_home(None, &|key| {
                std::env::var(key).ok()
            })),
            platform: config.platform.unwrap_or_else(host_platform),
            runtime_roots: Vec::new(),
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
            #[cfg(windows)]
            native: native_backend::NativeBackend::load(&cache),
            #[cfg(windows)]
            native_home: cache
                .parent()
                .and_then(std::path::Path::parent)
                .map(std::path::Path::to_path_buf),
            platform: config.platform.unwrap_or_else(host_platform),
            runtime_roots: roots,
        });
        let erased: Arc<dyn SandboxProvider> = provider.clone();
        ctx.register_service(erased);
        provider
    }

    #[cfg(windows)]
    fn native_selection(&self) -> Result<Option<native_backend::NativeBackend>, String> {
        match &self.native_home {
            Some(home) => native_backend::NativeBackend::load_from_home(home),
            None => self.native.clone(),
        }
    }

    pub fn install_with_runtimes_at_home(
        ctx: &Context,
        config: Config,
        roots: Vec<std::path::PathBuf>,
        cache: std::path::PathBuf,
        home: std::path::PathBuf,
    ) -> Arc<Self> {
        let mut provider = Self::new(config);
        let state = Arc::get_mut(&mut provider).expect("new provider");
        state.runtime_roots = roots;
        let _ = cache;
        #[cfg(windows)]
        {
            state.native_home = Some(home);
        }
        #[cfg(not(windows))]
        {
            let _ = home;
        }
        let erased: Arc<dyn SandboxProvider> = provider.clone();
        ctx.register_service(erased);
        provider
    }

    pub fn capability(&self) -> SandboxCapability {
        match self.platform.as_str() {
            "linux" | "darwin" => SandboxCapability::Full,
            #[cfg(windows)]
            "win32"
                if self
                    .native_selection()
                    .is_ok_and(|v| v.is_some_and(|v| v.verify().is_ok())) =>
            {
                SandboxCapability::Full
            }
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
    fn backend_id_for(&self, _policy: &SandboxExecutionPolicy) -> &'static str {
        self.backend_id()
    }
    fn backend_fingerprint_for(&self, policy: &SandboxExecutionPolicy) -> String {
        if self.backend_id_for(policy) != self.backend_id() {
            self.backend_id_for(policy).to_owned()
        } else {
            self.backend_fingerprint()
        }
    }
    fn backend_id(&self) -> &'static str {
        #[cfg(windows)]
        if self.platform == "win32" {
            return match self.native_selection() {
                Ok(Some(native)) if native.verify().is_ok() => {
                    if native.implementation == "unelevated" {
                        "windows-unelevated"
                    } else {
                        "windows-elevated"
                    }
                }
                Ok(Some(_)) => "windows-unavailable",
                Ok(None) => "windows-unavailable",
                Err(_) => "windows-unavailable",
            };
        }
        match self.platform.as_str() {
            "linux" => "linux-bwrap",
            "darwin" => "macos-seatbelt",
            _ => "local",
        }
    }

    fn backend_fingerprint(&self) -> String {
        #[cfg(windows)]
        if let Ok(Some(native)) = self.native_selection() {
            return format!(
                "windows-native:{}:{}:{}:{}:{}:{}",
                native.sha256,
                native.command_runner_sha256,
                native.setup_sha256,
                native.state_directory.display(),
                native.implementation,
                native.network
            );
        }
        self.backend_id().to_owned()
    }

    fn confine_with_startup(
        &self,
        argv: &[String],
        policy: &SandboxPolicy,
    ) -> Result<ConfinedArgv, SandboxUnavailableError> {
        let mut confined = self.confine(argv, policy)?;
        #[cfg(windows)]
        if self.platform == "win32" {
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
        if self.platform == "win32" && policy.mode != SandboxMode::DangerFullAccess {
            match self.native_selection() {
                Err(error) => {
                    let error = error.clone();
                    return Box::pin(async move { Err(error) });
                }
                Ok(Some(native)) => {
                    let native = native.clone();
                    let workspace = policy.workspace_root.clone();
                    let read_only = policy.mode == SandboxMode::ReadOnly;
                    return Box::pin(async move {
                        tokio::task::spawn_blocking(move || native.prepare(&workspace, read_only))
                            .await
                            .map_err(|e| format!("native readiness task: {e}"))?
                    });
                }
                Ok(None) => {
                    return Box::pin(async {
                        Err("[SANDBOX_SETUP_REQUIRED] packaged Windows native sandbox is unavailable".into())
                    });
                }
            }
        }
        let _ = policy;
        Box::pin(async { Ok(()) })
    }

    fn confine(
        &self,
        argv: &[String],
        policy: &SandboxPolicy,
    ) -> Result<ConfinedArgv, SandboxUnavailableError> {
        #[cfg(windows)]
        if self.platform == "win32" {
            match self.native_selection() {
                Err(error) => return Err(SandboxUnavailableError::new(policy.mode, Some(&error))),
                Ok(Some(native)) => {
                    native
                        .verify()
                        .and_then(|_| native.validate_scope(&policy.workspace_root))
                        .map_err(|e| SandboxUnavailableError::new(policy.mode, Some(&e)))?;
                    return Ok(native.confine(argv, policy, &self.runtime_roots));
                }
                Ok(None) => {
                    return Err(SandboxUnavailableError::new(
                        policy.mode,
                        Some("packaged native Windows sandbox is unavailable"),
                    ));
                }
            }
        }
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
            _ => return Err(SandboxUnavailableError::new(policy.mode, None)),
        };
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
