//! Explicit native backend selection owned by the host, never by tool arguments.
use dsh_sandbox::{ConfinedArgv, RunnerFailureRule, SandboxEnforcement, SandboxPolicy};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeBackend {
    #[serde(default)]
    pub workspaces: Vec<PathBuf>,
    pub version: u32,
    pub backend: String,
    pub runner: PathBuf,
    pub state_directory: PathBuf,
    pub sha256: String,
    pub command_runner_sha256: String,
    pub setup_sha256: String,
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
        let path = root.join("windows-sandbox.json");
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
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
        config.verify()?;
        Ok(Some(config))
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.version != 1
            || self.backend != "windows-native"
            || !self.runner.is_absolute()
            || !self.state_directory.is_absolute()
        {
            return Err("[SANDBOX_SETUP_FAILED] unsupported native backend configuration".into());
        }
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
            std::fs::canonicalize(path)
                .map(|p| PathBuf::from(p.to_string_lossy().to_lowercase()))
                .map_err(|e| format!("[SANDBOX_SETUP_FAILED] cannot resolve sandbox path: {e}"))
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
            if protected.exists() {
                let protected = normalize(&protected)?;
                if workspace.starts_with(&protected) || protected.starts_with(&workspace) {
                    return Err("[SANDBOX_SETUP_FAILED] workspace overlaps native sandbox executables or private state".into());
                }
            }
        }
        Ok(())
    }

    pub fn prepare(&self, workspace: &str, read_only: bool) -> Result<(), String> {
        self.verify()?;
        self.validate_scope(workspace)?;
        use std::os::windows::process::CommandExt;
        let result = std::process::Command::new(&self.runner)
            .args(["--status", "--native-home"])
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
            return Err("[SANDBOX_SETUP_REQUIRED] Windows native backend is not initialized; command not dispatched".into());
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
            runner_failure_rules: vec![RunnerFailureRule {
                allowed_exit_codes: Some(vec![125]),
                fatal_signatures: vec!["[DSH_NATIVE_SANDBOX_FAILED]".into()],
                informational_lines: None,
            }],
            startup: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_selection_cannot_silently_become_appcontainer_or_unconfined() {
        let directory =
            std::env::temp_dir().join(format!("dsh-native-selection-{}", std::process::id()));
        std::fs::create_dir_all(directory.join("cache")).unwrap();
        std::fs::write(directory.join("windows-sandbox.json"), b"{not-json}").unwrap();
        assert!(NativeBackend::load(&directory.join("cache/runtime-read-permissions")).is_err());
        std::fs::remove_file(directory.join("windows-sandbox.json")).unwrap();
        assert!(
            NativeBackend::load(&directory.join("cache/runtime-read-permissions"))
                .unwrap()
                .is_none()
        );
    }
}
