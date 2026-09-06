use cordis::Context;
use dsh_pwsh_local::{Config, LocalPwshExecutor};
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use dsh_shell::{ShellExecRequest, ShellExecutor};
use dsh_subprocess_local::LocalSubprocessRuntime;

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
