use serde_json::{Value, json};

#[test]
fn remove_and_recover_use_the_real_cli_and_publish_operation_receipts() {
    let root = std::env::temp_dir().join(format!("plugin-recovery-cli-{}", uuid::Uuid::new_v4()));
    let profile = root.join("profiles/web");
    let plugin = profile.join("node_modules/demo");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        profile.join("package.json"),
        json!({"private":true,"dependencies":{"demo":"1"},"kept":"unchanged"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        profile.join("plugins.json"),
        json!([{"id":"demo","name":"demo","disabled":true}]).to_string(),
    )
    .unwrap();
    std::fs::write(plugin.join("client.js"), "window.fixture=true;").unwrap();
    let run = |action: &str, spec: Option<&str>| {
        let id = uuid::Uuid::new_v4().to_string();
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_dsh"));
        command
            .args(["plugin", "--profile", "web", action])
            .env("DSH_HOME", &root)
            .env("DSH_PLUGIN_OPERATION_ID", &id);
        if let Some(spec) = spec {
            command.arg(spec);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: Value = serde_json::from_slice(
            &std::fs::read(profile.join(".dsh-plugin-last-operation.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["operationId"], id);
        assert_eq!(receipt["committed"], true);
    };
    run("remove", Some("demo"));
    assert!(!plugin.exists());
    std::fs::write(profile.join("plugins.json"), "{broken configuration}").unwrap();
    run("recover", None);
    let entries: Value =
        serde_json::from_slice(&std::fs::read(profile.join("plugins.json")).unwrap()).unwrap();
    assert_eq!(entries, json!([]));
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(profile.join("package.json")).unwrap()).unwrap();
    assert_eq!(manifest["kept"], "unchanged");
    assert!(
        std::fs::read_dir(&profile)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".dsh-plugin-rejected-"))
    );
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
