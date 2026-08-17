//! Loader service core (port of `src/index.ts`): plugin registry, realms,
//! fiber→entry bookkeeping, and the cordis event wiring.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, FiberCore, Plugin, PluginError, Service, arc};
use parking_lot::Mutex;

use crate::entry::Entry;
use crate::group::GroupPlugin;
use crate::isolate::GlobalRealm;
use crate::utils::interpolate;

/// Loader failure (port of the TS `updateError`/`AggregateError` flows).
#[derive(Debug, Clone)]
pub enum LoaderError {
    /// A loader entry operation failed at `stage`.
    Update {
        stage: &'static str,
        id: String,
        name: String,
        message: String,
    },
    /// A `!!js` expression cannot be evaluated yet.
    UnsupportedJs(String),
    /// A plugin specifier cannot be resolved.
    Import(String),
    /// Aggregate of several failures (mirrors the TS `AggregateError`).
    Aggregate(Vec<LoaderError>),
}

impl LoaderError {
    pub fn update(stage: &'static str, id: &str, name: &str, message: impl Into<String>) -> Self {
        Self::Update {
            stage,
            id: id.to_string(),
            name: name.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for LoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoaderError::Update {
                stage,
                id,
                name,
                message,
            } => {
                write!(f, "failed to {stage} loader entry {id} ({name}): {message}")
            }
            LoaderError::UnsupportedJs(message) => write!(f, "{message}"),
            LoaderError::Import(message) => write!(f, "{message}"),
            LoaderError::Aggregate(errors) => {
                write!(f, "loader entries failed ({} errors)", errors.len())?;
                for error in errors {
                    write!(f, "\n- {error}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LoaderError {}

/// Shared runtime state behind the loader service (registry + realms +
/// fiber bookkeeping).
pub struct LoaderCore {
    /// Static plugin registry: `name` (minus any `cordis:` prefix) → plugin.
    pub builtins: Mutex<HashMap<String, Arc<dyn Plugin>>>,
    /// Named isolation realms shared across entries.
    pub global_realms: Mutex<HashMap<String, Arc<GlobalRealm>>>,
    /// Loader entries owning a live fiber, keyed by the fiber `Arc` pointer
    /// (TS `fiber.entry` back-reference).
    pub entries_by_fiber: Mutex<HashMap<usize, Arc<Entry>>>,
    /// Fibers whose configs stay literal (group carriers; TS checks the
    /// `EntryGroup.key` marker on the plugin).
    pub carrier_fibers: Mutex<HashSet<usize>>,
}

impl LoaderCore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            builtins: Mutex::new(HashMap::new()),
            global_realms: Mutex::new(HashMap::new()),
            entries_by_fiber: Mutex::new(HashMap::new()),
            carrier_fibers: Mutex::new(HashSet::new()),
        })
    }

    /// Register a plugin under a loader entry name.
    pub fn register(self: &Arc<Self>, name: &str, plugin: Arc<dyn Plugin>) {
        self.builtins.lock().insert(name.to_string(), plugin);
    }

    /// Resolve a plugin specifier (TS `EntryTree.import`; dynamic module
    /// imports are replaced by this static registry).
    pub fn import(&self, name: &str) -> Result<Arc<dyn Plugin>, LoaderError> {
        let key = name.strip_prefix("cordis:").unwrap_or(name);
        self.builtins
            .lock()
            .get(key)
            .cloned()
            .ok_or_else(|| LoaderError::Import(format!("cannot resolve plugin \"{name}\"")))
    }

    /// The named global realm, creating it on first use.
    pub fn global_realm(&self, label: &str) -> Arc<GlobalRealm> {
        let mut realms = self.global_realms.lock();
        realms
            .entry(label.to_string())
            .or_insert_with(|| GlobalRealm::new(label.to_string()))
            .clone()
    }

    fn fiber_key(fiber: &Arc<FiberCore>) -> usize {
        Arc::as_ptr(fiber) as *const () as usize
    }

    /// Register the fiber→entry association (TS sets `fiber.entry`).
    pub fn track_fiber(&self, fiber: &Arc<FiberCore>, entry: Arc<Entry>) {
        self.entries_by_fiber
            .lock()
            .insert(Self::fiber_key(fiber), entry);
    }

    /// Drop the fiber→entry association (entry dispose).
    pub fn untrack_fiber(&self, fiber: &Arc<FiberCore>) {
        self.entries_by_fiber.lock().remove(&Self::fiber_key(fiber));
        self.carrier_fibers.lock().remove(&Self::fiber_key(fiber));
    }

    /// Mark a fiber as a config carrier (group entries; TS
    /// `plugin[EntryGroup.key]`).
    pub fn mark_carrier(&self, fiber: &Arc<FiberCore>) {
        self.carrier_fibers.lock().insert(Self::fiber_key(fiber));
    }

    fn is_carrier(&self, fiber: &Arc<FiberCore>) -> bool {
        self.carrier_fibers.lock().contains(&Self::fiber_key(fiber))
    }

    /// Look up the entry owning a fiber (TS `fiber.entry`).
    pub fn entry_of(&self, fiber: &Arc<FiberCore>) -> Option<Arc<Entry>> {
        self.entries_by_fiber
            .lock()
            .get(&Self::fiber_key(fiber))
            .cloned()
    }
}

/// The `ctx.loader` service (TS `Loader extends EntryTree`; the tree lives
/// beside the core in [`LoaderService`]).
pub struct LoaderService {
    pub core: Arc<LoaderCore>,
    /// Root entry tree owned by this loader.
    pub tree: Arc<crate::tree::EntryTree>,
    /// `enableLogs` flag (TS `EntryTree.enableLogs`).
    pub enable_logs: bool,
}

impl LoaderService {
    /// Create the service and wire the `internal/*` hooks on `ctx`.
    pub async fn new(ctx: &Context) -> Arc<Self> {
        let core = LoaderCore::new();
        let tree = crate::tree::EntryTree::new(ctx.clone(), core.clone());
        let service = Arc::new(Self {
            core: core.clone(),
            tree,
            enable_logs: true,
        });

        // Built-in plugin: nested entry groups.
        core.register("group", Arc::new(GroupPlugin { core: core.clone() }));

        // internal/config: interpolate configs for fibers owned by entries
        // (skip carriers whose configs hold other rows' expressions). TS
        // calls `next()` first and always returns a value.
        {
            let core = core.clone();
            ctx.on(
                "internal/config",
                Arc::new(move |ctx: &Context, args: Vec<ArcValue>| {
                    let ctx = ctx.clone();
                    let core = core.clone();
                    Box::pin(async move {
                        let next = args
                            .last()
                            .and_then(|v| cordis::downcast::<cordis::NextFn>(v));
                        let result = match next {
                            Some(next) => next.call().await,
                            None => arc(()),
                        };
                        let Some(config) = args.first() else {
                            return Some(result);
                        };
                        let Some(raw) = cordis::downcast::<serde_json::Value>(config) else {
                            return Some(result);
                        };
                        let fiber = &ctx.fiber;
                        if core.is_carrier(fiber) || core.entry_of(fiber).is_none() {
                            return Some(result);
                        }
                        match interpolate(raw) {
                            Ok(interpolated) => Some(arc(interpolated)),
                            Err(error) => Some(arc(PluginError::new(arc(error.to_string())))),
                        }
                    })
                }),
                EventOptions::default().global(true),
            )
            .await;
        }

        // internal/update (prepend): persist config write-back after updates.
        {
            let core = core.clone();
            let tree = service.tree.clone();
            ctx.on(
                "internal/update",
                Arc::new(move |ctx: &Context, args: Vec<ArcValue>| {
                    let ctx = ctx.clone();
                    let core = core.clone();
                    let tree = tree.clone();
                    Box::pin(async move {
                        let Some(next) = args
                            .last()
                            .and_then(|v| cordis::downcast::<cordis::NextFn>(v))
                        else {
                            return None;
                        };
                        let result = next.call().await;
                        let no_save = args
                            .get(1)
                            .and_then(|value| cordis::downcast::<bool>(value))
                            .copied()
                            .unwrap_or(false);
                        if no_save {
                            return Some(result);
                        }
                        if let Some(entry) = core.entry_of(&ctx.fiber) {
                            if let Some(config) = args.first() {
                                if let Some(raw) = cordis::downcast::<serde_json::Value>(config) {
                                    entry.options.lock().config = Some(raw.clone());
                                }
                            }
                            tree.write();
                        }
                        Some(result)
                    })
                }),
                EventOptions::default().global(true).prepend(true),
            )
            .await;
        }

        // internal/plugin: detect entry self-dispose (fiber.uid becomes None).
        {
            let core = core.clone();
            ctx.on(
                "internal/plugin",
                Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
                    let core = core.clone();
                    Box::pin(async move {
                        let Some(fiber_value) = args.first() else {
                            return None;
                        };
                        let Some(fiber_arc) = cordis::downcast::<Arc<FiberCore>>(fiber_value)
                        else {
                            return None;
                        };
                        let fiber: Arc<FiberCore> = (*fiber_arc).clone();
                        // Only self-dispose events carry uid == None.
                        if fiber.uid_value().is_some() {
                            return None;
                        }
                        let Some(entry) = core.entry_of(&fiber) else {
                            return None;
                        };
                        if entry.disposing() {
                            return None;
                        }
                        let current = entry.fiber.lock().clone();
                        if current.as_ref().is_none_or(|f| !Arc::ptr_eq(f, &fiber)) {
                            return None;
                        }
                        entry.options.lock().disabled = Some(serde_json::Value::Bool(true));
                        let Some(parent) = entry.parent.lock().clone() else {
                            return None;
                        };
                        parent.tree.write();
                        None
                    })
                }),
                EventOptions::default().global(true),
            )
            .await;
        }

        service
    }

    /// Look up the loader entry id owning `fiber`, if any (TS `locate`).
    pub fn locate(&self, fiber: &Arc<FiberCore>) -> Option<String> {
        let mut current: Option<Arc<FiberCore>> = Some(fiber.clone());
        while let Some(fiber) = current.take() {
            if let Some(entry) = self.core.entry_of(&fiber) {
                return Some(entry.id());
            }
            let parent_ctx = fiber.parent_ctx();
            let next = parent_ctx.map(|ctx| ctx.fiber.clone());
            if next.as_ref().is_some_and(|n| Arc::ptr_eq(n, &fiber)) {
                return None;
            }
            current = next;
        }
        None
    }

    /// Show an apply/reload/unload log line (TS `showLog`).
    pub fn show_log(&self, entry: &Entry, kind: &str) {
        if entry.options.lock().group.unwrap_or(false) || !self.enable_logs {
            return;
        }
        let name = entry.options.lock().name.clone();
        tracing::info!("loader: {kind} plugin {name}");
    }
}

impl Service for LoaderService {
    fn service_name(&self) -> &'static str {
        "loader"
    }
}

/// Loader plugin entrypoint (`export default Loader` in TS).
pub fn plugin() -> Arc<dyn Plugin> {
    Arc::new(LoaderPlugin)
}

struct LoaderPlugin;

#[async_trait::async_trait]
impl Plugin for LoaderPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("loader")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        ctx.register_service(LoaderService::new(ctx).await);
        Ok(())
    }
}
