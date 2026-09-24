//! Trusted human terminal entry, separate from the model-facing backend trait.
use super::*;

pub struct UserTerminalSpawnSpec {
    pub terminal_id: dsh_terminal::TerminalSessionId,
    pub session_id: String,
    pub cwd: String,
    pub signal: dsh_terminal::TerminalAbort,
}

impl ShellTerminalBackend {
    /// Called only by authenticated UI terminal controls. It never changes the
    /// agent's policy and does not require a live agent or a sandbox slot.
    pub fn spawn_user(
        &self,
        spec: UserTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>>
    {
        let subprocess = self.subprocess.clone();
        let profiles = execution_profiles(&self.ctx, &self.config.backend_type);
        let mut config = self.config.clone();
        Box::pin(async move {
            if (spec.signal)() {
                return Err(TerminalBackendSpawnError::coded(
                    "terminal startup cancelled",
                    TerminalErrorCode::Aborted,
                ));
            }
            let cwd = std::fs::canonicalize(&spec.cwd)
                .map_err(|error| TerminalBackendSpawnError::spawn(error.to_string()))?;
            if !cwd.is_dir() {
                return Err(TerminalBackendSpawnError::spawn(
                    "terminal workspace is not a directory",
                ));
            }
            let cwd = cwd.to_string_lossy().into_owned();
            let mut context_id = None;
            if let Some(profiles) = profiles {
                let profile = profiles
                    .resolve(Some(&spec.session_id), &cwd)
                    .map_err(TerminalBackendSpawnError::spawn)?;
                config.shell_path = profile.shell_path.ok_or_else(|| {
                    TerminalBackendSpawnError::spawn(
                        "Selected terminal shell has no resolved executable",
                    )
                })?;
                config.shell_args = match profile.shell_kind.as_str() {
                    "powershell" | "pwsh" => vec![
                        "-NoLogo".into(),
                        "-NoProfile".into(),
                        "-NoExit".into(),
                        "-Command".into(),
                        "$ProgressPreference='SilentlyContinue'".into(),
                    ],
                    "bash" => vec!["--noprofile".into(), "--norc".into(), "-i".into()],
                    "zsh" => vec!["-f".into(), "-i".into()],
                    other => {
                        return Err(TerminalBackendSpawnError::spawn(format!(
                            "Unsupported selected terminal shell: {other}"
                        )));
                    }
                };
                profiles
                    .validate(dsh_shell::ExecutionValidationRequest {
                        session_id: Some(spec.session_id.clone()),
                        workdir: cwd.clone(),
                        capability: "shell".into(),
                        executable: Some(config.shell_path.clone()),
                        shell_kind: Some(profile.shell_kind),
                        execution_context_id: Some(profile.context_id.clone()),
                        sandbox_policy: Some(dsh_sandbox::SandboxExecutionPolicy {
                            mode: SandboxMode::DangerFullAccess,
                            workspace_root: cwd.clone(),
                            session_id: None,
                            read_only_roots: vec![],
                        }),
                        signal: Some(spec.signal.clone()),
                    })
                    .await
                    .map_err(TerminalBackendSpawnError::spawn)?;
                context_id = Some(profile.context_id);
            }
            if (spec.signal)() {
                return Err(TerminalBackendSpawnError::coded(
                    "terminal startup cancelled",
                    TerminalErrorCode::Aborted,
                ));
            }
            let mut argv = vec![config.shell_path.clone()];
            argv.extend(config.shell_args.clone());
            let env = dsh_shell::developer_environment::environment()
                .into_iter()
                .chain(
                    dsh_shell::powershell::system_execution_policy(&config.shell_path)
                        .map(|value| ("PSExecutionPolicyPreference".into(), value)),
                )
                .chain([
                    ("TERM".into(), "xterm-256color".into()),
                    ("PAGER".into(), "cat".into()),
                    ("GIT_PAGER".into(), "cat".into()),
                    ("PS1".into(), "dsh> ".into()),
                    ("DSH_SHELL".into(), "1".into()),
                    ("DSH_SESSION_ID".into(), spec.session_id),
                    ("DSH_PTY_SESSION_ID".into(), spec.terminal_id.to_string()),
                ])
                .collect();
            let cancellation = spec.signal.clone();
            let terminal = subprocess
                .spawn_terminal(SubprocessTerminalSpawnSpec {
                    argv,
                    cwd: terminal_cwd(cwd),
                    env: Some(env),
                    rows: config.rows,
                    cols: config.cols,
                    grace_ms: config.dispose_grace_ms,
                    signal: Some(spec.signal),
                })
                .await
                .map_err(TerminalBackendSpawnError::spawn)?;
            let mut session = LocalPtySession::new(terminal, config);
            session.execution_context_id = context_id;
            let session = Arc::new(session);
            if let Err(mut error) = session.initialize(None, Some(cancellation)).await {
                if let Err(cleanup) = session.close("User terminal startup failed").await {
                    error.cleanup_error = Some(cleanup);
                }
                return Err(error);
            }
            Ok(session as Arc<dyn TerminalBackendSession>)
        })
    }
}
