//! Explicit native backend selection owned by the host, never by tool arguments.
use dsh_sandbox::{ConfinedArgv, SandboxEnforcement, SandboxPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Clone, Deserialize, Serialize)]
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
            let setup = config.clone();
            tokio::task::spawn_blocking(move || {
                use std::os::windows::process::CommandExt;
                let output = std::process::Command::new(&setup.runner)
                    .args([
                        "--setup",
                        "--implementation",
                        &setup.implementation,
                        "--network",
                        &setup.network,
                        "--native-home",
                    ])
                    .arg(&setup.state_directory)
                    .arg("--workspace")
                    .arg(workspace)
                    .creation_flags(0x08000000)
                    .output()
                    .map_err(|e| e.to_string())?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "Windows sandbox setup failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                            .chars()
                            .take(4000)
                            .collect::<String>()
                    ))
                }
            })
            .await
            .map_err(|e| e.to_string())??;
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
            std::fs::read(folder.join(name)).map(|bytes| format!("{:x}", Sha256::digest(bytes))).map_err(|e| format!("[SANDBOX_SETUP_REQUIRED] missing packaged Windows sandbox helper {name}: {e}"))
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
            let bytes = std::fs::read(&path).map_err(|e| {
                format!(
                    "[SANDBOX_SETUP_FAILED] missing native helper {}: {e}",
                    path.display()
                )
            })?;
            let hash = format!("{:x}", Sha256::digest(bytes));
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

    pub fn prepare(&self, workspace: &str, read_only: bool) -> Result<(), String> {
        self.verify()?;
        self.validate_scope(workspace)?;
        use std::os::windows::process::CommandExt;
        let result = std::process::Command::new(&self.runner)
            .args([
                "--status",
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
            .output()
            .map_err(|e| format!("[SANDBOX_SETUP_FAILED] native status: {e}"))?;
        let ready = serde_json::from_slice::<serde_json::Value>(&result.stdout)
            .ok()
            .is_some_and(|s| s["initialized"] == true && s["protocolVersion"] == 1);
        if !result.status.success() || !ready {
            return Err("[SANDBOX_SETUP_REQUIRED] Windows native backend is not initialized for this workspace; call environment_initialize for this workspace, then retry validation; Settings → Windows sandbox is also available; command not dispatched".into());
        }
        Ok(())
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
    use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode, SandboxProvider};

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
