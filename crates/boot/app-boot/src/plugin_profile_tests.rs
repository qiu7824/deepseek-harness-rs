use super::*;
fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!("plugin-profile-{}", nonce()));
    std::fs::create_dir(&path).unwrap();
    path
}
fn cleanup(path: PathBuf) {
    assert!(
        path.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(path).unwrap();
}
fn plugin(path: &Path, text: &str) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::write(path.join("client.js"), text).unwrap();
}
fn documents(name: &str, version: &str) -> Documents {
    Documents {
        manifest: json!({"dependencies":{name:version},"unrelated":{"keep":true}}),
        entries: vec![json!({"id":name,"name":name,"disabled":false})],
    }
}

#[test]
fn replacement_and_remove_publish_package_and_documents_together() {
    let root = root();
    let profile = Profile::open(&root).unwrap();
    let source = profile.operation_dir().unwrap();
    plugin(&source, "old");
    profile
        .replace(
            documents("test-plugin", "1"),
            Some(("test-plugin", Some(&source))),
        )
        .unwrap();
    let target = package_path(profile.root(), "test-plugin").unwrap();
    assert_eq!(
        std::fs::read_to_string(target.join("client.js")).unwrap(),
        "old"
    );
    plugin(&source, "new");
    profile
        .replace(
            documents("test-plugin", "2"),
            Some(("test-plugin", Some(&source))),
        )
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(target.join("client.js")).unwrap(),
        "new"
    );
    assert_eq!(
        profile.documents().unwrap().manifest["dependencies"]["test-plugin"],
        "2"
    );
    profile
        .replace(
            Documents {
                manifest: json!({"dependencies":{},"unrelated":{"keep":true}}),
                entries: vec![],
            },
            Some(("test-plugin", None)),
        )
        .unwrap();
    assert!(!target.exists());
    assert_eq!(
        profile.documents().unwrap().manifest["unrelated"]["keep"],
        true
    );
    profile.discard_stage(&source).unwrap();
    assert!(!root.join(JOURNAL).exists());
    drop(profile);
    cleanup(root);
}

#[test]
fn malformed_documents_use_good_runtime_snapshot_and_require_explicit_recovery() {
    let root = root();
    let profile = Profile::open(&root).unwrap();
    profile.replace(documents("keep", "1"), None).unwrap();
    std::fs::write(root.join("plugins.json"), "{broken}").unwrap();
    let runtime = read_runtime(&root);
    assert!(runtime.issue.is_some());
    assert_eq!(runtime.documents.entries[0]["id"], "keep");
    assert!(profile.replace(documents("other", "2"), None).is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("plugins.json")).unwrap(),
        "{broken}"
    );
    profile.restore_last_good().unwrap();
    assert_eq!(profile.documents().unwrap().entries[0]["id"], "keep");
    let retained = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".dsh-plugin-rejected-")
        })
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(retained.path().join("plugins.json")).unwrap(),
        "{broken}"
    );
    drop(profile);
    cleanup(root);
}

fn interrupted(profile: &Profile) -> PathBuf {
    profile.replace(documents("demo", "old"), None).unwrap();
    let target = package_path(profile.root(), "demo").unwrap();
    plugin(&target, "old package");
    let operation = profile.operation_dir().unwrap();
    plugin(&operation.join("next"), "new package");
    let next = documents("demo", "new");
    let journal = Journal {
        version: 1,
        operation: operation
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        committed: false,
        package: Some("demo".into()),
        old_package: Some(package_digest(&target).unwrap()),
        new_package: Some(package_digest(&operation.join("next")).unwrap()),
        before_manifest: read_optional(&profile.root.join("package.json")).unwrap(),
        before_entries: read_optional(&profile.root.join("plugins.json")).unwrap(),
        after_manifest: serde_json::to_string_pretty(&next.manifest).unwrap(),
        after_entries: serde_json::to_string_pretty(&next.entries).unwrap(),
        tag: None,
    };
    persist(&profile.root.join(JOURNAL), &journal).unwrap();
    std::fs::rename(&target, operation.join("previous")).unwrap();
    std::fs::rename(operation.join("next"), &target).unwrap();
    atomic(
        &profile.root.join("package.json"),
        journal.after_manifest.as_bytes(),
    )
    .unwrap();
    operation
}

#[test]
fn interrupted_publish_restores_both_documents_and_previous_package() {
    let root = root();
    let profile = Profile::open(&root).unwrap();
    let operation = interrupted(&profile);
    drop(profile);
    let profile = Profile::open(&root).unwrap();
    assert_eq!(
        profile.documents().unwrap().manifest["dependencies"]["demo"],
        "old"
    );
    assert_eq!(
        std::fs::read_to_string(
            package_path(profile.root(), "demo")
                .unwrap()
                .join("client.js")
        )
        .unwrap(),
        "old package"
    );
    assert!(!operation.exists());
    assert!(!root.join(JOURNAL).exists());
    drop(profile);
    cleanup(root);
}

#[test]
fn recovery_preserves_external_edits_and_keeps_evidence() {
    let root = root();
    let profile = Profile::open(&root).unwrap();
    let operation = interrupted(&profile);
    drop(profile);
    let edited = "[{\"id\":\"manual\",\"name\":\"manual\"}]";
    std::fs::write(root.join("plugins.json"), edited).unwrap();
    assert!(Profile::open(&root).is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("plugins.json")).unwrap(),
        edited
    );
    assert!(operation.join("previous").exists());
    assert!(root.join(JOURNAL).exists());
    cleanup(root);
}

#[test]
fn stale_document_update_does_not_overwrite_an_external_edit() {
    let root = root();
    let profile = Profile::open(&root).unwrap();
    profile.replace(documents("demo", "old"), None).unwrap();
    let mut stale = profile.documents().unwrap();
    stale.entries[0]["disabled"] = json!(true);
    let mut external = profile.documents().unwrap().manifest;
    external["manualEdit"] = json!(true);
    let raw = serde_json::to_string(&external).unwrap();
    std::fs::write(root.join("package.json"), &raw).unwrap();
    assert!(
        profile
            .replace(stale, None)
            .unwrap_err()
            .contains("外部修改")
    );
    assert_eq!(
        std::fs::read_to_string(root.join("package.json")).unwrap(),
        raw
    );
    assert!(!root.join(JOURNAL).exists());
    drop(profile);
    cleanup(root);
}

#[test]
fn profile_writers_are_exclusive_and_lock_file_survives_release() {
    let root = root();
    let first = Profile::open(&root).unwrap();
    assert!(Profile::open(&root).is_err());
    drop(first);
    drop(Profile::open(&root).unwrap());
    assert!(root.join(".dsh-plugins.lock").is_file());
    cleanup(root);
}

#[test]
fn profile_boot_uses_verified_manifest_without_rewriting_rejected_json() {
    let home = root();
    let directory = home.join("profiles/web");
    let bundles = crate::profile_templates()["web"]
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    crate::init_profile(&directory, &bundles).unwrap();
    {
        let profile = Profile::open(&directory).unwrap();
        profile.checkpoint().unwrap();
    }
    std::fs::write(directory.join("package.json"), "{rejected}").unwrap();
    let loaded = crate::load_profile("web", &home).unwrap();
    assert!(!loaded.layers.is_empty());
    assert_eq!(
        std::fs::read_to_string(directory.join("package.json")).unwrap(),
        "{rejected}"
    );
    cleanup(home);
}

#[test]
#[ignore = "private child entry for process interruption recovery"]
fn private_interrupted_profile_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("DSH_PROFILE_RECOVERY_TEST").unwrap());
    let profile = Profile::open(&root).unwrap();
    interrupted(&profile);
    std::fs::write(root.join("ready"), b"ready").unwrap();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[test]
fn process_exit_releases_ownership_and_restores_uncommitted_package() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = root();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "plugin_profile::tests::private_interrupted_profile_worker",
            "--ignored",
            "--nocapture",
        ])
        .env("DSH_PROFILE_RECOVERY_TEST", &root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join("ready").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "worker never reached the interrupted publication"
        );
        assert!(child.0.try_wait().unwrap().is_none());
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(Profile::open(&root).is_err());
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let profile = Profile::open(&root).unwrap();
    assert_eq!(
        profile.documents().unwrap().manifest["dependencies"]["demo"],
        "old"
    );
    assert_eq!(
        std::fs::read_to_string(
            package_path(profile.root(), "demo")
                .unwrap()
                .join("client.js")
        )
        .unwrap(),
        "old package"
    );
    drop(profile);
    cleanup(root);
}
