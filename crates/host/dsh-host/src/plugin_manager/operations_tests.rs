use super::*;
use dsh_app_boot::plugin_profile::{Documents, OperationTag};

struct Noop;
#[async_trait::async_trait]
impl cordis::Plugin for Noop {
    async fn apply(&self, _: &Context, _: cordis::ArcValue) -> Result<(), cordis::PluginError> {
        Ok(())
    }
}

#[tokio::test]
async fn enablement_waits_for_browser_ack_and_legacy_rpc_uses_the_same_operation_log() {
    let root = root();
    let ctx = Context::root();
    let loader = dsh_cordis_loader::LoaderService::new(&ctx).await;
    loader.core.register("demo", Arc::new(Noop));
    ctx.register_service(loader.clone());
    loader
        .tree
        .create(
            dsh_cordis_loader::EntryOptions {
                id: "demo".into(),
                name: "demo".into(),
                disabled: Some(json!(true)),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    dsh_host_plugin_inventory::PluginInventoryGateway::install(&ctx).unwrap();
    let api = ApiProxyService::install(
        &ctx,
        dsh_host_apiproxy::proxy::ApiProxyDefaults {
            dsh_home: root.to_string_lossy().into_owned(),
            plugins_document: Some(root.join("profiles/web/plugins.json")),
            ..Default::default()
        },
    );
    let runtime = dsh_subprocess_local::LocalSubprocessRuntime::install(&ctx);
    let manager = Manager::install(&ctx, root.clone(), "web".into(), runtime, api);
    let wait_client = |manager: Arc<Manager>, id: String| async move {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let status = manager
                    .dispatch(json!({"action":"status","operationId":id}))
                    .unwrap();
                if status["operation"]["phase"] == "awaiting-client" {
                    break;
                }
                assert!(
                    !terminal(status["operation"]["phase"].as_str().unwrap()),
                    "{status}"
                );
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    };
    let start = manager
        .dispatch(json!({"action":"enable","spec":"demo","clientAck":true}))
        .unwrap();
    let id = start["operation"]["operationId"]
        .as_str()
        .unwrap()
        .to_string();
    wait_client(manager.clone(), id.clone()).await;
    assert!(!loader.tree.resolve("demo").unwrap().disabled().unwrap());
    let raw: Value =
        serde_json::from_slice(&std::fs::read(root.join("profiles/web/plugins.json")).unwrap())
            .unwrap();
    assert_eq!(raw[0]["disabled"], true);
    manager
        .dispatch(json!({"action":"client-result","operationId":id,"ok":true}))
        .unwrap();
    finish(&manager).await;
    let status = manager.dispatch(json!({"action":"status"})).unwrap();
    assert_eq!(status["operation"]["phase"], "succeeded");
    assert_eq!(status["operation"]["restartRequired"], false);
    assert!(
        status["operation"]["log"]
            .as_str()
            .unwrap()
            .contains("提交")
    );
    let start = manager
        .dispatch(json!({"action":"disable","spec":"demo","clientAck":true}))
        .unwrap();
    let id = start["operation"]["operationId"]
        .as_str()
        .unwrap()
        .to_string();
    wait_client(manager.clone(), id.clone()).await;
    manager
        .dispatch(json!({"action":"cancel","operationId":id}))
        .unwrap();
    finish(&manager).await;
    assert!(!loader.tree.resolve("demo").unwrap().disabled().unwrap());
    assert_eq!(
        manager.dispatch(json!({"action":"status"})).unwrap()["operation"]["phase"],
        "cancelled"
    );
    let control = ctx
        .get_typed::<Arc<PluginOperationControl>>("pluginOperationControl", false)
        .unwrap();
    let result = (control.run)("demo".into(), false, AbortSignal::new())
        .await
        .unwrap();
    assert!(!result.entry.enabled);
    assert_eq!(
        manager.dispatch(json!({"action":"status"})).unwrap()["operation"]["action"],
        "disable"
    );
    for disposer in ctx.fiber.disposables.clear() {
        disposer().await;
    }
    cleanup(root);
}
fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("plugin-operations-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let profile = Profile::open(&root.join("profiles/web")).unwrap();
    profile
        .replace(
            Documents {
                manifest: json!({"dependencies":{"demo":"1"}}),
                entries: vec![json!({"id":"demo","name":"demo","disabled":true})],
            },
            None,
        )
        .unwrap();
    root
}
fn cleanup(root: PathBuf) {
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
async fn finish(manager: &Manager) {
    let active = manager.state.lock().active.clone();
    if let Some(active) = active {
        let mut done = active.done.subscribe();
        tokio::time::timeout(Duration::from_secs(3), done.wait_for(|done| *done))
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn cancellation_is_observable_and_a_second_operation_is_not_dispatched() {
    let root = root();
    let entered = Arc::new(tokio::sync::Notify::new());
    let called = entered.clone();
    let manager = Manager::with_runner(
        root.clone(),
        "web".into(),
        Arc::new(move |operation| {
            let called = called.clone();
            Box::pin(async move {
                called.notify_one();
                while !operation.cancelled.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                Err("cancelled before publication".into())
            })
        }),
    );
    let started = manager
        .dispatch(json!({"action":"remove","spec":"demo"}))
        .unwrap();
    entered.notified().await;
    assert!(
        manager
            .dispatch(json!({"action":"remove","spec":"demo"}))
            .is_err()
    );
    let id = started["operation"]["operationId"].as_str().unwrap();
    assert_eq!(
        manager
            .dispatch(json!({"action":"cancel","operationId":id}))
            .unwrap()["operation"]["phase"],
        "cancelling"
    );
    finish(&manager).await;
    let status = manager
        .dispatch(json!({"action":"status","operationId":id}))
        .unwrap();
    assert_eq!(status["operation"]["phase"], "cancelled");
    assert_eq!(status["operation"]["effects"], "rolled-back-or-unchanged");
    assert_eq!(
        Profile::open(&root.join("profiles/web"))
            .unwrap()
            .documents()
            .unwrap()
            .entries[0]["disabled"],
        true
    );
    drop(manager);
    cleanup(root);
}

#[tokio::test]
async fn a_committed_receipt_wins_over_late_cancellation_and_survives_restart() {
    let root = root();
    let profile_root = root.join("profiles/web");
    let entered = Arc::new(tokio::sync::Notify::new());
    let called = entered.clone();
    let manager = Manager::with_runner(
        root.clone(),
        "web".into(),
        Arc::new(move |operation| {
            let root = profile_root.clone();
            let called = called.clone();
            Box::pin(async move {
                {
                    let state = operation.state.lock().clone();
                    let mut profile = Profile::open(&root)?;
                    profile.set_operation(OperationTag {
                        operation_id: state.operation_id,
                        action: state.action,
                        spec: state.spec,
                    })?;
                    profile.replace(
                        Documents {
                            manifest: json!({"dependencies":{}}),
                            entries: vec![],
                        },
                        Some(("demo", None)),
                    )?;
                }
                called.notify_one();
                while !operation.cancelled.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                Err("caller cancelled after commit".into())
            })
        }),
    );
    let started = manager
        .dispatch(json!({"action":"remove","spec":"demo"}))
        .unwrap();
    entered.notified().await;
    let id = started["operation"]["operationId"].as_str().unwrap();
    manager
        .dispatch(json!({"action":"cancel","operationId":id}))
        .unwrap();
    finish(&manager).await;
    assert_eq!(
        manager.dispatch(json!({"action":"status"})).unwrap()["operation"]["phase"],
        "succeeded"
    );
    let operation = manager.state.lock().history.front().unwrap().clone();
    {
        let mut state = operation.state.lock();
        state.phase = "running".into();
        manager.persist(&state).unwrap();
    }
    drop(manager);
    let restored = Manager::with_runner(
        root.clone(),
        "web".into(),
        Arc::new(|_| Box::pin(async { panic!("cold restore must not rerun an operation") })),
    );
    let status = restored.dispatch(json!({"action":"status"})).unwrap();
    assert_eq!(status["operation"]["phase"], "succeeded");
    assert_eq!(status["operation"]["effects"], "committed");
    drop(restored);
    cleanup(root);
}
