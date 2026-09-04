use std::sync::Arc;

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
}

impl LocalSandboxProvider {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self {
            platform: config.platform.unwrap_or_else(host_platform),
        })
    }

    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let provider = Self::new(config);
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
                mode,
                workspace_root: policy.workspace_root.clone(),
                session_id: policy.session_id.clone(),
            },
        )
        .map(SandboxLaunch::Confined)
    }
}

impl SandboxProvider for LocalSandboxProvider {
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
        wrapped.push("--".to_string());
        wrapped.extend_from_slice(argv);
        Ok(ConfinedArgv {
            argv: wrapped,
            enforcement: SandboxEnforcement::Full,
            denial_signatures,
            runner_failure_rules,
        })
    }
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
