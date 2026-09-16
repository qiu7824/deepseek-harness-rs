use cordis::Context;
use dsh_pwsh_local::{Config, LocalPwshExecutor};
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use dsh_shell::{ShellExecRequest, ShellExecutor};
use dsh_subprocess_local::LocalSubprocessRuntime;

#[tokio::test]
#[cfg(windows)]
async fn redirected_python_keeps_chinese_stdout_and_stderr_in_utf8() {
    let ctx = Context::root();
    let _processes = LocalSubprocessRuntime::install(&ctx);
    let shell = LocalPwshExecutor::install(&ctx, Config {
        pwsh_path: Some(std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe").to_string_lossy().into_owned()),
        ..Default::default()
    });
    let result = shell.run(shell.resolve(ShellExecRequest::new(
        "python -c \"import sys; print('中文标题'); sys.stderr.write('诊断信息')\""
    ))).await.unwrap();
    assert_eq!(result.exit_code, Some(0), "{}", result.stderr.text);
    assert!(result.stdout.text.contains("中文标题"), "{}", result.stdout.text);
    assert!(result.stderr.text.contains("诊断信息"), "{}", result.stderr.text);
}

#[tokio::test]
#[cfg(windows)]
async fn optional_native_probes_preserve_diagnostics_without_weakening_later_failures() {
    let system = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut executables = vec![system];
    if let Some(core) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("pwsh.exe"))
        .find(|path| path.is_file())
    {
        executables.push(core);
    }
    for executable in executables {
        let ctx = Context::root();
        let _processes = LocalSubprocessRuntime::install(&ctx);
        let shell = LocalPwshExecutor::install(
            &ctx,
            Config {
                pwsh_path: Some(executable.to_string_lossy().into_owned()),
                ..Default::default()
            },
        );
        let probe = "$probe = Invoke-DshNativeProbe -FilePath 'cmd.exe' -ArgumentList @('/d', '/c', 'echo native-probe-diagnostic 1>&2 & exit /b 7'); if ($probe.ExitCode -ne 7 -or $probe.Output -notmatch 'native-probe-diagnostic') { throw 'native diagnostic lost' }; if ($ErrorActionPreference -ne 'Stop' -or $global:LASTEXITCODE -ne 0) { throw 'probe changed caller policy' }; Write-Output $probe.Output; Write-Output 'PROBE_HANDLED'";
        let result = shell
            .run(shell.resolve(ShellExecRequest::new(probe)))
            .await
            .unwrap();
        assert_eq!(
            result.exit_code,
            Some(0),
            "{executable:?}: {}",
            result.stderr.text
        );
        assert!(result.stdout.text.contains("PROBE_HANDLED"));
        let bounded = shell.run(shell.resolve(ShellExecRequest::new("$probe = Invoke-DshNativeProbe -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-NonInteractive','-Command','[Console]::WriteLine([string]::new([char]120,100000))'); if (-not $probe.Truncated -or $probe.Output.Length -gt 65536 -or $probe.ExitCode -ne 0) { throw 'unbounded diagnostic output' }; Write-Output 'BOUNDED_PROBE'"))).await.unwrap();
        assert_eq!(bounded.exit_code, Some(0), "{}", bounded.stderr.text);
        assert!(bounded.stdout.text.contains("BOUNDED_PROBE"));
        let command = format!(
            "{probe}; Get-Item -LiteralPath 'DSH_MISSING_AFTER_PROBE_7721'; Write-Output 'SHOULD_NOT_CONTINUE'"
        );
        let result = shell
            .run(shell.resolve(ShellExecRequest::new(command)))
            .await
            .unwrap();
        assert_ne!(result.exit_code, Some(0), "{executable:?}");
        assert!(!result.stdout.text.contains("SHOULD_NOT_CONTINUE"));
    }
}

#[tokio::test]
#[cfg(windows)]
async fn dropping_a_foreground_call_terminates_its_process_tree() {
    use dsh_subprocess::*;
    use futures::future::BoxFuture;
    use std::sync::{Arc, Mutex};
    struct Tracking {
        inner: Arc<LocalSubprocessRuntime>,
        spawned: Mutex<Option<tokio::sync::oneshot::Sender<Arc<dyn SubprocessHandle>>>>,
    }
    impl SubprocessRuntime for Tracking {
        fn resolve_executable(
            &self,
            command: &str,
            env: Option<&[(String, String)]>,
            signal: Option<SubprocessAbort>,
        ) -> BoxFuture<'static, Result<String, String>> {
            self.inner.resolve_executable(command, env, signal)
        }
        fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
            let handle = self.inner.spawn(spec)?;
            if let Some(sender) = self.spawned.lock().unwrap().take() {
                let _ = sender.send(handle.clone());
            }
            Ok(handle)
        }
        fn spawn_terminal(
            &self,
            spec: SubprocessTerminalSpawnSpec,
        ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
            self.inner.spawn_terminal(spec)
        }
    }
    let ctx = Context::root();
    let (spawned, received) = tokio::sync::oneshot::channel();
    let tracking = Arc::new(Tracking {
        inner: LocalSubprocessRuntime::new(),
        spawned: Mutex::new(Some(spawned)),
    });
    ctx.register_service(tracking.clone() as Arc<dyn SubprocessRuntime>);
    let shell = LocalPwshExecutor::install(
        &ctx,
        Config {
            grace_ms: Some(20),
            ..Default::default()
        },
    );
    let first_shell = shell.clone();
    let operation = tokio::spawn(async move {
        let shell = first_shell;
        shell
            .run(shell.resolve(ShellExecRequest::new("Start-Sleep -Seconds 60")))
            .await
    });
    let handle = received.await.unwrap();
    operation.abort();
    assert!(matches!(operation.await, Err(error) if error.is_cancelled()));
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            handle.wait_for_exit(None)
        )
        .await
        .unwrap(),
        "dropping the command future must release the real Windows process tree"
    );
    let (spawned, received) = tokio::sync::oneshot::channel();
    *tracking.spawned.lock().unwrap() = Some(spawned);
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = cancelled.clone();
    let mut request = ShellExecRequest::new("Start-Sleep -Seconds 60");
    request.signal = Some(Arc::new(move || {
        signal.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let operation = tokio::spawn(async move { shell.run(shell.resolve(request)).await });
    let handle = received.await.unwrap();
    cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.aborted);
    assert!(
        !result.timed_out,
        "user cancellation must not be reported as a timeout"
    );
    assert!(handle.wait_for_exit(None).await);
}

#[tokio::test]
#[cfg(windows)]
async fn selected_system_powershell_loads_its_own_builtin_modules() {
    let ctx = Context::root();
    let _processes = LocalSubprocessRuntime::install(&ctx);
    let system = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let shell = LocalPwshExecutor::install(
        &ctx,
        Config {
            pwsh_path: Some(system.to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    let result = shell
        .run(shell.resolve(ShellExecRequest::new(
            "Write-Output 'SYSTEM_CMDLET_OK'; Write-Output $PSVersionTable.PSVersion.Major",
        )))
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "{}", result.stderr.text);
    assert!(result.stdout.text.contains("SYSTEM_CMDLET_OK"));
    assert!(result.stdout.text.lines().any(|line| line.trim() == "5"));
}

#[tokio::test]
async fn request_workspace_precedes_host_cwd_and_explicit_workdir_wins() {
    let ctx = Context::root();
    let _processes = LocalSubprocessRuntime::install(&ctx);
    let shell = LocalPwshExecutor::install(
        &ctx,
        Config {
            cwd: Some("host-install".into()),
            max_timeout_ms: Some(1000),
            ..Default::default()
        },
    );
    let mut request = ShellExecRequest::new("pwd");
    request.sandbox_policy = Some(SandboxExecutionPolicy {
        read_only_roots: Vec::new(),
        mode: SandboxMode::DangerFullAccess,
        workspace_root: "project".into(),
        session_id: None,
    });
    request.timeout_ms = Some(999999);
    assert_eq!(shell.resolve(request.clone()).workdir, "project");
    assert_eq!(shell.resolve(request.clone()).timeout_ms, 1000);
    request.workdir = Some("explicit".into());
    assert_eq!(shell.resolve(request).workdir, "explicit");
}

#[tokio::test]
#[cfg(windows)]
async fn foreground_wait_is_bounded_and_nonzero_and_stderr_are_preserved() {
    let ctx = Context::root();
    let _processes = LocalSubprocessRuntime::install(&ctx);
    let shell = LocalPwshExecutor::install(
        &ctx,
        Config {
            grace_ms: Some(20),
            ..Default::default()
        },
    );
    let result = shell
        .run(shell.resolve(ShellExecRequest::new(
            "[Console]::Error.WriteLine('diagnostic'); exit 7",
        )))
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(7));
    assert!(result.stderr.text.contains("diagnostic"));
    assert!(!result.timed_out);
    let failed=shell.run(shell.resolve(ShellExecRequest::new("Get-Item -LiteralPath 'DSH_NONEXISTENT_INPUT_74e6'; Write-Output 'SHOULD_NOT_CONTINUE'"))).await.unwrap();
    assert_ne!(failed.exit_code, Some(0));
    assert!(!failed.stdout.text.contains("SHOULD_NOT_CONTINUE"));
    let mut request = ShellExecRequest::new("Start-Sleep -Seconds 60");
    request.timeout_ms = Some(500);
    let started = std::time::Instant::now();
    let result = shell.run(shell.resolve(request)).await.unwrap();
    assert!(result.timed_out);
    assert!(started.elapsed() < std::time::Duration::from_secs(8));
}

#[tokio::test]
async fn missing_background_executable_reports_failure_instead_of_cancellation() {
    let ctx = Context::root();
    let _processes = LocalSubprocessRuntime::install(&ctx);
    let shell = LocalPwshExecutor::install(
        &ctx,
        Config {
            pwsh_path: Some(
                std::env::temp_dir()
                    .join(format!("dsh-missing-shell-{}.exe", std::process::id()))
                    .to_string_lossy()
                    .into_owned(),
            ),
            ..Default::default()
        },
    );
    let process = shell.start(shell.resolve(ShellExecRequest::new("exit 0")));
    process.done().await;
    assert_eq!(process.status(), dsh_shell::ShellProcessStatus::Completed);
    assert_eq!(process.exit_code(), Some(127));
    assert!(!process.read_output().delta.is_empty());
}
