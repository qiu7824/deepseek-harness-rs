use super::*;
use serde_json::json;

struct Noop;
#[async_trait]
impl cordis::Plugin for Noop {
    async fn apply(&self, _: &Context, _: cordis::ArcValue) -> Result<(), cordis::PluginError> {
        Ok(())
    }
}

#[tokio::test]
async fn plugin_enablement_cancellation_and_client_failure_restore_before_commit() {
    for cancelled in [false, true] {
        let root =
            std::env::temp_dir().join(format!("plugin-client-stage-{}", uuid::Uuid::new_v4()));
        let profile = root.join("profiles/web");
        {
            let profile = dsh_app_boot::plugin_profile::Profile::open(&profile).unwrap();
            profile
                .replace(
                    dsh_app_boot::plugin_profile::Documents {
                        manifest: json!({"dependencies":{"demo":"1"}}),
                        entries: vec![json!({"id":"demo","name":"demo","disabled":true})],
                    },
                    None,
                )
                .unwrap();
        }
        let original = std::fs::read(profile.join("plugins.json")).unwrap();
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
            ApiProxyDefaults {
                dsh_home: root.to_string_lossy().into_owned(),
                plugins_document: Some(profile.join("plugins.json")),
                ..Default::default()
            },
        );
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let signal = AbortSignal::new();
        let ready: PluginClientReady = {
            let entered = entered.clone();
            let release = release.clone();
            Arc::new(move || {
                let entered = entered.clone();
                let release = release.clone();
                Box::pin(async move {
                    entered.notify_one();
                    release.notified().await;
                    if cancelled {
                        Ok(())
                    } else {
                        Err("browser start failed".into())
                    }
                })
            })
        };
        let running_signal = signal.clone();
        let operation = uuid::Uuid::new_v4().to_string();
        let running = tokio::spawn(async move {
            api.apply_plugin_enablement(
                "demo".into(),
                true,
                running_signal,
                Some(operation),
                Arc::new(|_| {}),
                Some(ready),
            )
            .await
        });
        entered.notified().await;
        assert!(!loader.tree.resolve("demo").unwrap().disabled().unwrap());
        assert_eq!(
            std::fs::read(profile.join("plugins.json")).unwrap(),
            original
        );
        if cancelled {
            signal.abort();
        }
        release.notify_one();
        assert!(running.await.unwrap().is_err());
        assert!(loader.tree.resolve("demo").unwrap().disabled().unwrap());
        assert_eq!(
            std::fs::read(profile.join("plugins.json")).unwrap(),
            original
        );
        assert!(!profile.join(".dsh-plugin-last-operation.json").exists());
        for disposer in ctx.fiber.disposables.clear() {
            disposer().await;
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn plugin_toggle_persists_configuration_and_refuses_busy_or_broken_profiles_before_runtime_change()
 {
    let root = std::env::temp_dir().join(format!("plugin-toggle-api-{}", uuid::Uuid::new_v4()));
    let profile = root.join("profiles/web");
    {
        let profile = dsh_app_boot::plugin_profile::Profile::open(&profile).unwrap();
        profile.replace(dsh_app_boot::plugin_profile::Documents {manifest:json!({"dependencies":{"demo":"1"},"unrelated":true}),entries:vec![json!({"id":"demo","name":"demo","disabled":false,"config":{"workspace":"keep"}}),json!({"id":"other","name":"other","disabled":true})]},None).unwrap();
    }
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
        ApiProxyDefaults {
            dsh_home: root.to_string_lossy().into_owned(),
            plugins_document: Some(profile.join("plugins.json")),
            ..Default::default()
        },
    );
    let request = |enabled| RpcRequest {
        rpc_id: crate::api::rpc::rpc_id(uuid::Uuid::new_v4().to_string()),
        payload: dsh_host_plugin_inventory::PluginSetEnabledRequest {
            entry_id: "demo".into(),
            enabled,
        },
    };
    let response = api.plugin_inventory_set_enabled(request(false)).await;
    assert_eq!(
        serde_json::to_value(response).unwrap()["result"]["ok"],
        true
    );
    assert!(loader.tree.resolve("demo").unwrap().disabled().unwrap());
    let owner = dsh_app_boot::plugin_profile::Profile::open(&profile).unwrap();
    let documents = owner.documents().unwrap();
    assert_eq!(documents.entries[0]["config"]["workspace"], "keep");
    assert_eq!(documents.entries[0]["disabled"], true);
    assert_eq!(documents.entries[1]["id"], "other");
    let response = api.plugin_inventory_set_enabled(request(true)).await;
    assert_eq!(
        serde_json::to_value(response).unwrap()["result"]["ok"],
        false
    );
    assert!(loader.tree.resolve("demo").unwrap().disabled().unwrap());
    drop(owner);
    std::fs::write(profile.join("plugins.json"), "{broken}").unwrap();
    let response = api.plugin_inventory_set_enabled(request(true)).await;
    assert_eq!(
        serde_json::to_value(response).unwrap()["result"]["ok"],
        false
    );
    assert!(loader.tree.resolve("demo").unwrap().disabled().unwrap());
    assert_eq!(
        std::fs::read_to_string(profile.join("plugins.json")).unwrap(),
        "{broken}"
    );
    for disposer in ctx.fiber.disposables.clear() {
        disposer().await;
    }
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
