//! Explicit native backend selection owned by the host, never by tool arguments.
use dsh_sandbox::{ConfinedArgv, SandboxEnforcement, SandboxPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

fn digest_reader(mut reader: impl Read) -> std::io::Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = match reader.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn digest_file(path: &Path) -> std::io::Result<String> {
    digest_reader(std::fs::File::open(path)?)
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeBackend {
    #[serde(default)]
    pub workspaces: Vec<PathBuf>,
    pub version: u32,
    pub backend: String,
    #[serde(default = "elevated")]
    pub implementation: String,
    #[serde(default = "enabled")]
    pub network: String,
    pub runner: PathBuf,
    pub state_directory: PathBuf,
    pub sha256: String,
    pub command_runner_sha256: String,
    pub setup_sha256: String,
}

fn elevated() -> String {
    "elevated".into()
}
fn enabled() -> String {
    "enabled".into()
}

static INITIALIZATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn prepare_with<P, PF, S, SF>(mut probe: P, mut setup: S) -> Result<(), String>
where
    P: FnMut() -> PF,
    PF: std::future::Future<Output = Result<bool, String>>,
    S: FnMut() -> SF,
    SF: std::future::Future<Output = Result<(), String>>,
{
    if probe().await? {
        return Ok(());
    }
    let _initialization = INITIALIZATION.lock().await;
    if probe().await? {
        return Ok(());
    }
    setup().await?;
    if !probe().await? {
        return Err("[SANDBOX_SETUP_FAILED] initialization did not publish a ready workspace; requested command not dispatched".into());
    }
    Ok(())
}

fn readiness(output: &std::process::Output) -> Result<bool, String> {
    if !output.status.success() {
        return Err(format!(
            "[SANDBOX_STATUS_FAILED] native readiness check failed: {}; requested command not dispatched",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(4000)
                .collect::<String>()
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("[SANDBOX_STATUS_FAILED] invalid readiness response: {e}"))?;
    if value["protocolVersion"].as_u64() != Some(1) {
        return Err("[SANDBOX_STATUS_FAILED] unsupported readiness protocol; requested command not dispatched".into());
    }
    value["initialized"].as_bool().ok_or_else(|| "[SANDBOX_STATUS_FAILED] readiness response lacks initialized state; requested command not dispatched".into())
}

fn config_revision(home: &Path) -> Result<String, String> {
    match std::fs::read(home.join("windows-sandbox.json")) {
        Ok(bytes) => Ok(format!("{:x}", Sha256::digest(bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("default".into()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn windows_backend_configuration(home: &Path) -> Result<serde_json::Value, String> {
    let config =
        NativeBackend::load_from_home(home)?.ok_or("Windows native sandbox is unavailable")?;
    config.verify()?;
    Ok(
        serde_json::json!({"available":true,"revision":config_revision(home)?,"implementation":config.implementation,"network":config.network,"runner":config.runner,"stateDirectory":config.state_directory,"readScope":if config.implementation=="unelevated" {"current-user"} else {"dedicated-account"},"networkIsolation":if config.implementation=="unelevated" {"environment"} else {"account-firewall"}}),
    )
}

pub async fn windows_backend_manage(
    home: PathBuf,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    static MUTATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = MUTATION
        .try_lock()
        .map_err(|_| "Windows sandbox configuration or setup is already running")?;
    let expected = args["expectedRevision"]
        .as_str()
        .ok_or("Missing configuration revision")?;
    if config_revision(&home)? != expected {
        return Err("Sandbox configuration changed; refresh before retrying".into());
    }
    let mut config = NativeBackend::load_from_home(&home)?.ok_or("Native backend unavailable")?;
    config.implementation = args["implementation"]
        .as_str()
        .ok_or("Select elevated or unelevated")?
        .into();
    config.network = args["network"]
        .as_str()
        .ok_or("Select enabled or restricted networking")?
        .into();
    config.workspaces.clear();
    config.verify()?;
    match args["action"].as_str() {
        Some("setup") => {
            let workspace = PathBuf::from(args["workspace"].as_str().ok_or("Select a workspace")?);
            if !workspace.is_absolute() || !workspace.is_dir() {
                return Err("Workspace must be an existing absolute directory".into());
            }
            config.validate_scope(&workspace.to_string_lossy())?;
            let _initialization = INITIALIZATION.lock().await;
            config
                .initialize(&workspace.to_string_lossy(), false)
                .await?;
            if !config.probe(&workspace.to_string_lossy(), false).await? {
                return Err(
                    "[SANDBOX_SETUP_FAILED] setup completed without a ready workspace".into(),
                );
            }
        }
        Some("configure") => {}
        _ => return Err("Unknown Windows sandbox operation".into()),
    }
    if config_revision(&home)? != expected {
        return Err("Configuration changed during setup; initialization finished but selection was not overwritten".into());
    }
    dsh_atomic_write::write_file_atomic(
        &home.join("windows-sandbox.json"),
        &serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?,
        dsh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    windows_backend_configuration(&home)
}

impl NativeBackend {
    pub fn matches_workspace(&self, workspace: &str) -> bool {
        if self.workspaces.is_empty() {
            return true;
        }
        let normalized = |p: &Path| {
            std::fs::canonicalize(p)
                .unwrap_or_else(|_| p.to_path_buf())
                .to_string_lossy()
                .to_lowercase()
        };
        let workspace = normalized(Path::new(workspace));
        self.workspaces
            .iter()
            .any(|path| normalized(path) == workspace)
    }
    pub fn load(cache: &Path) -> Result<Option<Self>, String> {
        let Some(root) = cache.parent().and_then(Path::parent) else {
            return Ok(None);
        };
        Self::load_from_home(root)
    }

    pub(crate) fn load_from_home(root: &Path) -> Result<Option<Self>, String> {
        let path = root.join("windows-sandbox.json");
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Self::installed(root).map(Some);
            }
            Err(e) => {
                return Err(format!(
                    "[SANDBOX_SETUP_FAILED] cannot read backend selection: {e}"
                ));
            }
        };
        if data.len() > 16 * 1024 {
            return Err("[SANDBOX_SETUP_FAILED] backend selection exceeds size limit".into());
        }
        let config: Self = serde_json::from_slice(&data)
            .map_err(|e| format!("[SANDBOX_SETUP_FAILED] invalid native backend selection: {e}"))?;
        // Legacy workspace lists remain parseable, but Windows execution now
        // consistently uses the selected native implementation for every project.
        config.validate_configuration()?;
        Ok(Some(config))
    }

    pub(crate) fn installed(home: &Path) -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let folder = executable
            .parent()
            .ok_or("Host executable has no directory")?
            .join("native-sandbox");
        let digest = |name: &str| {
            digest_file(&folder.join(name)).map_err(|e| {
                format!(
                    "[SANDBOX_SETUP_REQUIRED] missing packaged Windows sandbox helper {name}: {e}"
                )
            })
        };
        Ok(Self {
            version: 1,
            backend: "windows-native".into(),
            implementation: elevated(),
            network: "restricted".into(),
            workspaces: vec![],
            runner: folder.join("dsh-windows-native.exe"),
            state_directory: home.join("windows-native"),
            sha256: digest("dsh-windows-native.exe")?,
            command_runner_sha256: digest("dsh-command-runner.exe")?,
            setup_sha256: digest("dsh-windows-sandbox-setup.exe")?,
        })
    }

    fn validate_configuration(&self) -> Result<(), String> {
        if self.version != 1
            || self.backend != "windows-native"
            || !matches!(self.implementation.as_str(), "elevated" | "unelevated")
            || !matches!(self.network.as_str(), "enabled" | "restricted")
            || !self.runner.is_absolute()
            || !self.state_directory.is_absolute()
            || [&self.runner, &self.state_directory]
                .into_iter()
                .chain(self.workspaces.iter())
                .any(|path| {
                    path.components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
                })
            || self.workspaces.iter().any(|path| !path.is_absolute())
            || [
                &self.sha256,
                &self.command_runner_sha256,
                &self.setup_sha256,
            ]
            .iter()
            .any(|digest| digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err("[SANDBOX_SETUP_FAILED] unsupported native backend configuration".into());
        }
        Ok(())
    }

    pub fn verify(&self) -> Result<(), String> {
        self.validate_configuration()?;
        let folder = self
            .runner
            .parent()
            .ok_or("native runner has no parent directory")?;
        for (path, expected) in [
            (self.runner.clone(), &self.sha256),
            (
                folder.join("dsh-command-runner.exe"),
                &self.command_runner_sha256,
            ),
            (
                folder.join("dsh-windows-sandbox-setup.exe"),
                &self.setup_sha256,
            ),
        ] {
            let hash = digest_file(&path).map_err(|e| {
                format!(
                    "[SANDBOX_SETUP_FAILED] missing native helper {}: {e}",
                    path.display()
                )
            })?;
            if !hash.eq_ignore_ascii_case(expected) {
                return Err(format!(
                    "[SANDBOX_SETUP_FAILED] native helper identity mismatch: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    pub fn validate_scope(&self, workspace: &str) -> Result<(), String> {
        let normalize = |path: &Path| {
            let mut current = path;
            let mut tail = Vec::new();
            while !current.exists() {
                tail.push(
                    current
                        .file_name()
                        .ok_or("[SANDBOX_SETUP_FAILED] invalid sandbox path")?
                        .to_os_string(),
                );
                current = current
                    .parent()
                    .ok_or("[SANDBOX_SETUP_FAILED] sandbox path has no existing ancestor")?;
            }
            let mut resolved = std::fs::canonicalize(current)
                .map_err(|e| format!("[SANDBOX_SETUP_FAILED] cannot resolve sandbox path: {e}"))?;
            for component in tail.into_iter().rev() {
                resolved.push(component);
            }
            Ok::<_, String>(PathBuf::from(resolved.to_string_lossy().to_lowercase()))
        };
        let workspace = normalize(Path::new(workspace))?;
        let program_data = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("DeepSeekHarnessNative");
        for protected in [
            self.runner.parent().unwrap().to_path_buf(),
            self.state_directory.clone(),
            program_data,
        ] {
            let protected = normalize(&protected)?;
            if workspace.starts_with(&protected) || protected.starts_with(&workspace) {
                return Err("[SANDBOX_SETUP_FAILED] workspace overlaps native sandbox executables or private state".into());
            }
        }
        Ok(())
    }

    pub async fn prepare(&self, workspace: &str, read_only: bool) -> Result<(), String> {
        self.verify()?;
        if !Path::new(workspace).is_absolute() || !Path::new(workspace).is_dir() {
            return Err("[SANDBOX_SETUP_FAILED] workspace must be an existing absolute directory; requested command not dispatched".into());
        }
        self.validate_scope(workspace)?;
        prepare_with(
            || self.probe(workspace, read_only),
            || self.initialize(workspace, read_only),
        )
        .await
    }

    async fn probe(&self, workspace: &str, read_only: bool) -> Result<bool, String> {
        readiness(&self.helper("--status", workspace, read_only, 30).await?)
    }

    async fn initialize(&self, workspace: &str, read_only: bool) -> Result<(), String> {
        self.verify()?;
        self.validate_scope(workspace)?;
        let output = self.helper("--setup", workspace, read_only, 900).await?;
        if !output.status.success() {
            return Err(format!(
                "[SANDBOX_SETUP_FAILED] automatic workspace initialization failed: {}; requested command not dispatched",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(4000)
                    .collect::<String>()
            ));
        }
        Ok(())
    }

    async fn helper(
        &self,
        action: &str,
        workspace: &str,
        read_only: bool,
        timeout_seconds: u64,
    ) -> Result<std::process::Output, String> {
        let mut command = tokio::process::Command::new(&self.runner);
        command
            .args([
                action,
                "--implementation",
                &self.implementation,
                "--network",
                &self.network,
                "--native-home",
            ])
            .arg(&self.state_directory)
            .args([
                "--workspace",
                workspace,
                "--mode",
                if read_only {
                    "read-only"
                } else {
                    "workspace-write"
                },
            ])
            .creation_flags(0x08000000)
            .kill_on_drop(true);
        tokio::time::timeout(std::time::Duration::from_secs(timeout_seconds), command.output()).await
            .map_err(|_| "[SANDBOX_SETUP_TIMEOUT] native environment preparation timed out; requested command not dispatched".to_string())?
            .map_err(|e| format!("[SANDBOX_SETUP_FAILED] native helper launch failed: {e}; requested command not dispatched"))
    }

    pub fn confine(
        &self,
        argv: &[String],
        policy: &SandboxPolicy,
        runtime_roots: &[PathBuf],
    ) -> ConfinedArgv {
        let mut wrapped = vec![
            self.runner.to_string_lossy().into_owned(),
            "--implementation".into(),
            self.implementation.clone(),
            "--network".into(),
            self.network.clone(),
            "--native-home".into(),
            self.state_directory.to_string_lossy().into_owned(),
            "--mode".into(),
            policy.mode.as_str().into(),
            "--workspace".into(),
            policy.workspace_root.clone(),
        ];
        if let Some(session) = &policy.session_id {
            wrapped.extend(["--session-id".into(), session.as_str().into()]);
        }
        for root in &policy.read_only_roots {
            wrapped.extend(["--read-root".into(), root.clone()]);
        }
        for root in runtime_roots {
            wrapped.extend(["--runtime-root".into(), root.to_string_lossy().into_owned()]);
        }
        if policy.mode == dsh_sandbox::ConfinedSandboxMode::WorkspaceWrite {
            for root in dsh_sandbox::roots::managed_temp_roots() {
                wrapped.extend(["--temp-root".into(), root]);
            }
        }
        wrapped.push("--".into());
        wrapped.extend_from_slice(argv);
        ConfinedArgv {
            argv: wrapped,
            enforcement: SandboxEnforcement::Full,
            denial_signatures: vec![
                "access is denied".into(),
                "permission denied".into(),
                "unauthorizedaccessexception".into(),
            ],
            // Startup failures are authenticated by the readiness channel.
            // Child stderr after readiness must not impersonate runner errors.
            runner_failure_rules: vec![],
            startup: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_or_failed_status_never_becomes_an_initialization_request() {
        use std::os::windows::process::ExitStatusExt;
        let output = |status, stdout: &[u8]| std::process::Output {
            status: std::process::ExitStatus::from_raw(status),
            stdout: stdout.to_vec(),
            stderr: b"owner check failed".to_vec(),
        };
        assert_eq!(
            readiness(&output(0, br#"{"protocolVersion":1,"initialized":false}"#)).unwrap(),
            false
        );
        assert_eq!(
            readiness(&output(0, br#"{"protocolVersion":1,"initialized":true}"#)).unwrap(),
            true
        );
        assert!(
            readiness(&output(
                125,
                br#"{"protocolVersion":1,"initialized":false}"#
            ))
            .is_err()
        );
        assert!(readiness(&output(0, br#"{"protocolVersion":2,"initialized":false}"#)).is_err());
        assert!(readiness(&output(0, br#"{"protocolVersion":1}"#)).is_err());
    }

    #[tokio::test]
    async fn concurrent_first_use_initializes_once_and_ready_workspaces_skip_setup() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let ready = std::sync::Arc::new(AtomicBool::new(false));
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let run = || {
            let ready_probe = ready.clone();
            let ready_setup = ready.clone();
            let count = count.clone();
            prepare_with(
                move || {
                    let ready = ready_probe.clone();
                    async move { Ok(ready.load(Ordering::Acquire)) }
                },
                move || {
                    let ready = ready_setup.clone();
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, Ordering::Relaxed);
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        ready.store(true, Ordering::Release);
                        Ok(())
                    }
                },
            )
        };
        let (a, b) = tokio::join!(run(), run());
        a.unwrap();
        b.unwrap();
        run().await.unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn setup_failure_and_cancellation_do_not_leave_the_preparation_gate_locked() {
        let failed = prepare_with(
            || async { Ok(false) },
            || async { Err("setup refused".into()) },
        )
        .await
        .unwrap_err();
        assert_eq!(failed, "setup refused");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                prepare_with(
                    || async { Ok(false) },
                    || std::future::pending::<Result<(), String>>()
                )
            )
            .await
            .is_err()
        );
        assert!(
            prepare_with(|| async { Ok(false) }, || async { Ok(()) })
                .await
                .unwrap_err()
                .contains("did not publish")
        );
        prepare_with(
            || async { Ok(true) },
            || async { panic!("ready workspace must not run setup") },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires an explicit owned live workspace and installed helper selection"]
    async fn explicit_live_workspace_automatically_prepares_before_isolated_execution() {
        let home =
            PathBuf::from(std::env::var_os("DSH_NATIVE_LIVE_HOME").expect("explicit live home"));
        let workspace =
            std::env::var("DSH_NATIVE_LIVE_WORKSPACE").expect("explicit live workspace");
        let config = NativeBackend::load_from_home(&home).unwrap().unwrap();
        assert_eq!(config.implementation, "elevated");
        config.verify().unwrap();
        config.validate_scope(&workspace).unwrap();
        let before = config.probe(&workspace, false).await.unwrap();
        config.prepare(&workspace, false).await.unwrap();
        assert!(config.probe(&workspace, false).await.unwrap());
        let program = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/whoami.exe")
            .to_string_lossy()
            .into_owned();
        let confined = config.confine(
            &[program.clone()],
            &SandboxPolicy {
                mode: dsh_sandbox::ConfinedSandboxMode::WorkspaceWrite,
                workspace_root: workspace.clone(),
                read_only_roots: vec![],
                session_id: None,
            },
            &[],
        );
        let output = tokio::process::Command::new(&confined.argv[0])
            .args(&confined.argv[1..])
            .creation_flags(0x08000000)
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "isolated probe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let host = tokio::process::Command::new(program)
            .creation_flags(0x08000000)
            .output()
            .await
            .unwrap();
        assert_ne!(
            output.stdout, host.stdout,
            "workspace command must use an independent principal"
        );
        println!(
            "{}",
            serde_json::json!({"initializedBefore":before,"initializedAfter":true,"independentPrincipal":true,"workspace":workspace,"commandExit":output.status.code(),"implementation":config.implementation,"network":config.network})
        );
    }
    use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode, SandboxProvider};

    #[test]
    fn helper_identity_is_hashed_in_bounded_chunks_and_propagates_read_failures() {
        struct Generated {
            remaining: usize,
            first: bool,
        }
        impl Read for Generated {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                assert!(buffer.len() <= 64 * 1024);
                if self.first {
                    self.first = false;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                let len = self.remaining.min(buffer.len());
                buffer[..len].fill(42);
                self.remaining -= len;
                Ok(len)
            }
        }
        let mut expected = Sha256::new();
        for _ in 0..256 {
            expected.update([42u8; 8192]);
        }
        assert_eq!(
            digest_reader(Generated {
                remaining: 2 * 1024 * 1024,
                first: true
            })
            .unwrap(),
            format!("{:x}", expected.finalize())
        );
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::PermissionDenied.into())
            }
        }
        assert_eq!(
            digest_reader(Failed).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn private_state_overlap_is_rejected_before_state_directory_exists() {
        let root = std::env::temp_dir().join(format!("dsh-native-overlap-{}", std::process::id()));
        let workspace = root.join("project");
        let install = root.join("install");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&install).unwrap();
        let mut config:NativeBackend=serde_json::from_value(serde_json::json!({"version":1,"backend":"windows-native","runner":install.join("runner.exe"),"stateDirectory":workspace.join("missing/state"),"sha256":"0".repeat(64),"commandRunnerSha256":"0".repeat(64),"setupSha256":"0".repeat(64)})).unwrap();
        assert!(
            config
                .validate_scope(&workspace.to_string_lossy())
                .unwrap_err()
                .contains("overlaps")
        );
        assert!(!config.state_directory.exists());
        config.state_directory = root.join("separate/state");
        config.validate_scope(&workspace.to_string_lossy()).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn invalid_native_identity_never_falls_back_to_appcontainer() {
        let root = std::env::temp_dir().join(format!("dsh-native-scope-{}", std::process::id()));
        std::fs::create_dir_all(root.join("cache")).unwrap();
        let selected = root.join("selected");
        let other = root.join("other");
        let value = serde_json::json!({
            "version": 1, "backend": "windows-native", "runner": root.join("missing.exe"),
            "stateDirectory": root.join("state"), "workspaces": [selected],
            "sha256": "0".repeat(64), "commandRunnerSha256": "0".repeat(64), "setupSha256": "0".repeat(64)
        });
        std::fs::write(
            root.join("windows-sandbox.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let native = NativeBackend::load(&root.join("cache/runtime-read-permissions"))
            .unwrap()
            .unwrap();
        assert!(native.verify().is_err());
        let mut provider = crate::LocalSandboxProvider::new(crate::Config::default());
        std::sync::Arc::get_mut(&mut provider).unwrap().native_home = None;
        std::sync::Arc::get_mut(&mut provider).unwrap().native = Ok(Some(native));
        let mut policy = SandboxExecutionPolicy {
            mode: SandboxMode::WorkspaceWrite,
            workspace_root: other.to_string_lossy().into_owned(),
            read_only_roots: vec![],
            session_id: None,
        };
        assert_eq!(provider.backend_id_for(&policy), "windows-unavailable");
        policy.workspace_root = selected.to_string_lossy().into_owned();
        assert_eq!(provider.backend_id_for(&policy), "windows-unavailable");
        assert!(provider.prepare(&policy).await.is_err());
        std::fs::remove_file(root.join("windows-sandbox.json")).unwrap();
        std::fs::remove_dir(root.join("cache")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn invalid_selection_cannot_silently_become_appcontainer_or_unconfined() {
        let directory =
            std::env::temp_dir().join(format!("dsh-native-selection-{}", std::process::id()));
        std::fs::create_dir_all(directory.join("cache")).unwrap();
        std::fs::write(directory.join("windows-sandbox.json"), b"{not-json}").unwrap();
        assert!(NativeBackend::load(&directory.join("cache/runtime-read-permissions")).is_err());
        std::fs::remove_file(directory.join("windows-sandbox.json")).unwrap();
        assert!(NativeBackend::load(&directory.join("cache/runtime-read-permissions")).is_err());
    }
}
