use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const MODULE: &str = "@deepseek-ai/dsh-time-context";

struct ConfigPlugin {
    applied: Arc<AtomicUsize>,
    gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    external_write: Option<std::path::PathBuf>,
}

#[async_trait]
impl cordis::Plugin for ConfigPlugin {
    async fn apply(
        &self,
        _: &Context,
        config: cordis::ArcValue,
    ) -> Result<(), cordis::PluginError> {
        self.applied.fetch_add(1, Ordering::SeqCst);
        let changing = config
            .downcast_ref::<Value>()
            .is_some_and(|value| value["refreshIntervalMs"] == 30_000);
        if changing {
            if let Some((entered, release)) = &self.gate {
                entered.notify_one();
                release.notified().await;
            }
            if let Some(path) = &self.external_write {
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                value[1]["config"]["external"] = json!(true);
                std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
        }
        Ok(())
    }
}

struct Fixture {
    root: std::path::PathBuf,
    profile: std::path::PathBuf,
    ctx: Context,
    loader: Arc<LoaderService>,
    api: Arc<ApiProxyService>,
    applied: Arc<AtomicUsize>,
}

impl Fixture {
    async fn new(
        disabled: bool,
        gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        external: bool,
    ) -> Self {
        let root = std::env::temp_dir().join(format!("plugin-config-api-{}", uuid::Uuid::new_v4()));
        let profile = root.join("profiles/web");
        let rows = vec![
            json!({"id":"clock","name":MODULE,"disabled":disabled,"config":{"refreshIntervalMs":600000}}),
            json!({"id":"other","name":"secret-plugin","disabled":true,"config":{"apiKey":"must-not-return"}}),
        ];
        {
            let owner = Profile::open(&profile).unwrap();
            owner
                .replace(
                    Documents {
                        manifest: json!({"dependencies":{},"unrelated":"keep"}),
                        entries: rows.clone(),
                    },
                    None,
                )
                .unwrap();
        }
        let ctx = Context::root();
        let loader = LoaderService::new(&ctx).await;
        let applied = Arc::new(AtomicUsize::new(0));
        loader.core.register(
            MODULE,
            Arc::new(ConfigPlugin {
                applied: applied.clone(),
                gate,
                external_write: external.then(|| profile.join("plugins.json")),
            }),
        );
        ctx.register_service(loader.clone());
        loader
            .tree
            .create(serde_json::from_value(rows[0].clone()).unwrap(), None, None)
            .await
            .unwrap();
        let api = ApiProxyService::install(
            &ctx,
            ApiProxyDefaults {
                dsh_home: root.to_string_lossy().into_owned(),
                plugins_document: Some(profile.join("plugins.json")),
                ..Default::default()
            },
        );
        Self {
            root,
            profile,
            ctx,
            loader,
            api,
            applied,
        }
    }

    async fn get(&self, id: &str) -> Value {
        serde_json::to_value(
            self.api
                .plugin_inventory_get_config(RpcRequest {
                    rpc_id: crate::api::rpc::rpc_id("read"),
                    payload: PluginGetConfigRequest {
                        entry_id: id.into(),
                    },
                })
                .await,
        )
        .unwrap()["result"]
            .clone()
    }

    async fn set(&self, revision: &str, config: Value, signal: AbortSignal) -> Value {
        serde_json::to_value(
            self.api
                .plugin_inventory_set_config(
                    RpcRequest {
                        rpc_id: crate::api::rpc::rpc_id("write"),
                        payload: PluginSetConfigRequest {
                            entry_id: "clock".into(),
                            expected_revision: revision.into(),
                            config,
                        },
                    },
                    signal,
                )
                .await,
        )
        .unwrap()["result"]
            .clone()
    }

    async fn fetch(&self, method: &str, payload: Value) -> Value {
        let handler = crate::fetch::handler::to_fetch_handler(self.api.clone());
        let response = handler
            .handle(crate::fetch::handler::CarrierRequest {
                method: http::Method::POST,
                path: format!("/api/{method}"),
                query: Vec::new(),
                headers: vec![("content-type".into(), "application/json".into())],
                body: Some(
                    serde_json::to_vec(&json!({
                        "type":"client-request", "rpcId":"plugin-config-wire",
                        "method":method, "payload":payload,
                    }))
                    .unwrap(),
                ),
            })
            .await;
        assert_eq!(response.status(), http::StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("config RPC must return a JSON response")
        };
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["rpcId"], "plugin-config-wire");
        serde_json::from_value::<crate::api::rpc::RpcMessage>(value.clone()).unwrap();
        value["result"].clone()
    }

    fn document(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.profile.join("plugins.json")).unwrap()).unwrap()
    }

    async fn close(self) {
        for disposer in self.ctx.fiber.disposables.clear() {
            disposer().await;
        }
        assert!(
            self.root
                .canonicalize()
                .unwrap()
                .starts_with(std::env::temp_dir().canonicalize().unwrap())
        );
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[test]
fn plugin_config_machine_codes_roundtrip_without_reclassifying_unknown_errors() {
    for (prefix, code) in [
        ("PLUGIN_CONFIG_UNSUPPORTED", "plugin-config-unsupported"),
        ("PLUGIN_CONFIG_NOT_FOUND", "plugin-config-not-found"),
        ("PLUGIN_CONFIG_AMBIGUOUS", "plugin-config-ambiguous"),
        (
            "PLUGIN_CONFIG_RUNTIME_CONFLICT",
            "plugin-config-runtime-conflict",
        ),
        ("PLUGIN_CONFIG_UNAVAILABLE", "plugin-config-unavailable"),
        ("PLUGIN_CONFIG_CANCELLED", "plugin-config-cancelled"),
        ("PLUGIN_CONFIG_CONFLICT", "plugin-config-conflict"),
        ("PLUGIN_CONFIG_INVALID", "plugin-config-invalid"),
        ("PLUGIN_CONFIG_APPLY_FAILED", "plugin-config-apply-failed"),
        ("PLUGIN_CONFIG_COMMIT_FAILED", "plugin-config-commit-failed"),
        (
            "PLUGIN_RUNTIME_RECOVERY_REQUIRED",
            "plugin-config-recovery-required",
        ),
    ] {
        let message = format!("{prefix}: retained diagnostic");
        let error = config_error(message.clone());
        assert_eq!(error.code().as_str(), code);
        assert_eq!(
            crate::api::rpc::RpcErrorCode::parse_wire_code(code),
            Some(error.code())
        );
        assert_eq!(error.message(), message);
        let wire = serde_json::to_value(&error).unwrap();
        assert_eq!(wire["code"], code);
        assert_eq!(serde_json::from_value::<RpcError>(wire).unwrap(), error);
    }
    for message in [
        "OS error 5",
        "failed: PLUGIN_CONFIG_CONFLICT: not a domain error",
        "PLUGIN_CONFIG_OPERATION_FAILED: join failure",
    ] {
        assert_eq!(
            config_error(message.into()).code(),
            crate::api::rpc::RpcErrorCode::Internal
        );
    }
}

#[tokio::test]
async fn fetch_config_errors_expose_conflict_unsupported_invalid_and_io_codes() {
    let fixture = Fixture::new(false, None, false).await;
    let unsupported = fixture
        .fetch("pluginInventory.getConfig", json!({"entryId":"other"}))
        .await;
    assert_eq!(unsupported["ok"], false);
    assert_eq!(unsupported["error"]["code"], "plugin-config-unsupported");
    assert!(!unsupported.to_string().contains("must-not-return"));

    let current = fixture
        .fetch(
            "pluginInventory.getConfig",
            json!({"args":{"request":{"entryId":"clock"}}}),
        )
        .await;
    assert_eq!(current["ok"], true);
    let conflict = fixture
        .fetch(
            "pluginInventory.setConfig",
            json!({"entryId":"clock","expectedRevision":"stale","config":{}}),
        )
        .await;
    assert_eq!(conflict["ok"], false);
    assert_eq!(conflict["error"]["code"], "plugin-config-conflict");
    assert!(
        conflict["error"]["message"]
            .as_str()
            .unwrap()
            .contains("PLUGIN_CONFIG_CONFLICT:")
    );

    let invalid = fixture.fetch("pluginInventory.setConfig", json!({"args":{"request":{
        "entryId":"clock","expectedRevision":current["value"]["revision"],"config":{"refreshIntervalMs":-1}
    }}})).await;
    assert_eq!(invalid["error"]["code"], "plugin-config-invalid");
    assert_eq!(fixture.applied.load(Ordering::SeqCst), 1);

    std::fs::write(fixture.profile.join("plugins.json"), b"{broken}").unwrap();
    let io = fixture
        .fetch("pluginInventory.getConfig", json!({"entryId":"clock"}))
        .await;
    assert_eq!(io["ok"], false);
    assert_eq!(io["error"]["code"], "internal");
    fixture.close().await;
}

#[test]
fn snapshot_revision_and_wire_keep_absent_distinct_from_null() {
    let absent = json!({"id":"clock","name":MODULE});
    let null = json!({"id":"clock","name":MODULE,"config":null});
    let a = snapshot(&absent).unwrap();
    let b = snapshot(&null).unwrap();
    assert_ne!(a.revision, b.revision);
    assert!(serde_json::to_value(&a).unwrap().get("config").is_none());
    assert_eq!(serde_json::to_value(&b).unwrap()["config"], Value::Null);
    assert_eq!(
        serde_json::from_value::<PluginConfigSnapshot>(serde_json::to_value(b).unwrap())
            .unwrap()
            .config,
        Some(Value::Null)
    );
    assert!(
        serde_json::from_value::<PluginSetConfigRequest>(
            json!({"entryId":"clock","expectedRevision":"x"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<PluginGetConfigRequest>(json!({"entryId":"clock","config":{}}))
            .is_err()
    );
    let disabled = json!({"id":"clock","name":MODULE,"disabled":true});
    assert_ne!(a.revision, snapshot(&disabled).unwrap().revision);
    assert!(
        snapshot(&json!({"id":"other","name":"secret-plugin","config":{"apiKey":"secret"}}))
            .unwrap_err()
            .contains("UNSUPPORTED")
    );
}

#[tokio::test]
async fn config_cas_preserves_profile_enablement_and_noop_does_not_reload() {
    let fixture = Fixture::new(false, None, false).await;
    let first = fixture.get("clock").await;
    assert_eq!(first["ok"], true);
    let revision = first["value"]["revision"].as_str().unwrap();
    let uid = fixture
        .loader
        .tree
        .resolve("clock")
        .unwrap()
        .fiber
        .lock()
        .as_ref()
        .unwrap()
        .uid_value();
    let bytes = std::fs::read(fixture.profile.join("plugins.json")).unwrap();
    let noop = fixture
        .set(
            revision,
            json!({"refreshIntervalMs":600000}),
            AbortSignal::new(),
        )
        .await;
    assert_eq!(noop["ok"], true);
    assert_eq!(fixture.applied.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture
            .loader
            .tree
            .resolve("clock")
            .unwrap()
            .fiber
            .lock()
            .as_ref()
            .unwrap()
            .uid_value(),
        uid
    );
    assert_eq!(
        std::fs::read(fixture.profile.join("plugins.json")).unwrap(),
        bytes
    );
    let changed = fixture
        .set(
            revision,
            json!({"refreshIntervalMs":0,"timeZone":"UTC"}),
            AbortSignal::new(),
        )
        .await;
    assert_eq!(changed["ok"], true, "{changed}");
    assert_ne!(changed["value"]["revision"], revision);
    assert_eq!(fixture.applied.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.document()[0]["disabled"], false);
    assert_eq!(fixture.document()[1]["config"]["apiKey"], "must-not-return");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(fixture.profile.join("package.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["unrelated"], "keep");
    assert!(!fixture.root.join("settings.json").exists());
    let stale = fixture
        .set(
            revision,
            json!({"refreshIntervalMs":20}),
            AbortSignal::new(),
        )
        .await;
    assert_eq!(stale["ok"], false);
    assert!(
        stale["error"]["message"]
            .as_str()
            .unwrap()
            .contains("PLUGIN_CONFIG_CONFLICT")
    );
    let hidden = fixture.get("other").await;
    assert_eq!(hidden["ok"], false);
    assert!(!hidden.to_string().contains("must-not-return"));
    fixture.close().await;
}

#[tokio::test]
async fn disabled_configs_are_validated_without_enabling_or_loading_fibers() {
    let fixture = Fixture::new(true, None, false).await;
    let current = fixture.get("clock").await;
    let revision = current["value"]["revision"].as_str().unwrap();
    for config in [
        Value::Null,
        json!([]),
        json!({"refreshIntervalMs":-1}),
        json!({"refreshIntervalMs":0.5}),
        json!({"timeZone":"Invalid/Zone"}),
    ] {
        assert_eq!(
            fixture.set(revision, config, AbortSignal::new()).await["ok"],
            false
        );
    }
    assert_eq!(
        fixture
            .set(revision, json!({"refreshIntervalMs":0}), AbortSignal::new())
            .await["ok"],
        true
    );
    assert_eq!(fixture.applied.load(Ordering::SeqCst), 0);
    assert!(
        fixture
            .loader
            .tree
            .resolve("clock")
            .unwrap()
            .disabled()
            .unwrap()
    );
    assert_eq!(fixture.document()[0]["disabled"], true);
    fixture.close().await;
}

#[tokio::test]
async fn config_read_and_write_reject_runtime_drift_and_invalid_profile() {
    let fixture = Fixture::new(false, None, false).await;
    let first = fixture.get("clock").await;
    let revision = first["value"]["revision"].as_str().unwrap();
    let owner = Profile::open(&fixture.profile).unwrap();
    assert_eq!(
        fixture.set(revision, json!({}), AbortSignal::new()).await["ok"],
        false
    );
    drop(owner);
    let mut rows = fixture.document();
    rows[0]["disabled"] = json!(true);
    std::fs::write(
        fixture.profile.join("plugins.json"),
        serde_json::to_vec(&rows).unwrap(),
    )
    .unwrap();
    let drift = fixture.get("clock").await;
    assert!(
        drift["error"]["message"]
            .as_str()
            .unwrap()
            .contains("RUNTIME_CONFLICT")
    );
    rows[0]["disabled"] = Value::Null;
    std::fs::write(
        fixture.profile.join("plugins.json"),
        serde_json::to_vec(&rows).unwrap(),
    )
    .unwrap();
    assert!(
        fixture.get("clock").await["error"]["message"]
            .as_str()
            .unwrap()
            .contains("UNSUPPORTED")
    );
    std::fs::write(fixture.profile.join("plugins.json"), b"{broken}").unwrap();
    assert_eq!(fixture.get("clock").await["ok"], false);
    assert_eq!(
        fixture.set(revision, json!({}), AbortSignal::new()).await["ok"],
        false
    );
    assert_eq!(
        std::fs::read(fixture.profile.join("plugins.json")).unwrap(),
        b"{broken}"
    );
    assert_eq!(fixture.applied.load(Ordering::SeqCst), 1);
    fixture.close().await;
}

#[tokio::test]
async fn failed_profile_commit_rolls_runtime_back_and_preserves_external_write() {
    let fixture = Fixture::new(false, None, true).await;
    let first = fixture.get("clock").await;
    let failed = fixture
        .set(
            first["value"]["revision"].as_str().unwrap(),
            json!({"refreshIntervalMs":30000}),
            AbortSignal::new(),
        )
        .await;
    assert_eq!(failed["ok"], false);
    assert!(
        failed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("COMMIT_FAILED")
    );
    assert_eq!(
        fixture
            .loader
            .tree
            .resolve("clock")
            .unwrap()
            .options
            .lock()
            .config,
        Some(json!({"refreshIntervalMs":600000}))
    );
    assert_eq!(fixture.document()[0]["config"]["refreshIntervalMs"], 600000);
    assert_eq!(fixture.document()[1]["config"]["external"], true);
    assert_eq!(fixture.applied.load(Ordering::SeqCst), 3);
    fixture.close().await;
}

#[tokio::test]
async fn explicit_cancel_and_dropped_carrier_complete_runtime_rollback() {
    for drop_carrier in [false, true] {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let fixture = Fixture::new(false, Some((entered.clone(), release.clone())), false).await;
        let current = fixture.get("clock").await;
        let revision = current["value"]["revision"].as_str().unwrap().to_string();
        let signal = AbortSignal::new();
        let running_signal = signal.clone();
        let api = fixture.api.clone();
        let running = tokio::spawn(async move {
            api.plugin_inventory_set_config(
                RpcRequest {
                    rpc_id: crate::api::rpc::rpc_id("cancel-write"),
                    payload: PluginSetConfigRequest {
                        entry_id: "clock".into(),
                        expected_revision: revision,
                        config: json!({"refreshIntervalMs":30000}),
                    },
                },
                running_signal,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        if drop_carrier {
            running.abort();
            assert!(running.await.unwrap_err().is_cancelled());
            release.notify_one();
        } else {
            signal.abort();
            release.notify_one();
            let finished = tokio::time::timeout(Duration::from_secs(3), running)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(finished.unwrap()).unwrap()["result"]["ok"],
                false
            );
        }
        let after = tokio::time::timeout(Duration::from_secs(3), fixture.get("clock"))
            .await
            .unwrap();
        assert_eq!(after["ok"], true, "{after}");
        assert_eq!(after["value"]["config"]["refreshIntervalMs"], 600000);
        assert_eq!(fixture.document()[0]["config"]["refreshIntervalMs"], 600000);
        assert_eq!(fixture.applied.load(Ordering::SeqCst), 3);
        fixture.close().await;
    }
}
