use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bundled_auto_review_defaults_off_and_plugin_disable_migrates_live_sessions() {
    let root = std::env::temp_dir().join(format!("host-auto-plugin-{}", uuid::Uuid::new_v4()));
    let ctx = Context::root();
    let host = compose_persistent_host_at(&ctx, &root, Some("web")).unwrap();
    let permissions = ctx
        .get_typed::<Arc<dsh_permission_presets::PermissionPresetService>>(
            "permissionPresets",
            false,
        )
        .unwrap();
    assert!(!permissions.names().contains(&"auto"));
    let path = root.join("profiles/web/plugins.json");
    let entries: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "dsh-auto-review")
            .unwrap()["disabled"],
        true
    );
    let toggle = |enabled| {
        host.api_proxy.apply_plugin_enablement(
            "dsh-auto-review".into(),
            enabled,
            Default::default(),
            None,
            Arc::new(|_| {}),
            None,
        )
    };
    toggle(true).await.unwrap();
    assert!(permissions.names().contains(&"auto"));
    let session = host
        .sessions
        .create(
            &ctx,
            Some(dsh_session::session_id("auto-plugin-session")),
            None,
        )
        .await
        .unwrap();
    permissions.set(&session, "auto").unwrap();
    let knobs = || {
        session.with_events(|e| {
            e.iter()
                .filter(|e| matches!(e.type_.as_str(), "sandbox/mode" | "approval/policy"))
                .count()
        })
    };
    let before = knobs();
    toggle(false).await.unwrap();
    assert_eq!(
        session.with_events(|e| permissions.current(e)),
        "danger-full-access"
    );
    assert_eq!(knobs(), before);
    assert!(!permissions.names().contains(&"auto"));
    toggle(true).await.unwrap();
    assert_eq!(
        session.with_events(|e| permissions.current(e)),
        "danger-full-access"
    );
    toggle(false).await.unwrap();
    host.shutdown().await.unwrap();
    drop(host);
    drop(session);
    drop(permissions);
    drop(ctx);
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
