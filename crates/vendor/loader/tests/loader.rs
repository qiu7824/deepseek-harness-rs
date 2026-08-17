//! Integration tests for the loader: entry lifecycle, config updates,
//! groups, isolation, and self-dispose detection.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_cordis_loader::{EntryOptions, LoaderService};
use indexmap::IndexMap;
use serde_json::{Value, json};

/// Records every config it receives so tests can observe restarts.
struct ProbePlugin {
    name: &'static str,
    runs: Arc<AtomicU32>,
    configs: Arc<std::sync::Mutex<Vec<Value>>>,
    provide_foo: Option<Value>,
}

#[async_trait::async_trait]
impl Plugin for ProbePlugin {
    fn name(&self) -> Option<&'static str> {
        Some(self.name)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let config = cordis::downcast::<Value>(&config)
            .cloned()
            .unwrap_or(Value::Null);
        self.configs.lock().unwrap().push(config);
        if let Some(value) = &self.provide_foo {
            // The entry's isolate option governs the scope; providing
            // directly registers under the entry's label.
            ctx.provide("foo", Some(cordis::arc(value.clone())));
        }
        Ok(())
    }
}

fn probe(
    name: &'static str,
    runs: Arc<AtomicU32>,
    configs: Arc<std::sync::Mutex<Vec<Value>>>,
) -> Arc<dyn Plugin> {
    Arc::new(ProbePlugin {
        name,
        runs,
        configs,
        provide_foo: None,
    })
}

async fn setup() -> (Context, Arc<LoaderService>) {
    let ctx = Context::root();
    let fiber = ctx.plugin(dsh_cordis_loader::plugin(), cordis::arc(()));
    fiber.settle().await.expect("loader plugin loads");
    let service = ctx
        .get_typed::<Arc<LoaderService>>("loader", true)
        .expect("loader service")
        .as_ref()
        .clone();
    (ctx, service)
}

fn entry(name: &str, config: Value) -> EntryOptions {
    EntryOptions {
        name: name.to_string(),
        config: Some(config),
        ..EntryOptions::default()
    }
}

#[tokio::test]
async fn entry_starts_and_runs_plugin() {
    let (_ctx, service) = setup().await;
    let runs = Arc::new(AtomicU32::new(0));
    let configs = Arc::new(std::sync::Mutex::new(Vec::new()));
    service
        .core
        .register("probe", probe("probe", runs.clone(), configs.clone()));

    let id = service
        .tree
        .create(entry("probe", json!(42)), None, None)
        .await
        .expect("create entry");
    service.tree.await_ready().await.expect("tree settles");

    let entry = service.tree.resolve(&id).expect("resolve entry");
    let fiber = entry.fiber.lock().clone().expect("live fiber");
    assert!(fiber.uid_value().is_some());
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(configs.lock().unwrap().as_slice(), &[json!(42)]);
    assert_eq!(service.locate(&fiber), Some(id.clone()));
}

#[tokio::test]
async fn config_update_restarts_plugin_in_place() {
    let (_ctx, service) = setup().await;
    let runs = Arc::new(AtomicU32::new(0));
    let configs = Arc::new(std::sync::Mutex::new(Vec::new()));
    service
        .core
        .register("probe", probe("probe", runs.clone(), configs.clone()));

    let id = service
        .tree
        .create(entry("probe", json!(1)), None, None)
        .await
        .unwrap();
    service.tree.await_ready().await.unwrap();
    let entry = service.tree.resolve(&id).unwrap();
    let fiber_before = entry.fiber.lock().clone().unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    // config-only change keeps the fiber (TS _patchContext path)
    let patch = IndexMap::from([("config".to_string(), json!(2))]);
    entry.update(patch, false).await.expect("config update");
    service.tree.await_ready().await.unwrap();

    let fiber_after = entry.fiber.lock().clone().expect("still live");
    assert!(
        Arc::ptr_eq(&fiber_before, &fiber_after),
        "config-only update keeps the fiber"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(configs.lock().unwrap().as_slice(), &[json!(1), json!(2)]);
}

#[tokio::test]
async fn name_update_replaces_fiber() {
    let (_ctx, service) = setup().await;
    let runs = Arc::new(AtomicU32::new(0));
    let configs = Arc::new(std::sync::Mutex::new(Vec::new()));
    service
        .core
        .register("probe", probe("probe", runs.clone(), configs.clone()));
    service
        .core
        .register("probe2", probe("probe2", runs.clone(), configs.clone()));

    let id = service
        .tree
        .create(entry("probe", json!(1)), None, None)
        .await
        .unwrap();
    service.tree.await_ready().await.unwrap();
    let entry = service.tree.resolve(&id).unwrap();
    let fiber_before = entry.fiber.lock().clone().unwrap();

    let patch = IndexMap::from([
        ("name".to_string(), json!("probe2")),
        ("config".to_string(), json!(3)),
    ]);
    entry.update(patch, false).await.expect("replace");
    service.tree.await_ready().await.unwrap();

    let fiber_after = entry.fiber.lock().clone().expect("live after replace");
    assert!(!Arc::ptr_eq(&fiber_before, &fiber_after));
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(configs.lock().unwrap().last(), Some(&json!(3)));
}

#[tokio::test]
async fn remove_disposes_entry() {
    let (_ctx, service) = setup().await;
    let runs = Arc::new(AtomicU32::new(0));
    let configs = Arc::new(std::sync::Mutex::new(Vec::new()));
    service
        .core
        .register("probe", probe("probe", runs.clone(), configs.clone()));

    let id = service
        .tree
        .create(entry("probe", json!(1)), None, None)
        .await
        .unwrap();
    service.tree.await_ready().await.unwrap();
    service.tree.remove(&id).await.expect("remove");
    assert!(service.tree.resolve(&id).is_err());
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disabled_entries_do_not_start() {
    let (_ctx, service) = setup().await;
    let runs = Arc::new(AtomicU32::new(0));
    let configs = Arc::new(std::sync::Mutex::new(Vec::new()));
    service
        .core
        .register("probe", probe("probe", runs.clone(), configs.clone()));

    let mut options = entry("probe", json!(1));
    options.disabled = Some(json!(true));
    let id = service.tree.create(options, None, None).await.unwrap();
    service.tree.await_ready().await.unwrap();
    let entry = service.tree.resolve(&id).unwrap();
    assert!(entry.fiber.lock().is_none());
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn nested_groups_mount_child_entries() {
    let (_ctx, service) = setup().await;
    let runs = Arc::new(AtomicU32::new(0));
    let configs = Arc::new(std::sync::Mutex::new(Vec::new()));
    service
        .core
        .register("probe", probe("probe", runs.clone(), configs.clone()));

    let mut group_options = EntryOptions {
        name: "group".to_string(),
        group: Some(true),
        ..EntryOptions::default()
    };
    group_options.config = Some(json!([{ "name": "probe", "config": 7 }]));
    let group_id = service
        .tree
        .create(group_options, None, None)
        .await
        .unwrap();
    service.tree.await_ready().await.unwrap();

    let group = service
        .tree
        .resolve_group(Some(&group_id))
        .expect("resolve group");
    assert_eq!(group.data.lock().len(), 1);
    let child = &group.data.lock()[0];
    assert_eq!(child.name, "probe");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(configs.lock().unwrap().as_slice(), &[json!(7)]);
}

#[tokio::test]
async fn self_dispose_marks_entry_disabled() {
    let (_ctx, service) = setup().await;
    let runs = Arc::new(AtomicU32::new(0));
    let configs = Arc::new(std::sync::Mutex::new(Vec::new()));
    service
        .core
        .register("probe", probe("probe", runs.clone(), configs.clone()));

    let id = service
        .tree
        .create(entry("probe", json!(1)), None, None)
        .await
        .unwrap();
    service.tree.await_ready().await.unwrap();
    let entry = service.tree.resolve(&id).unwrap();
    let fiber = entry.fiber.lock().clone().unwrap();
    fiber.dispose().await;
    // the loader's internal/plugin listener persists `disabled: true`
    assert_eq!(
        entry.options.lock().disabled,
        Some(json!(true)),
        "self-dispose must be persisted as disabled"
    );
}

#[tokio::test]
async fn local_isolation_scopes_providers() {
    let (_ctx, service) = setup().await;

    // Two entries, each providing "foo" in its own local scope.
    let make = |name: &'static str, value: i32| -> Arc<dyn Plugin> {
        Arc::new(ProbePlugin {
            name,
            runs: Arc::new(AtomicU32::new(0)),
            configs: Arc::new(std::sync::Mutex::new(Vec::new())),
            provide_foo: Some(json!(value)),
        })
    };
    service.core.register("p1", make("p1", 1));
    service.core.register("p2", make("p2", 2));

    let mut opts1 = entry("p1", json!(null));
    opts1.isolate = Some([("foo".to_string(), None)].into_iter().collect());
    let mut opts2 = entry("p2", json!(null));
    opts2.isolate = Some([("foo".to_string(), None)].into_iter().collect());
    let id1 = service.tree.create(opts1, None, None).await.unwrap();
    let id2 = service.tree.create(opts2, None, None).await.unwrap();
    service.tree.await_ready().await.unwrap();

    let e1 = service.tree.resolve(&id1).unwrap();
    let e2 = service.tree.resolve(&id2).unwrap();
    let f1 = e1.fiber.lock().clone().expect("p1 live");
    let f2 = e2.fiber.lock().clone().expect("p2 live");
    let v1 = f1.ctx().unwrap().get("foo", true).expect("p1 foo");
    let v2 = f2.ctx().unwrap().get("foo", true).expect("p2 foo");
    assert_eq!(*cordis::downcast::<Value>(&v1).unwrap(), json!(1));
    assert_eq!(*cordis::downcast::<Value>(&v2).unwrap(), json!(2));
}

#[tokio::test]
async fn unsupported_js_exprs_fail_entries() {
    let (_ctx, service) = setup().await;
    let runs = Arc::new(AtomicU32::new(0));
    let configs = Arc::new(std::sync::Mutex::new(Vec::new()));
    service
        .core
        .register("probe", probe("probe", runs.clone(), configs.clone()));

    // TS also rejects at entry create: the fiber fails to apply and the
    // update propagates out of `group.create`.
    let result = service
        .tree
        .create(
            entry("probe", json!({ "__jsExpr": "process.env.X" })),
            None,
            None,
        )
        .await;
    assert!(result.is_err(), "js-expr config must fail loudly");
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[test]
fn entry_options_serde_round_trip() {
    let options = EntryOptions {
        id: "abc".to_string(),
        name: "probe".to_string(),
        config: Some(json!({ "a": 1 })),
        group: Some(true),
        disabled: Some(json!(false)),
        inject: Some([("foo".to_string(), None)].into_iter().collect()),
        intercept: Some([("bar".to_string(), json!(1))].into_iter().collect()),
        isolate: Some(
            [("baz".to_string(), Some("shared".to_string()))]
                .into_iter()
                .collect(),
        ),
    };
    let value = serde_json::to_value(&options).unwrap();
    let back: EntryOptions = serde_json::from_value(value).unwrap();
    assert_eq!(back.id, "abc");
    assert_eq!(back.name, "probe");
    assert_eq!(back.config, Some(json!({ "a": 1 })));
    assert_eq!(back.group, Some(true));
    assert_eq!(
        back.isolate.unwrap().get("baz").cloned(),
        Some(Some("shared".to_string()))
    );
}
