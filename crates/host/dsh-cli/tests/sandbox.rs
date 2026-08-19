#[cfg(windows)]
#[test]
fn dsh_binary_hosts_the_internal_windows_sandbox_runner() {
    let root = std::env::temp_dir().join(format!(
        "dsh-cli-sandbox-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let workspace = root.join("workspace");
    let inside = workspace.join("inside.txt");
    let outside = root.join("outside.txt");
    std::fs::create_dir_all(&workspace).expect("create workspace");

    let quote = |path: &std::path::Path| path.to_string_lossy().replace('’', "''");
    let command = format!(
        "$inside=$false; try {{ Set-Content -LiteralPath '{}' -Value inside -ErrorAction Stop; $inside=$true }} catch {{}}; try {{ Set-Content -LiteralPath '{}' -Value outside -ErrorAction Stop }} catch {{}}; if ($inside -and -not (Test-Path -LiteralPath '{}')) {{ exit 0 }} else {{ exit 9 }}",
        quote(&inside),
        quote(&outside),
        quote(&outside),
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_dsh"))
        .args([
            "__dsh-sandbox-windows",
            "--mode",
            "workspace-write",
            "--workspace",
        ])
        .arg(&workspace)
        .args([
            "--",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &command,
        ])
        .output()
        .expect("run dsh internal sandbox runner");

    let inside_exists = inside.exists();
    let outside_exists = outside.exists();
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(inside_exists, "sandbox did not write inside its workspace");
    assert!(!outside_exists, "sandbox escaped its workspace");
}
