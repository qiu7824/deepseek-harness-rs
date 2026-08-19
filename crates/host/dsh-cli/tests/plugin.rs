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
fn plugin_cli_forwards_direct_argv_cwd_and_exit_code() {
    let root = temp_root();
    let home = root.join("home");
    let profile = home.join("profiles").join("web");
    std::fs::create_dir_all(&profile).expect("profile dir");
    std::fs::write(profile.join("package.json"), "{}\n").expect("manifest");
    let record = root.join("record.json");
    let helper = root.join("pnpm_fixture.py");
    std::fs::write(
        &helper,
        "import json, os, sys\njson.dump({'argv': sys.argv[1:], 'cwd': os.getcwd()}, open(os.environ['DSH_PLUGIN_RECORD'], 'w', encoding='utf-8'))\nsys.exit(7)\n",
    )
    .expect("helper");

    let output = Command::new(env!("CARGO_BIN_EXE_dsh"))
        .arg("plugin")
        .arg("--profile")
        .arg("web")
        .arg(&helper)
        .arg("add")
        .arg("pkg;not-shell")
        .arg("$(not-expanded)")
        .env("DSH_HOME", &home)
        .env("DSH_PNPM_BIN", "python")
        .env("DSH_PLUGIN_RECORD", &record)
        .env("PYTHONUTF8", "1")
        .env("PYTHONPATH", "")
        .env("PYTHONSTARTUP", "")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUNBUFFERED", "1")
        .env("DSH_PNPM_WRAPPER", &helper)
        .output()
        .expect("run dsh plugin");

    // DSH_PNPM_BIN receives argv directly; point it at a tiny executable
    // wrapper by using Python's `-c`-free script path as the first forwarded
    // argument below. The launcher's raw pnpm arguments remain individually
    // addressable and are never joined into a shell command.
    assert_eq!(output.status.code(), Some(7));
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).expect("recorded plugin invocation"))
            .expect("record JSON");
    assert_eq!(
        value["argv"],
        serde_json::json!(["add", "pkg;not-shell", "$(not-expanded)"])
    );
    assert_eq!(
        std::path::PathBuf::from(value["cwd"].as_str().expect("cwd")),
        profile
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("not implemented"));

    let _ = std::fs::remove_dir_all(root);
}
