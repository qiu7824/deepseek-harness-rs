//! Rust port of the core `packages/settings/settings/tests/*.spec.ts`
//! behaviors: provider metadata, namespace branding, registration/resolution
//! layering, update/replace/mutate write paths, watcher/event emission,
//! conflict detection, describe + redaction, invariant companion, and the
//! optional-settings consumer wiring.

use std::sync::Arc;

use cordis::Context;
use dsh_settings::{
    SettingsDescribeOptions, SettingsNamespace, SettingsPathOp, SettingsProvider,
    SettingsRegisterOptions, SettingsStorage, deep_equal_json, install_settings_section,
    settings_namespace,
};
use indexmap::IndexMap;
use parking_lot::Mutex;
use schemastery::{Data, Schema};

fn theme_schema() -> Schema {
    let mut properties = IndexMap::new();
    properties.insert(
        "theme".to_string(),
        Schema::union(vec![
            Schema::constant(Data::String("dark".to_string())),
            Schema::constant(Data::String("light".to_string())),
        ])
        .default(Data::String("dark".to_string())),
    );
    properties.insert(
        "font_size".to_string(),
        Schema::number().default(Data::Number(14.0)),
    );
    Schema::object(properties)
}

fn nested_schema() -> Schema {
    let mut retry = IndexMap::new();
    retry.insert(
        "attempts".to_string(),
        Schema::number().default(Data::Number(2.0)),
    );
    retry.insert(
        "delayMs".to_string(),
        Schema::number().default(Data::Number(100.0)),
    );
    let mut properties = IndexMap::new();
    properties.insert("retry".to_string(), Schema::object(retry));
    properties.insert(
        "tags".to_string(),
        Schema::array(Schema::string())
            .default(Data::Array(vec![Data::String("default".to_string())])),
    );
    Schema::object(properties)
}

fn ns(name: &str) -> SettingsNamespace {
    settings_namespace(name).unwrap()
}

/// In-memory provider fixture (TS `MemorySettings`).
struct MemorySettings {
    doc: Mutex<IndexMap<String, Data>>,
    persisted: Mutex<Vec<(String, Data)>>,
    writable_flag: bool,
}

#[async_trait::async_trait]
impl SettingsStorage for MemorySettings {
    fn writable(&self) -> bool {
        self.writable_flag
    }

    async fn load(&self) -> Result<IndexMap<String, Data>, String> {
        Ok(self.doc.lock().clone())
    }

    async fn persist(&self, ns: &SettingsNamespace, section: Data) -> Result<(), String> {
        self.persisted
            .lock()
            .push((ns.as_str().to_string(), section.clone()));
        self.doc.lock().insert(ns.as_str().to_string(), section);
        Ok(())
    }
}

fn data(value: serde_json::Value) -> Data {
    json_to_data(&value)
}

fn json_to_data(value: &serde_json::Value) -> Data {
    match value {
        serde_json::Value::Null => Data::Null,
        serde_json::Value::Bool(value) => Data::Bool(*value),
        serde_json::Value::Number(value) => Data::Number(value.as_f64().unwrap()),
        serde_json::Value::String(value) => Data::String(value.clone()),
        serde_json::Value::Array(array) => Data::Array(array.iter().map(json_to_data).collect()),
        serde_json::Value::Object(object) => {
            let mut entries = IndexMap::new();
            for (key, value) in object {
                entries.insert(key.clone(), json_to_data(value));
            }
            Data::Object(entries)
        }
    }
}

async fn boot(
    doc: Option<IndexMap<String, Data>>,
) -> (Context, Arc<SettingsProvider>, Arc<MemorySettings>) {
    let ctx = Context::root();
    let storage = Arc::new(MemorySettings {
        doc: Mutex::new(doc.unwrap_or_default()),
        persisted: Mutex::new(Vec::new()),
        writable_flag: true,
    });
    let provider = SettingsProvider::install(&ctx, storage.clone());
    provider.ready().await.unwrap();
    (ctx, provider, storage)
}

#[tokio::test(flavor = "multi_thread")]
async fn brands_lowercase_kebab_case_names() {
    assert_eq!(settings_namespace("ui-theme").unwrap().as_str(), "ui-theme");
    for invalid in ["", "UI", "9lives", "a_b", "-lead"] {
        assert!(
            settings_namespace(invalid).is_err(),
            "{invalid} must reject"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn resolves_defaults_then_base_then_user_layer() {
    let mut doc = IndexMap::new();
    doc.insert(
        "ui-theme".to_string(),
        data(serde_json::json!({"theme": "light"})),
    );
    let (ctx, provider, _) = boot(Some(doc)).await;
    let scope = provider
        .register(
            &ctx,
            ns("ui-theme"),
            theme_schema(),
            SettingsRegisterOptions {
                base: Some(data(serde_json::json!({"font_size": 16}))),
                ..Default::default()
            },
        )
        .unwrap();
    // theme: user layer wins; font_size: base wins over the schema default.
    let value = (scope.get)();
    assert_eq!(
        data(serde_json::json!({"theme": "light", "font_size": 16})),
        value
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn validate_rejects_bad_write_and_publish_keeps_last_good() {
    let (ctx, provider, _) = boot(None).await;
    let scope = provider
        .register(
            &ctx,
            ns("ui-theme"),
            theme_schema(),
            SettingsRegisterOptions {
                validate: Some(Arc::new(|value: &Data| {
                    let font_size = match value {
                        Data::Object(object) => object.get("font_size").and_then(|v| match v {
                            Data::Number(value) => Some(*value),
                            _ => None,
                        }),
                        _ => None,
                    };
                    if let Some(size) = font_size {
                        if size < 10.0 {
                            return Err(format!("font size {size} is unreadable"));
                        }
                    }
                    Ok(())
                })),
                ..Default::default()
            },
        )
        .unwrap();
    let before = (scope.get)();

    let error = provider
        .update(&ns("ui-theme"), serde_json::json!({"font_size": 4}), None)
        .await
        .err()
        .expect("update must fail");
    assert!(error.contains("unreadable"), "{error}");
    assert_eq!((scope.get)(), before);

    // An externally edited document keeps the last good value.
    let mut doc = IndexMap::new();
    doc.insert(
        "ui-theme".to_string(),
        data(serde_json::json!({"font_size": 4})),
    );
    provider.publish(doc, dsh_settings::SettingsUpdateSource::Provider);
    assert_eq!((scope.get)(), before);

    provider
        .update(&ns("ui-theme"), serde_json::json!({"font_size": 18}), None)
        .await
        .unwrap();
    let value = (scope.get)();
    assert_eq!(
        data(serde_json::json!({"theme": "dark", "font_size": 18})),
        value
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn fails_registration_when_stored_section_is_unserviceable() {
    let mut doc = IndexMap::new();
    doc.insert(
        "ui-theme".to_string(),
        data(serde_json::json!({"font_size": 4})),
    );
    let (ctx, provider, _) = boot(Some(doc)).await;
    let error = provider
        .register(
            &ctx,
            ns("ui-theme"),
            theme_schema(),
            SettingsRegisterOptions {
                validate: Some(Arc::new(|value: &Data| {
                    let font_size = match value {
                        Data::Object(object) => object.get("font_size").and_then(|v| match v {
                            Data::Number(value) => Some(*value),
                            _ => None,
                        }),
                        _ => None,
                    };
                    if let Some(size) = font_size {
                        if size < 10.0 {
                            return Err(format!("font size {size} is unreadable"));
                        }
                    }
                    Ok(())
                })),
                ..Default::default()
            },
        )
        .err()
        .expect("register must fail");
    assert!(error.contains("unreadable"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_duplicate_and_invalid_sections() {
    let (ctx, provider, _) = boot(None).await;
    provider
        .register(
            &ctx,
            ns("ui-theme"),
            theme_schema(),
            SettingsRegisterOptions::default(),
        )
        .unwrap();
    let error = provider
        .register(
            &ctx,
            ns("ui-theme"),
            theme_schema(),
            SettingsRegisterOptions::default(),
        )
        .err()
        .expect("duplicate must fail");
    assert!(error.contains("already registered"), "{error}");

    // A non-object stored section rejects.
    let mut doc = IndexMap::new();
    doc.insert("ui-other".to_string(), Data::String("dark".to_string()));
    let (ctx2, provider2, _) = boot(Some(doc)).await;
    let error = provider2
        .register(
            &ctx2,
            ns("ui-other"),
            theme_schema(),
            SettingsRegisterOptions::default(),
        )
        .err()
        .expect("non-object section must fail");
    assert!(error.contains("must be an object"), "{error}");

    // A schema-invalid stored section rejects.
    let mut doc = IndexMap::new();
    doc.insert(
        "ui-other".to_string(),
        data(serde_json::json!({"font_size": "big"})),
    );
    let (ctx3, provider3, _) = boot(Some(doc)).await;
    assert!(
        provider3
            .register(
                &ctx3,
                ns("ui-other"),
                theme_schema(),
                SettingsRegisterOptions::default()
            )
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_undefined_for_unregistered_namespace() {
    let (_, provider, _) = boot(None).await;
    assert!(provider.get(&ns("missing")).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn update_persists_merged_section_without_baking_base() {
    let mut doc = IndexMap::new();
    doc.insert(
        "ui-theme".to_string(),
        data(serde_json::json!({"theme": "light"})),
    );
    let (ctx, provider, storage) = boot(Some(doc)).await;
    let scope = provider
        .register(
            &ctx,
            ns("ui-theme"),
            theme_schema(),
            SettingsRegisterOptions {
                base: Some(data(serde_json::json!({"font_size": 16}))),
                ..Default::default()
            },
        )
        .unwrap();
    (scope.update)(serde_json::json!({"theme": "dark"}))
        .await
        .unwrap();
    let persisted = storage.persisted.lock().clone();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].1, data(serde_json::json!({"theme": "dark"})));
    assert_eq!(
        (scope.get)(),
        data(serde_json::json!({"theme": "dark", "font_size": 16}))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn update_deep_merges_nested_objects_and_replaces_arrays() {
    let mut doc = IndexMap::new();
    doc.insert(
        "workspace".to_string(),
        data(serde_json::json!({"retry": {"attempts": 5, "delayMs": 300}, "tags": ["a", "b"]})),
    );
    let (ctx, provider, storage) = boot(Some(doc)).await;
    let scope = provider
        .register(
            &ctx,
            ns("workspace"),
            nested_schema(),
            SettingsRegisterOptions::default(),
        )
        .unwrap();
    (scope.update)(serde_json::json!({"retry": {"attempts": 7}, "tags": ["c"]}))
        .await
        .unwrap();
    let persisted = storage.persisted.lock().clone();
    assert_eq!(
        persisted[0].1,
        data(serde_json::json!({"retry": {"attempts": 7, "delayMs": 300}, "tags": ["c"]}))
    );
    assert_eq!(
        (scope.get)(),
        data(serde_json::json!({"retry": {"attempts": 7, "delayMs": 300}, "tags": ["c"]}))
    );
}

/// Wait until a mutex-guarded value satisfies a predicate (the watcher
/// segments run on spawned tasks; TS's promise microtask ordering has no
/// deterministic Rust analogue).
async fn wait_until(condition: impl Fn() -> bool) {
    for _ in 0..1000 {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    panic!("condition did not become true in time");
}

#[tokio::test(flavor = "multi_thread")]
async fn commits_notifies_watchers_and_emits_update_event() {
    let (ctx, provider, _) = boot(None).await;
    let seen: Arc<Mutex<Vec<(Data, Data)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_for_event = Arc::clone(&seen);
    {
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let events_for_assert = Arc::clone(&events);
        let event_ctx = ctx.clone();
        let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args: Vec<cordis::ArcValue>| {
            let ns = cordis::downcast::<SettingsNamespace>(&args[0])
                .cloned()
                .unwrap();
            let events = Arc::clone(&events);
            Box::pin(async move {
                events.lock().push(ns.as_str().to_string());
                None
            })
        });
        let _ = futures::executor::block_on(event_ctx.on(
            "settings/updated",
            listener,
            cordis::EventOptions::default(),
        ));
        let scope = provider
            .register(
                &ctx,
                ns("ui-theme"),
                theme_schema(),
                SettingsRegisterOptions::default(),
            )
            .unwrap();
        (scope.watch)(Arc::new({
            let seen = Arc::clone(&seen);
            move |next: &Data, prev: &Data| {
                let seen = Arc::clone(&seen);
                let next = next.clone();
                let prev = prev.clone();
                Box::pin(async move {
                    seen.lock().push((next, prev));
                })
            }
        }));
        (scope.update)(serde_json::json!({"theme": "light"}))
            .await
            .unwrap();
        wait_until(|| seen_for_event.lock().len() == 1).await;
        assert_eq!(
            seen_for_event.lock().clone(),
            vec![(
                data(serde_json::json!({"theme": "light", "font_size": 14})),
                data(serde_json::json!({"theme": "dark", "font_size": 14})),
            )]
        );
        assert_eq!(
            events_for_assert.lock().clone(),
            vec!["ui-theme".to_string()]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_invalid_patch_before_persisting() {
    let (ctx, provider, storage) = boot(None).await;
    provider
        .register(
            &ctx,
            ns("ui-theme"),
            theme_schema(),
            SettingsRegisterOptions::default(),
        )
        .unwrap();
    assert!(
        provider
            .update(
                &ns("ui-theme"),
                serde_json::json!({"font_size": "big"}),
                None
            )
            .await
            .is_err()
    );
    assert!(storage.persisted.lock().is_empty(), "nothing persisted");
}

#[tokio::test(flavor = "multi_thread")]
async fn replace_resets_and_expected_revision_rejects_stale() {
    let (ctx, provider, _) = boot(None).await;
    let scope = provider
        .register(
            &ctx,
            ns("ui-theme"),
            theme_schema(),
            SettingsRegisterOptions {
                base: Some(data(serde_json::json!({"font_size": 16}))),
                ..Default::default()
            },
        )
        .unwrap();
    (scope.update)(serde_json::json!({"theme": "light", "font_size": 20}))
        .await
        .unwrap();
    let descriptors = provider.describe(SettingsDescribeOptions::default());
    assert_eq!(descriptors[0].revision, 1);

    // A stale expectedRevision rejects.
    let error = provider
        .update(
            &ns("ui-theme"),
            serde_json::json!({"font_size": 22}),
            Some(0),
        )
        .await
        .err()
        .expect("stale write must fail");
    assert!(error.contains("changed since it was read"), "{error}");

    // replace({}) re-inherits the base.
    (scope.replace)(serde_json::json!({})).await.unwrap();
    assert_eq!(
        (scope.get)(),
        data(serde_json::json!({"theme": "dark", "font_size": 16}))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mutate_applies_path_ops_on_the_current_section() {
    let mut doc = IndexMap::new();
    doc.insert(
        "workspace".to_string(),
        data(serde_json::json!({"retry": {"attempts": 5, "delayMs": 300}})),
    );
    let (ctx, provider, storage) = boot(Some(doc)).await;
    let scope = provider
        .register(
            &ctx,
            ns("workspace"),
            nested_schema(),
            SettingsRegisterOptions::default(),
        )
        .unwrap();
    provider
        .mutate(
            &ns("workspace"),
            vec![
                SettingsPathOp::Set {
                    path: vec!["retry".to_string(), "attempts".to_string()],
                    value: serde_json::json!(9),
                },
                SettingsPathOp::Unset {
                    path: vec!["retry".to_string(), "delayMs".to_string()],
                },
            ],
            None,
        )
        .await
        .unwrap();
    let persisted = storage.persisted.lock().clone();
    assert_eq!(
        persisted[0].1,
        data(serde_json::json!({"retry": {"attempts": 9}}))
    );
    assert_eq!(
        (scope.get)(),
        data(serde_json::json!({"retry": {"attempts": 9, "delayMs": 100}, "tags": ["default"]}))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn publish_moves_revision_and_emits_document_updated() {
    let (ctx, provider, _) = boot(None).await;
    provider
        .register(
            &ctx,
            ns("ui-theme"),
            theme_schema(),
            SettingsRegisterOptions::default(),
        )
        .unwrap();
    let seen: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let seen = Arc::clone(&seen);
        let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args: Vec<cordis::ArcValue>| {
            let revision = cordis::downcast::<u64>(&args[1]).copied().unwrap();
            let seen = Arc::clone(&seen);
            Box::pin(async move {
                seen.lock().push(revision);
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "settings/document-updated",
            listener,
            cordis::EventOptions::default(),
        ));
    }
    let mut doc = IndexMap::new();
    doc.insert(
        "ui-theme".to_string(),
        data(serde_json::json!({"theme": "light"})),
    );
    provider.publish(doc, dsh_settings::SettingsUpdateSource::Provider);
    assert_eq!(seen.lock().clone(), vec![1]);
    assert_eq!(
        provider.describe(SettingsDescribeOptions::default())[0].revision,
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn describe_redacts_secret_fields() {
    let mut secret = IndexMap::new();
    secret.insert("apiKey".to_string(), Schema::string().role("secret", None));
    let mut properties = IndexMap::new();
    properties.insert(
        "endpoint".to_string(),
        Schema::string().default(Data::String("https://x".to_string())),
    );
    properties.insert("apiKey".to_string(), Schema::string().role("secret", None));
    let schema = Schema::object(properties);
    let _ = secret;
    let (ctx, provider, _) = boot(None).await;
    provider
        .register(&ctx, ns("wire"), schema, SettingsRegisterOptions::default())
        .unwrap();
    provider
        .update(&ns("wire"), serde_json::json!({"apiKey": "hunter2"}), None)
        .await
        .unwrap();
    let descriptors = provider.describe(SettingsDescribeOptions {
        redact_secrets: true,
    });
    assert_eq!(descriptors.len(), 1);
    assert_eq!(
        descriptors[0].secrets,
        vec![dsh_settings::RedactedSecret {
            path: vec!["apiKey".to_string()],
            set: true,
        }]
    );
    let value = descriptors[0].value.clone();
    match value {
        Data::Object(object) => {
            assert!(!object.contains_key("apiKey"), "secret removed");
            assert_eq!(
                object.get("endpoint"),
                Some(&Data::String("https://x".to_string()))
            );
        }
        other => panic!("unexpected value {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn deep_equal_json_compares_structurally() {
    assert!(deep_equal_json(
        &data(serde_json::json!({"a": [1, {"b": 2}]})),
        &data(serde_json::json!({"a": [1, {"b": 2}]}))
    ));
    assert!(!deep_equal_json(
        &data(serde_json::json!({"a": 1})),
        &data(serde_json::json!({"a": 2}))
    ));
    assert!(!deep_equal_json(
        &data(serde_json::json!([1, 2])),
        &data(serde_json::json!([1, 3]))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn removes_namespace_when_registrant_fiber_disposes() {
    let (ctx, provider, _) = boot(None).await;
    let fiber = ctx.plugin(Arc::new(ThemePlugin), cordis::arc(serde_json::Value::Null));
    fiber.settle().await.unwrap();
    assert!(provider.get(&ns("ui-theme")).is_some());
    fiber.dispose().await;
    assert!(provider.get(&ns("ui-theme")).is_none());
    assert!(
        provider
            .describe(SettingsDescribeOptions::default())
            .is_empty()
    );
}

struct ThemePlugin;

#[async_trait::async_trait]
impl cordis::Plugin for ThemePlugin {
    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(["settings"])
    }

    async fn apply(
        &self,
        ctx: &Context,
        _config: cordis::ArcValue,
    ) -> Result<(), cordis::PluginError> {
        let provider: Arc<Arc<SettingsProvider>> = ctx
            .get_typed::<Arc<SettingsProvider>>("settings", false)
            .ok_or_else(|| cordis::PluginError::new(cordis::arc("settings missing")))?;
        provider
            .register(
                ctx,
                ns("ui-theme"),
                theme_schema(),
                SettingsRegisterOptions::default(),
            )
            .map_err(|error| cordis::PluginError::new(cordis::arc(error)))?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn install_settings_section_wires_source_and_falls_back() {
    let (ctx, provider, _) = boot(None).await;
    // The source sink stores the THUNK (TS `setSource(current)` semantics);
    // asserting reads through it to see the authoritative value.
    let current: Arc<Mutex<Option<Arc<dyn Fn() -> Data + Send + Sync>>>> =
        Arc::new(Mutex::new(None));
    let changes: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let current_for_set = Arc::clone(&current);
    let changes_for_hook = Arc::clone(&changes);
    let fiber = install_settings_section(
        &ctx,
        ns("ui-theme"),
        theme_schema(),
        data(serde_json::json!({"theme": "dark", "font_size": 12})),
        dsh_settings::SettingsSectionHooks {
            set_source: Arc::new(move |source| {
                *current_for_set.lock() = Some(source);
            }),
            on_change: Arc::new(move || {
                *changes_for_hook.lock() += 1;
            }),
            validate: None,
        },
    );
    fiber.settle().await.unwrap();
    // Attach called setSource + onChange.
    assert_eq!(*changes.lock(), 1);
    {
        let source = current.lock().clone().expect("source set");
        assert_eq!(
            source(),
            data(serde_json::json!({"theme": "dark", "font_size": 12}))
        );
    }

    // A committed user change re-judges the consumer.
    provider
        .update(&ns("ui-theme"), serde_json::json!({"theme": "light"}), None)
        .await
        .unwrap();
    wait_until(|| *changes.lock() == 2).await;
    // The source thunk now resolves the scope value.
    {
        let source = current.lock().clone().expect("source set");
        assert_eq!(
            source(),
            data(serde_json::json!({"theme": "light", "font_size": 12}))
        );
    }
    assert_eq!(*changes.lock(), 2);
}
