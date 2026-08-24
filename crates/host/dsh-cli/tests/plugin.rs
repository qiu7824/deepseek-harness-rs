use std::process::Command;

fn temp_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "dsh-plugin-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

#[test]
fn plugin_cli_uses_the_native_profile_installer_without_node_or_pnpm() {
    let root = temp_root();
    let home = root.join("home");
    let profile = home.join("profiles").join("web");
    std::fs::create_dir_all(&profile).expect("profile dir");
    std::fs::write(
        profile.join("package.json"),
        r#"{"dependencies":{"existing":"github:owner/repo#0123456789abcdef0123456789abcdef01234567"}}"#,
    )
    .expect("manifest");

    let list = Command::new(env!("CARGO_BIN_EXE_dsh"))
        .args(["plugin", "--profile", "web", "list"])
        .env("DSH_HOME", &home)
        .env("PATH", "")
        .output()
        .expect("run native plugin list");
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&list.stdout).trim(),
        "existing\tgithub:owner/repo#0123456789abcdef0123456789abcdef01234567"
    );

    let rejected = Command::new(env!("CARGO_BIN_EXE_dsh"))
        .args(["plugin", "--profile", "web", "add", "../unsafe"])
        .env("DSH_HOME", &home)
        .env("PATH", "")
        .output()
        .expect("run rejected plugin add");
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("only github:"));

    let _ = std::fs::remove_dir_all(root);
}
