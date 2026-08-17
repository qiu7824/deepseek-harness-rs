//! Integration tests for the include plugin: file mounting, initial
//! creation, patches, write-back persistence, and refresh.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_cordis_include::IncludeConfig;
use dsh_cordis_loader::{EntryOptions, LoaderService};
use serde_json::{Value, json};

struct ProbePlugin {
    name: &'static str,
    runs: Arc<AtomicU32>,
    configs: Arc<std::sync::Mutex<Vec<Value>>>,
}

#[async_trait::async_trait]
impl Plugin for ProbePlugin {
    fn name(&self) -> Option<&'static str> {
        Some(self.name)
    }

    async fn apply(&self, _ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let config = cordis::downcast::<Value>(&config)
            .cloned()
            .unwrap_or(Value::Null);
        self.configs.lock().unwrap().push(config);
        Ok(())
    }
}

fn probe(name: &'static str) -> (Arc<dyn Plugin>, Arc<AtomicU32>, Arc<std::sync::Mutex<Vec<Value>>>) {
    let runs = Arc::new(AtomicU32::new(0));
    let configs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let plugin = Arc::new(ProbePlugin {
        name,
        runs: runs.clone(),
        configs: configs.clone(),
    });
    (plugin, runs, configs)
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
    service.core.register("include", dsh_cordis_include::plugin());
    (ctx, service)
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsh-include-test-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

async fn mount_include(
    service: &Arc<LoaderService>,
    config: IncludeConfig,
) -> Result<(), String> {
    let config_value = serde_json::to_value(&config).unwrap();
    let entry = EntryOptions {
        name: "include".to_string(),
        config: Some(config_value),
        ..EntryOptions::default()
    };
    service.tree.create(entry, None, None).await.map_err(|e| e.to_string())?;
    service.tree.await_ready().await.map_err(|e| e.to_string())
}

fn entry(name: &str, config: Value) -> EntryOptions {
    EntryOptions {
        name: name.to_string(),
        config: Some(config),
        ..EntryOptions::default()
    }
}

#[tokio::test]
async fn mounts_entry_list_from_yaml() {
    let (_ctx, service) = setup().await;
    let (probe_plugin, runs, configs) = probe("probe");
    service.core.register("probe", probe_plugin);

    let dir = temp_dir("yaml");
    let path = dir.join("cordis.yml");
    std::fs::write(
        &path,
        "- id: a\n  name: probe\n  config: 42\n",
    )
    .unwrap();

    mount_include(
        &service,
        IncludeConfig {
            path: path.to_string_lossy().to_string(),
            initial: None,
            patches: None,
            enable_logs: None,
        },
    )
    .await
    .expect("include mounts");

    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(configs.lock().unwrap().as_slice(), &[json!(42)]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn initial_creates_missing_file() {
    let (_ctx, service) = setup().await;
    let (probe_plugin, runs, configs) = probe("probe");
    service.core.register("probe", probe_plugin);

    let dir = temp_dir("initial");
    let path = dir.join("cordis.yml");
    mount_include(
        &service,
        IncludeConfig {
            path: path.to_string_lossy().to_string(),
            initial: Some(vec![EntryOptions {
                name: "probe".to_string(),
                config: Some(json!(7)),
                ..EntryOptions::default()
            }]),
            patches: None,
            enable_logs: None,
        },
    )
    .await
    .expect("include creates and mounts");

    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(configs.lock().unwrap().as_slice(), &[json!(7)]);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("probe"), "initial list persisted: {content}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn patches_override_entries() {
    let (_ctx, service) = setup().await;
    let (probe_plugin, runs, configs) = probe("probe");
    service.core.register("probe", probe_plugin);

    let dir = temp_dir("patches");
    let path = dir.join("cordis.yml");
    std::fs::write(&path, "- id: a\n  name: probe\n  config: 1\n").unwrap();

    let mut patch = indexmap::IndexMap::new();
    patch.insert("id".to_string(), json!("a"));
    patch.insert("config".to_string(), json!(2));
    mount_include(
        &service,
        IncludeConfig {
            path: path.to_string_lossy().to_string(),
            initial: None,
            patches: Some(vec![patch]),
            enable_logs: None,
        },
    )
    .await
    .expect("include mounts");

    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(configs.lock().unwrap().as_slice(), &[json!(2)]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn tree_mutations_persist_to_file() {
    let (_ctx, service) = setup().await;
    let (probe_plugin, runs, _configs) = probe("probe");
    service.core.register("probe", probe_plugin);

    let dir = temp_dir("writeback");
    let path = dir.join("cordis.yml");
    std::fs::write(&path, "- id: a\n  name: probe\n  config: 1\n").unwrap();
    mount_include(
        &service,
        IncludeConfig {
            path: path.to_string_lossy().to_string(),
            initial: None,
            patches: None,
            enable_logs: None,
        },
    )
    .await
    .unwrap();

    // Tree-level mutations persist (TS `group.create` → `tree.write()`);
    // in-place config updates use the noSave path and do not write.
    let include_entry = service
        .tree
        .entries()
        .into_iter()
        .find(|entry| entry.options.lock().name == "include")
        .expect("include entry");
    let include_tree = include_entry.subtree.lock().clone().expect("include subtree");
    include_tree
        .create(entry("probe", json!(5)), None, None)
        .await
        .expect("create second entry");
    // debounced flush runs on the next tick
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("config: 5"), "tree mutation persisted: {content}");
    assert_eq!(runs.load(Ordering::SeqCst), 2, "second probe entry started");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn refresh_reapplies_changed_file() {
    let (_ctx, service) = setup().await;
    let (probe_plugin, runs, configs) = probe("probe");
    service.core.register("probe", probe_plugin);

    let dir = temp_dir("refresh");
    let path = dir.join("cordis.yml");
    std::fs::write(&path, "- id: a\n  name: probe\n  config: 1\n").unwrap();
    mount_include(
        &service,
        IncludeConfig {
            path: path.to_string_lossy().to_string(),
            initial: None,
            patches: None,
            enable_logs: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    std::fs::write(&path, "- id: a\n  name: probe\n  config: 2\n").unwrap();

    // Reach the Include handle through the subtree's owner slot.
    let include_entry = service
        .tree
        .entries()
        .into_iter()
        .find(|entry| entry.options.lock().name == "include")
        .expect("include entry");
    let include_tree = include_entry.subtree.lock().clone().expect("include subtree");
    let handle = include_tree.extras.lock().clone().expect("include handle");
    let include: Arc<dsh_cordis_include::Include> = handle
        .downcast::<Arc<dsh_cordis_include::Include>>()
        .ok()
        .map(|arc| (*arc).clone())
        .expect("include handle type");
    include.refresh().await.expect("refresh re-applies");
    service.tree.await_ready().await.unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(configs.lock().unwrap().last(), Some(&json!(2)));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_update_reapplies_patches_without_rereading() {
    let (_ctx, service) = setup().await;
    let (probe_plugin, runs, configs) = probe("probe");
    service.core.register("probe", probe_plugin);

    let dir = temp_dir("patch-update");
    let path = dir.join("cordis.yml");
    std::fs::write(&path, "- id: a\n  name: probe\n  config: 1\n").unwrap();

    let mut patch = indexmap::IndexMap::new();
    patch.insert("id".to_string(), json!("a"));
    patch.insert("config".to_string(), json!(2));
    mount_include(
        &service,
        IncludeConfig {
            path: path.to_string_lossy().to_string(),
            initial: None,
            patches: Some(vec![patch]),
            enable_logs: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(configs.lock().unwrap().as_slice(), &[json!(2)]);

    // A patches-only config update (the watchUserPatches path) re-applies
    // over the settled data without re-reading the file.
    let include_entry = service
        .tree
        .entries()
        .into_iter()
        .find(|entry| entry.options.lock().name == "include")
        .expect("include entry");
    let mut patch = indexmap::IndexMap::new();
    patch.insert("id".to_string(), json!("a"));
    patch.insert("config".to_string(), json!(3));
    let new_config = IncludeConfig {
        path: path.to_string_lossy().to_string(),
        initial: None,
        patches: Some(vec![patch]),
        enable_logs: None,
    };
    let mut update = indexmap::IndexMap::new();
    update.insert("config".to_string(), serde_json::to_value(&new_config).unwrap());
    include_entry
        .update(update, false)
        .await
        .expect("patches update applies");
    service.tree.await_ready().await.unwrap();

    assert_eq!(configs.lock().unwrap().as_slice(), &[json!(2), json!(3)]);
    assert_eq!(runs.load(Ordering::SeqCst), 2);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn js_expr_in_file_fails_loudly() {
    let (_ctx, service) = setup().await;
    let (probe_plugin, runs, _configs) = probe("probe");
    service.core.register("probe", probe_plugin);

    let dir = temp_dir("jsexpr");
    let path = dir.join("cordis.yml");
    std::fs::write(&path, "- id: a\n  name: probe\n  config: !!js '1 + 1'\n").unwrap();
    let result = mount_include(
        &service,
        IncludeConfig {
            path: path.to_string_lossy().to_string(),
            initial: None,
            patches: None,
            enable_logs: None,
        },
    )
    .await;
    assert!(result.is_err(), "js-expr config must fail loudly: {result:?}");
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    let _ = std::fs::remove_dir_all(&dir);
}
