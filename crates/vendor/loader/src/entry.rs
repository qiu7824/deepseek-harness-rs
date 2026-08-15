//! Loader entry node (port of `src/config/entry.ts`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{ArcValue, Context, FiberCore, Plugin, PluginError, arc};
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::group::EntryGroup;
use crate::isolate::LocalRealm;
use crate::loader::{LoaderCore, LoaderError};
use crate::tree::EntryTree;
use crate::utils::is_js_expr;

/// Serialized plugin entry options stored in loader config files.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntryOptions {
    /// Stable id inside the containing entry tree.
    #[serde(default)]
    pub id: String,
    /// Module specifier imported by the entry tree (registry key in Rust).
    #[serde(default)]
    pub name: String,
    /// Config passed to the plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    /// Marks this entry as a nested group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<bool>,
    /// Prevents this entry and descendants from running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<Value>,
    /// Required services or service intercept config for this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<IndexMap<String, Option<Value>>>,
    /// Service intercept config applied to the entry context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intercept: Option<IndexMap<String, Value>>,
    /// Service isolation scopes (`true` = entry-local, string = shared label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolate: Option<IndexMap<String, Option<String>>>,
}

/// One configured plugin node inside an [`EntryTree`].
pub struct Entry {
    pub core: Arc<LoaderCore>,
    /// Entry context (rebuilt when isolate/intercept options change).
    pub ctx: Mutex<Context>,
    /// The group containing this entry.
    pub parent: Mutex<Option<Arc<EntryGroup>>>,
    /// Serialized options; the write-back target for updates.
    pub options: Mutex<EntryOptions>,
    /// The entry's live plugin fiber.
    pub fiber: Mutex<Option<Arc<FiberCore>>>,
    /// Nested group owned by this entry (group entries).
    pub subgroup: Mutex<Option<Arc<EntryGroup>>>,
    /// Nested subtree owned by this entry (include-provided).
    pub subtree: Mutex<Option<Arc<EntryTree>>>,
    /// Entry-local isolation realm.
    pub realm: Mutex<Option<Arc<LocalRealm>>>,
    disposing: AtomicBool,
}

impl Entry {
    pub fn new(core: Arc<LoaderCore>, parent_ctx: Context) -> Arc<Self> {
        let entry = Arc::new(Self {
            core,
            ctx: Mutex::new(parent_ctx.clone()),
            parent: Mutex::new(None),
            options: Mutex::new(EntryOptions::default()),
            fiber: Mutex::new(None),
            subgroup: Mutex::new(None),
            subtree: Mutex::new(None),
            realm: Mutex::new(None),
            disposing: AtomicBool::new(false),
        });
        parent_ctx.emit("loader/entry-init", vec![arc(entry.clone())]);
        entry
    }

    pub fn self_arc(self: &Arc<Self>) -> Arc<Entry> {
        self.clone()
    }

    /// TS `entry.id` (nested trees prefix with the owner id + `:`).
    pub fn id(&self) -> String {
        let options = self.options.lock();
        let Some(parent) = self.parent.lock().clone() else {
            return options.id.clone();
        };
        let tree = parent.tree.clone();
        match &tree.owner_entry_id {
            Some(owner) => format!("{owner}:{}", options.id),
            None => options.id.clone(),
        }
    }

    /// Whether this entry or any owning parent entry is disabled.
    pub fn disabled(&self) -> Result<bool, LoaderError> {
        let options = self.options.lock().clone();
        self.disabled_of(&options)
    }

    fn disabled_of(&self, options: &EntryOptions) -> Result<bool, LoaderError> {
        // group is always enabled
        if options.group.unwrap_or(false) {
            return Ok(false);
        }
        if disabled_value(&options.disabled)? {
            return Ok(true);
        }
        let mut parent_entry = self.parent.lock().as_ref().and_then(|g| g.owner_entry());
        while let Some(entry) = parent_entry {
            let parent_options = entry.options.lock().clone();
            if disabled_value(&parent_options.disabled)? {
                return Ok(true);
            }
            parent_entry = entry.parent.lock().as_ref().and_then(|g| g.owner_entry());
        }
        Ok(false)
    }

    /// The local isolation realm (created on first use).
    pub fn local_realm(self: &Arc<Self>) -> Arc<LocalRealm> {
        let mut realm = self.realm.lock();
        realm
            .get_or_insert_with(|| LocalRealm::new(self.options.lock().id.clone()))
            .clone()
    }

    /// Whether a replacement is being disposed (self-dispose filtering).
    pub fn disposing(&self) -> bool {
        self.disposing.load(Ordering::SeqCst)
    }

    /// Start the plugin if it is not already running (TS `refresh`).
    pub async fn refresh(self: &Arc<Self>) -> Result<(), LoaderError> {
        if self.fiber.lock().is_some() {
            return Ok(());
        }
        if self.disabled()? {
            return Ok(());
        }
        self.init().await
    }

    /// Import and start the configured plugin (TS `init`).
    pub async fn init(self: &Arc<Self>) -> Result<(), LoaderError> {
        let options = self.options.lock().clone();
        let plugin = self
            .core
            .import(&options.name)
            .map_err(|error| {
                LoaderError::update("import", &options.id, &options.name, error.to_string())
            })?;
        self.start(plugin).await
    }

    async fn start(self: &Arc<Self>, plugin: Arc<dyn Plugin>) -> Result<(), LoaderError> {
        let options = self.options.lock().clone();
        self.patch_context().await?;
        let ctx = self.ctx.lock().clone();
        let config: ArcValue = options.config.clone().map(arc).unwrap_or_else(|| arc(()));
        let fiber = ctx.plugin(plugin, config);
        self.core.track_fiber(&fiber, self.clone());
        if options.group.unwrap_or(false) {
            self.core.mark_carrier(&fiber);
        }
        *self.fiber.lock() = Some(fiber.clone());
        match fiber.settle().await {
            Ok(()) => Ok(()),
            Err(error) => {
                self.dispose_fiber().await?;
                Err(LoaderError::update(
                    "apply",
                    &options.id,
                    &options.name,
                    error.message(),
                ))
            }
        }
    }

    /// Stop the entry's fiber (TS `_dispose`).
    pub async fn dispose_fiber(self: &Arc<Self>) -> Result<(), LoaderError> {
        let fiber = self.fiber.lock().take();
        let Some(fiber) = fiber else { return Ok(()) };
        self.disposing.store(true, Ordering::SeqCst);
        self.core.untrack_fiber(&fiber);
        fiber.dispose().await;
        self.disposing.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Rebuild the entry context from parent + isolate/intercept options and
    /// push config-only updates into the live fiber (TS `_patchContext`).
    async fn patch_context(self: &Arc<Self>) -> Result<(), LoaderError> {
        let parent_ctx = self
            .parent
            .lock()
            .as_ref()
            .map(|group| group.ctx.clone())
            .unwrap_or_else(|| self.ctx.lock().clone());
        let entry = self.clone();
        let core = self.core.clone();
        let error_slot: Arc<Mutex<Option<PluginError>>> = Arc::new(Mutex::new(None));
        let error_slot_for_fallback = error_slot.clone();
        let result = self.ctx.lock().clone().waterfall(
            "loader/patch-context",
            vec![arc(entry.clone())],
            Box::pin(async move {
                let options = entry.options.lock().clone();
                let mut ctx = parent_ctx.clone();
                for (name, label) in options.isolate.clone().unwrap_or_default() {
                    let realm_label = match label {
                        None => entry.local_realm().realm.access(&name, true),
                        Some(label) => core.global_realm(&label).realm.access(&name, true),
                    };
                    ctx = ctx.isolate_with_label(&name, Some(realm_label));
                }
                for (name, config) in options.intercept.clone().unwrap_or_default() {
                    ctx = ctx.intercept(&name, arc(config));
                }
                *entry.ctx.lock() = ctx;
                let live_fiber = entry.fiber.lock().clone();
                if let Some(fiber) = live_fiber {
                    if fiber.uid_value().is_some() {
                        let config: ArcValue =
                            options.config.clone().map(arc).unwrap_or_else(|| arc(()));
                        if let Err(error) = fiber.update(config, true).await {
                            *error_slot_for_fallback.lock() = Some(error);
                        }
                    }
                }
                arc(())
            }),
        );
        let _ = result.await;
        if let Some(error) = error_slot.lock().take() {
            let options = self.options.lock().clone();
            return Err(LoaderError::update(
                "apply",
                &options.id,
                &options.name,
                error.message(),
            ));
        }
        Ok(())
    }

    /// Merge new options, restart as needed (TS `update`).
    pub async fn update(
        self: &Arc<Self>,
        patch: IndexMap<String, Value>,
        force: bool,
    ) -> Result<(), LoaderError> {
        let previous = self.options.lock().clone();
        let mut candidate = previous.clone();
        for (key, value) in patch {
            if value.is_null() {
                match key.as_str() {
                    "id" => {}
                    "name" => candidate.name = String::new(),
                    "config" => candidate.config = None,
                    "group" => candidate.group = None,
                    "disabled" => candidate.disabled = None,
                    "inject" => candidate.inject = None,
                    "intercept" => candidate.intercept = None,
                    "isolate" => candidate.isolate = None,
                    _ => {}
                }
            } else {
                match key.as_str() {
                    "name" => candidate.name = value.as_str().unwrap_or_default().to_string(),
                    "config" => candidate.config = Some(value),
                    "group" => candidate.group = value.as_bool(),
                    "disabled" => candidate.disabled = Some(value),
                    "inject" => {
                        candidate.inject = serde_json::from_value(value).ok().or(candidate.inject)
                    }
                    "intercept" => {
                        candidate.intercept = serde_json::from_value(value).ok().or(candidate.intercept)
                    }
                    "isolate" => {
                        candidate.isolate = serde_json::from_value(value).ok().or(candidate.isolate)
                    }
                    _ => {}
                }
            }
        }
        let diff: Vec<String> = diff_keys(&candidate, &previous);
        if diff.is_empty() && !force {
            return Ok(());
        }

        let previous_fiber = self.fiber.lock().clone();
        if previous_fiber.is_none() {
            *self.options.lock() = candidate.clone();
            match self.init_or_skip().await {
                Ok(()) => {}
                Err(error) => {
                    *self.options.lock() = previous;
                    return Err(error);
                }
            }
            return Ok(());
        }

        let replace = diff
            .iter()
            .any(|key| matches!(key.as_str(), "name" | "inject" | "group" | "isolate" | "intercept"));
        if !replace {
            *self.options.lock() = candidate.clone();
            if let Err(error) = self.patch_context().await {
                *self.options.lock() = previous.clone();
                let _ = self.patch_context().await;
                self.emit_partial_dispose(&candidate, true);
                return Err(error);
            }
            self.emit_partial_dispose(&previous, true);
            return Ok(());
        }

        // Replace: import the new plugin (if the name changed), dispose the
        // old fiber, start the new one, rolling back on failure.
        let plugin = if diff.iter().any(|key| key == "name") {
            match self.core.import(&candidate.name) {
                Ok(plugin) => plugin,
                Err(error) => {
                    return Err(LoaderError::update(
                        "import",
                        &candidate.id,
                        &candidate.name,
                        error.to_string(),
                    ))
                }
            }
        } else {
            let Some(fiber) = &previous_fiber else { unreachable!() };
            let Some(runtime) = &fiber.runtime else { unreachable!() };
            runtime.plugin.clone()
        };
        let previous_plugin = {
            let Some(fiber) = &previous_fiber else { unreachable!() };
            let Some(runtime) = &fiber.runtime else { unreachable!() };
            runtime.plugin.clone()
        };
        *self.options.lock() = candidate.clone();
        if let Err(error) = self.dispose_fiber().await {
            *self.options.lock() = previous.clone();
            return Err(error);
        }
        if let Err(error) = self.start(plugin).await {
            *self.options.lock() = previous.clone();
            let rollback = self.start(previous_plugin).await;
            match rollback {
                Ok(()) => self.emit_partial_dispose(&candidate, true),
                Err(rollback_error) => {
                    return Err(LoaderError::Aggregate(vec![error, rollback_error]));
                }
            }
            return Err(error);
        }
        self.emit_partial_dispose(&previous, true);
        Ok(())
    }

    /// Replace the full option set (create path; TS `update(options, true,
    /// true)`). Disposes a live fiber and starts the new plugin, rolling
    /// back on failure.
    pub async fn replace_options(
        self: &Arc<Self>,
        options: EntryOptions,
    ) -> Result<(), LoaderError> {
        let previous = self.options.lock().clone();
        let previous_fiber = self.fiber.lock().clone();
        let live = previous_fiber
            .as_ref()
            .is_some_and(|fiber| fiber.uid_value().is_some());
        if live {
            *self.options.lock() = options.clone();
            if let Err(error) = self.dispose_fiber().await {
                *self.options.lock() = previous.clone();
                return Err(error);
            }
            let plugin = self.core.import(&options.name).map_err(|error| {
                LoaderError::update("import", &options.id, &options.name, error.to_string())
            })?;
            if let Err(error) = self.start(plugin).await {
                *self.options.lock() = previous.clone();
                let rollback = match self.core.import(&previous.name) {
                    Ok(previous_plugin) => self.start(previous_plugin).await,
                    Err(import_error) => Err(LoaderError::update(
                        "import",
                        &previous.id,
                        &previous.name,
                        import_error.to_string(),
                    )),
                };
                match rollback {
                    Ok(()) => self.emit_partial_dispose(&options, true),
                    Err(rollback_error) => {
                        return Err(LoaderError::Aggregate(vec![error, rollback_error]));
                    }
                }
                return Err(error);
            }
            self.emit_partial_dispose(&previous, true);
            return Ok(());
        }
        *self.options.lock() = options.clone();
        match self.init_or_skip().await {
            Ok(()) => Ok(()),
            Err(error) => {
                *self.options.lock() = previous;
                Err(error)
            }
        }
    }

    async fn init_or_skip(self: &Arc<Self>) -> Result<(), LoaderError> {
        if self.disabled()? {
            return Ok(());
        }
        self.init().await
    }

    fn emit_partial_dispose(self: &Arc<Self>, legacy: &EntryOptions, active: bool) {
        self.ctx.lock().emit(
            "loader/partial-dispose",
            vec![arc(self.clone()), arc(legacy.clone()), arc(active)],
        );
    }
}

fn disabled_value(value: &Option<Value>) -> Result<bool, LoaderError> {
    match value {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(value) if is_js_expr(value) => {
            let expr = value["__jsExpr"].as_str().unwrap_or_default().to_string();
            Err(LoaderError::UnsupportedJs(format!(
                "JavaScript expression \"{expr}\" cannot be evaluated yet (embedded JS runtime milestone)"
            )))
        }
        Some(other) => Ok(other.as_bool().unwrap_or(false)),
    }
}

/// Keys whose values differ between two option sets (TS `update` diff).
fn diff_keys(candidate: &EntryOptions, previous: &EntryOptions) -> Vec<String> {
    let pairs: Vec<(&str, Value, Value)> = vec![
        ("id", json_string(&candidate.id), json_string(&previous.id)),
        ("name", json_string(&candidate.name), json_string(&previous.name)),
        ("config", json_opt(candidate.config.as_ref()), json_opt(previous.config.as_ref())),
        (
            "group",
            json_opt(candidate.group.map(|b| Value::Bool(b)).as_ref()),
            json_opt(previous.group.map(|b| Value::Bool(b)).as_ref()),
        ),
        ("disabled", json_opt(candidate.disabled.as_ref()), json_opt(previous.disabled.as_ref())),
        ("inject", json_map_opt(candidate.inject.as_ref()), json_map_opt(previous.inject.as_ref())),
        (
            "intercept",
            json_map_opt(candidate.intercept.as_ref()),
            json_map_opt(previous.intercept.as_ref()),
        ),
        ("isolate", json_map_opt(candidate.isolate.as_ref()), json_map_opt(previous.isolate.as_ref())),
    ];
    pairs
        .into_iter()
        .filter(|(_, a, b)| a != b)
        .map(|(key, _, _)| key.to_string())
        .collect()
}

fn json_string(value: &str) -> Value {
    Value::String(value.to_string())
}

fn json_opt(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or(Value::Null)
}

fn json_map_opt<K: serde::Serialize>(value: Option<&IndexMap<String, K>>) -> Value {
    match value {
        Some(map) => serde_json::to_value(map).unwrap_or(Value::Null),
        None => Value::Null,
    }
}
