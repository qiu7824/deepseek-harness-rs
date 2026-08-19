use cordis::Context;
use dsh_pwsh_local::{Config, ENCODING_PREAMBLE, LocalPwshExecutor};
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use dsh_shell::{ShellExecRequest, ShellExecutor};
use dsh_subprocess_local::LocalSubprocessRuntime;

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[tokio::test(flavor = "current_thread")]
async fn foreground_runs_real_pwsh_with_utf8_and_a_controlled_environment() {
    let suffix = std::process::id();
    let ambient_dsh = format!("DSH_PWSH_AMBIENT_{suffix}");
    let ambient_secret = format!("PWSH_AMBIENT_TOKEN_{suffix}");

    // SAFETY: these process-local probe names are unique to this test and are
    // removed before it returns.
    unsafe {
        std::env::set_var(&ambient_dsh, "ambient-dsh");
        std::env::set_var(&ambient_secret, "ambient-secret");
    }

    let ctx = Context::root();
    LocalSubprocessRuntime::install(&ctx);
    let pwsh = LocalPwshExecutor::install(
        &ctx,
        Config {
            grace_ms: Some(200),
            ..Default::default()
        },
    );

    let command = format!(
        "Write-Output '你好'; Write-Output \"[$env:{ambient_dsh}][$env:{ambient_secret}]\""
    );
    let scrubbed = pwsh
        .run(pwsh.resolve(ShellExecRequest::new(command)))
        .await
        .expect("real PowerShell run");
    assert_eq!(scrubbed.exit_code, Some(0));
    assert_eq!(lf(&scrubbed.stdout.text), "你好\n[][]\n");
    assert!(ENCODING_PREAMBLE.contains("[Console]::OutputEncoding"));
    assert!(ENCODING_PREAMBLE.contains("$OutputEncoding"));

    let command = format!("Write-Output \"[$env:{ambient_dsh}][$env:{ambient_secret}]\"");
    let mut request = ShellExecRequest::new(command);
    request.env = Some(vec![(
        ambient_secret.clone(),
        "explicit-secret".to_string(),
    )]);
    request.dsh_env = Some(vec![(ambient_dsh.clone(), "explicit-dsh".to_string())]);
    let restored = pwsh
        .run(pwsh.resolve(request))
        .await
        .expect("real PowerShell run with explicit environment");
    assert_eq!(
        lf(&restored.stdout.text),
        "[explicit-dsh][explicit-secret]\n"
    );

    unsafe {
        std::env::remove_var(&ambient_dsh);
        std::env::remove_var(&ambient_secret);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_write_never_executes_without_a_windows_sandbox_backend() {
    let root = std::env::temp_dir().join(format!(
        "dsh-pwsh-sandbox-red-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("root");
    let marker = root.join("must-not-exist.txt");
    let ctx = Context::root();
    LocalSubprocessRuntime::install(&ctx);
    let pwsh = LocalPwshExecutor::install(&ctx, Config::default());
    let marker_literal = marker.to_string_lossy().replace('\'', "''");
    let mut request = ShellExecRequest::new(format!(
        "Set-Content -LiteralPath '{marker_literal}' -Value unsafe"
    ));
    request.sandbox_policy = Some(SandboxExecutionPolicy {
        mode: SandboxMode::WorkspaceWrite,
        workspace_root: root.to_string_lossy().into_owned(),
        session_id: None,
    });
    let result = pwsh.run(pwsh.resolve(request)).await;
    assert!(result.is_err(), "constrained command ran unconfined");
    assert!(
        !marker.exists(),
        "constrained command wrote without an enforcing backend"
    );
    let _ = std::fs::remove_dir_all(root);
}
