//! Plugin registry, dependency injection, and plugin entrypoint types.
//!
//! Rust port of `vendor/cordis/src/registry.ts`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use indexmap::IndexMap;

use crate::context::Context;
use crate::error::{PluginError, ValidationError};
use crate::fiber::FiberCore;
use crate::util::{ArcValue, BoxFuture, DisposableList};

/// Callback used by dependency-triggered anonymous plugins.
pub type InjectCallback =
    Arc<dyn Fn(&Context, ArcValue) -> BoxFuture<'static, Result<(), PluginError>> + Send + Sync>;

/// Service dependency declaration accepted by plugins: service name →
/// optional intercept config (`None` = plain requirement).
#[derive(Debug, Clone, Default)]
pub struct InjectSpec {
    pub deps: Vec<(String, Option<ArcValue>)>,
}

impl InjectSpec {
    pub fn new(names: impl IntoIterator<Item = &'static str>) -> Self {
        Self {
            deps: names
                .into_iter()
                .map(|name| (name.to_string(), None))
                .collect(),
        }
    }

    pub fn with_config(names: impl IntoIterator<Item = (&'static str, Option<ArcValue>)>) -> Self {
        Self {
            deps: names.into_iter().map(|(n, c)| (n.to_string(), c)).collect(),
        }
    }

    pub fn as_map(&self) -> IndexMap<String, Option<ArcValue>> {
        self.deps.iter().cloned().collect()
    }
}

/// Supported plugin entrypoint: a single `apply` body.
///
/// TS supports function, class, and `{ apply }` object shapes; Rust unifies
/// them into this trait. The shared runtime is keyed by the `Arc` pointer of
/// the plugin value, mirroring the registry's per-callback runtime record.
#[async_trait::async_trait]
pub trait Plugin: Send + Sync + 'static {
    /// Display name used for fiber diagnostics and logger names.
    fn name(&self) -> Option<&'static str> {
        None
    }

    /// Services the plugin requires; it only loads while all are available.
    fn inject(&self) -> InjectSpec {
        InjectSpec::default()
    }

    /// Schema validation applied to config before the plugin starts.
    fn validate(&self, config: ArcValue) -> Result<ArcValue, ValidationError> {
        Ok(config)
    }

    /// The plugin body, called with `(ctx, config)`.
    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError>;
}

/// Mutable registry record shared by all fibers of one plugin callback.
pub struct PluginRuntime {
    /// Display name copied from the plugin shape.
    pub name: Option<&'static str>,
    /// The executable entrypoint all fibers share (registry identity).
    pub plugin: Arc<dyn Plugin>,
    /// Every live fiber of this plugin (one per `ctx.plugin()` call).
    pub fibers: DisposableList<FiberCore>,
}

impl PluginRuntime {
    pub fn validate(&self, config: ArcValue) -> Result<ArcValue, ValidationError> {
        self.plugin.validate(config)
    }
}

/// Plugin registry installed as `ctx.registry` and mixed into every context.
pub struct RegistryService {
    counter: AtomicU64,
    internal: DashMap<usize, Arc<PluginRuntime>>,
}

impl Default for RegistryService {
    fn default() -> Self {
        Self {
            counter: AtomicU64::new(0),
            internal: DashMap::new(),
        }
    }
}

impl RegistryService {
    /// Allocate the next fiber uid (increments on every read).
    pub fn counter(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Number of registered plugin runtimes.
    pub fn size(&self) -> usize {
        self.internal.len()
    }

    /// Look up the runtime record for a plugin value.
    pub fn get(&self, plugin: &Arc<dyn Plugin>) -> Option<Arc<PluginRuntime>> {
        let key = Arc::as_ptr(plugin) as *const () as usize;
        self.internal.get(&key).map(|entry| entry.clone())
    }

    /// Check whether a plugin has a registered runtime.
    pub fn has(&self, plugin: &Arc<dyn Plugin>) -> bool {
        let key = Arc::as_ptr(plugin) as *const () as usize;
        self.internal.contains_key(&key)
    }

    /// Remove a runtime record once its last fiber has gone (fibers are
    /// disposed by their owning parent before this is called).
    pub fn remove_runtime(&self, runtime: &Arc<PluginRuntime>) {
        let key = Arc::as_ptr(&runtime.plugin) as *const () as usize;
        if let Some((_, current)) = self.internal.remove(&key) {
            let _ = current;
        }
    }

    /// Iterate the registered plugin runtimes.
    pub fn values(&self) -> Vec<Arc<PluginRuntime>> {
        self.internal
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Start a plugin in the current context and return its fiber.
    ///
    /// Creates (or reuses) the plugin's runtime record, then starts a new
    /// fiber under the current context. The returned fiber settles once
    /// loading finished (`fiber.settle()`), rejecting on config or startup
    /// errors — mirroring the thenable fiber of the TS registry.
    pub fn plugin(
        &self,
        parent: &Context,
        plugin: Arc<dyn Plugin>,
        config: ArcValue,
    ) -> Arc<FiberCore> {
        let key = Arc::as_ptr(&plugin) as *const () as usize;
        let runtime = self
            .internal
            .entry(key)
            .or_insert_with(|| {
                Arc::new(PluginRuntime {
                    name: plugin.name(),
                    plugin: plugin.clone(),
                    fibers: DisposableList::default(),
                })
            })
            .clone();
        let inject = plugin.inject().as_map();
        FiberCore::spawn_plugin(parent, config, inject, runtime)
    }

    /// Run a callback once the requested services are available.
    ///
    /// Shorthand for `ctx.plugin({ inject, apply: callback })`: the callback
    /// is unloaded and re-run whenever a required service changes.
    pub fn inject(
        &self,
        parent: &Context,
        deps: InjectSpec,
        callback: InjectCallback,
    ) -> Arc<FiberCore> {
        struct CallbackPlugin {
            name: Option<&'static str>,
            deps: InjectSpec,
            callback: InjectCallback,
        }

        #[async_trait::async_trait]
        impl Plugin for CallbackPlugin {
            fn name(&self) -> Option<&'static str> {
                self.name
            }

            fn inject(&self) -> InjectSpec {
                self.deps.clone()
            }

            async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
                (self.callback)(ctx, config).await
            }
        }

        self.plugin(
            parent,
            Arc::new(CallbackPlugin {
                name: None,
                deps,
                callback,
            }),
            crate::util::arc(()),
        )
    }
}
