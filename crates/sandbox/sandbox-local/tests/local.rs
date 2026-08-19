use dsh_sandbox::{
    ConfinedSandboxMode, SANDBOX_UNAVAILABLE, SandboxExecutionPolicy, SandboxMode, SandboxPolicy,
    SandboxProvider,
};
use dsh_sandbox_local::{Config, LocalSandboxProvider, SandboxLaunch};

#[cfg(windows)]
static RUNNER_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn policy(mode: SandboxMode) -> SandboxExecutionPolicy {
    SandboxExecutionPolicy {
        mode,
        workspace_root: "C:\\workspace".to_string(),
        session_id: None,
    }
}

#[cfg(windows)]
#[test]
fn windows_never_runs_a_constrained_policy_without_the_restricted_token_backend() {
    let _lock = RUNNER_ENV_LOCK.lock().expect("runner env lock");
    let previous_runner = std::env::var_os("DSH_SANDBOX_WINDOWS_RUNNER");
    let missing = std::env::temp_dir().join(format!(
        "missing-dsh-sandbox-runner-{}-{}.exe",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let sibling = std::env::current_exe()
        .expect("current test executable")
        .parent()
        .expect("test executable parent")
        .join("dsh-sandbox-windows.exe");
    assert!(
        !sibling.exists(),
        "test must not replace a real sibling runner"
    );
    std::fs::write(&sibling, b"fixture").expect("create discoverable sibling fixture");
    unsafe {
        std::env::set_var("DSH_SANDBOX_WINDOWS_RUNNER", &missing);
    }
    let provider = LocalSandboxProvider::new(Config {
        platform: Some("win32".to_string()),
    });
    let argv = vec![
        "cmd.exe".to_string(),
        "/c".to_string(),
        "echo ok".to_string(),
    ];
    let constrained: Vec<_> = [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite]
        .into_iter()
        .map(|mode| provider.wrap_execution(&argv, &policy(mode)))
        .collect();
    let unrestricted = provider.wrap_execution(&argv, &policy(SandboxMode::DangerFullAccess));
    unsafe {
        match previous_runner {
            Some(value) => std::env::set_var("DSH_SANDBOX_WINDOWS_RUNNER", value),
            None => std::env::remove_var("DSH_SANDBOX_WINDOWS_RUNNER"),
        }
    }
    std::fs::remove_file(&sibling).expect("remove sibling fixture");

    for result in constrained {
        let error = result.expect_err("explicit missing runner must fail closed without fallback");
        assert_eq!(error.code(), SANDBOX_UNAVAILABLE);
    }
    assert_eq!(
        unrestricted.expect("explicit unrestricted policy"),
        SandboxLaunch::Direct(argv)
    );
}

#[cfg(windows)]
#[test]
fn windows_restricted_runner_enforces_read_only_and_workspace_write() {
    let _lock = RUNNER_ENV_LOCK.lock().expect("runner env lock");
    let previous_runner = std::env::var_os("DSH_SANDBOX_WINDOWS_RUNNER");
    let runner = std::env::var_os("CARGO_BIN_EXE_dsh-sandbox-windows")
        .map(std::path::PathBuf::from)
        .or_else(|| option_env!("CARGO_BIN_EXE_dsh-sandbox-windows").map(std::path::PathBuf::from))
        .unwrap_or_else(|| {
            std::env::current_exe()
                .expect("current test executable")
                .parent()
                .and_then(std::path::Path::parent)
                .expect("target/debug")
                .join("dsh-sandbox-windows.exe")
        });
    unsafe {
        std::env::set_var("DSH_SANDBOX_WINDOWS_RUNNER", &runner);
    }

    let root = std::env::temp_dir().join(format!(
        "dsh-sandbox-windows-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let workspace = root.join("workspace");
    let outside = root.join("outside.txt");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let provider = LocalSandboxProvider::new(Config {
        platform: Some("win32".to_string()),
    });

    for (mode, inside_allowed) in [
        (ConfinedSandboxMode::WorkspaceWrite, true),
        (ConfinedSandboxMode::ReadOnly, false),
    ] {
        let inside = workspace.join(format!("inside-{}.txt", mode.as_str()));
        let command = format!(
            "$ErrorActionPreference='Stop'; \
             try {{ Set-Content -LiteralPath '{}' -Value inside; Write-Output inside-ok }} \
             catch {{ Write-Output inside-denied }}; \
             try {{ Set-Content -LiteralPath '{}' -Value outside; Write-Output outside-ok }} \
             catch {{ Write-Output outside-denied }}",
            inside.to_string_lossy().replace('\'', "''"),
            outside.to_string_lossy().replace('\'', "''")
        );
        let argv = vec![
            r"C:\Program Files\PowerShell\7\pwsh.exe".to_string(),
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            command,
        ];
        let confined = provider
            .confine(
                &argv,
                &SandboxPolicy {
                    mode,
                    workspace_root: workspace.to_string_lossy().into_owned(),
                    session_id: None,
                },
            )
            .expect("Windows restricted runner");
        assert_eq!(confined.enforcement.as_str(), "full");
        let output = std::process::Command::new(&confined.argv[0])
            .args(&confined.argv[1..])
            .current_dir(&workspace)
            .output()
            .expect("run Windows restricted process");
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(inside.exists(), inside_allowed, "mode={}", mode.as_str());
        assert!(
            !outside.exists(),
            "outside write escaped mode={}",
            mode.as_str()
        );
    }

    unsafe {
        match previous_runner {
            Some(value) => std::env::set_var("DSH_SANDBOX_WINDOWS_RUNNER", value),
            None => std::env::remove_var("DSH_SANDBOX_WINDOWS_RUNNER"),
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn linux_and_macos_build_their_native_profile_dialects() {
    let argv = vec!["sh".to_string(), "-c".to_string(), "echo ok".to_string()];
    let policy = SandboxPolicy {
        mode: ConfinedSandboxMode::WorkspaceWrite,
        workspace_root: "/work/a\"b".to_string(),
        session_id: None,
    };

    let linux = LocalSandboxProvider::new(Config {
        platform: Some("linux".to_string()),
    })
    .confine(&argv, &policy)
    .expect("linux profile");
    assert_eq!(linux.argv.first().map(String::as_str), Some("bwrap"));
    assert!(
        linux
            .argv
            .windows(3)
            .any(|items| items == ["--ro-bind", "/", "/"])
    );
    assert!(
        linux
            .argv
            .windows(3)
            .any(|items| items == ["--bind", "/work/a\"b", "/work/a\"b"])
    );
    assert!(linux.argv.ends_with(&argv));

    let mac = LocalSandboxProvider::new(Config {
        platform: Some("darwin".to_string()),
    })
    .confine(&argv, &policy)
    .expect("seatbelt profile");
    assert_eq!(mac.argv.first().map(String::as_str), Some("sandbox-exec"));
    let profile = mac.argv.get(2).expect("SBPL profile");
    assert!(profile.contains("(deny file-write*)"));
    assert!(profile.contains("/work/a\\\"b"), "{profile}");
    assert!(mac.argv.ends_with(&argv));
}
