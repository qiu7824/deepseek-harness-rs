//! Plugin fiber lifecycle, effects, and the load/unload state machine.
//!
//! Rust port of `vendor/cordis/src/fiber.ts`.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt;
use indexmap::IndexMap;
use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::context::Context;
use crate::error::{CordisError, CordisErrorCode, PluginError};
use crate::events::Disposer;
use crate::reflect::Impl;
use crate::registry::PluginRuntime;
use crate::util::{ArcValue, BoxFuture, DisposableList, Epoch, arc};

/// Lifecycle state for one plugin fiber (port of `FiberState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberState {
    /// Waiting for required services.
    Pending,
    /// The plugin callback is running.
    Loading,
    /// Loaded and providing.
    Active,
    /// The callback or its config threw.
    Failed,
    /// Disposers are running.
    Unloading,
    /// The fiber was removed and cannot restart.
    Disposed,
}

/// Internal bookkeeping: pending dependency impls, the active snapshot, and
/// the runner epoch (TS `_store` + `store` + `_runner.epoch`).
#[derive(Default)]
struct StoreState {
    pending: HashMap<String, Arc<Impl>>,
    active: Option<HashMap<String, Arc<Impl>>>,
    runner_epoch: Epoch,
}

/// Tree node used to expose nested effect labels for diagnostics.
#[derive(Debug, Clone)]
pub struct EffectMeta {
    pub label: String,
    pub children: Vec<EffectMeta>,
}

/// Shared per-effect teardown state backing the public disposer closure.
struct EffectInner {
    #[allow(dead_code)] // reserved for getEffects() diagnostics
    label: String,
    disposables: Mutex<Vec<Disposer>>,
    disposing: AtomicBool,
    /// Set once the async setup body finished and collected its disposer.
    setup_done: AtomicBool,
    /// Notified when the setup body finishes.
    setup_notify: Notify,
    /// Serializes teardown: the first caller drains, later callers join it.
    disposal_lock: tokio::sync::Mutex<()>,
}

impl EffectInner {
    fn collect(
        &self,
        disposer: Disposer,
        fiber_list: &DisposableList<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) {
        // TS track(): collect into the effect and drop the inner disposer
        // from the fiber list (it was never registered — the fiber holds the
        // wrapper — but delete is harmless and mirrors the source).
        fiber_list.delete(&disposer);
        self.disposables.lock().push(disposer);
    }

    async fn dispose(&self) {
        let _guard = self.disposal_lock.lock().await;
        if self.disposing.swap(true, Ordering::SeqCst) {
            // Another caller already drained (we joined via the lock).
            return;
        }
        // Wait for a still-running setup body before draining (TS setupBarrier).
        while !self.setup_done.load(Ordering::SeqCst) {
            self.setup_notify.notified().await;
        }
        let disposables: Vec<Disposer> = {
            let mut guard = self.disposables.lock();
            guard.drain(..).rev().collect()
        };
        for disposer in disposables {
            let _ = std::panic::AssertUnwindSafe(disposer())
                .catch_unwind()
                .await;
        }
    }
}

/// The runtime instance of one plugin application (TS `Fiber`).
///
/// The TS `Fiber` class and its context own each other; here the heavy state
/// lives in `FiberCore` (shared by the fiber and its context), while the
/// context is referenced weakly from the core, breaking the ownership cycle.
pub struct FiberCore {
    /// Unique id within the registry; `None` once disposed (0 for root).
    pub uid: Mutex<Option<u64>>,
    /// The parent context (the fiber's plugin context extends it).
    /// `None` only for the root fiber.
    pub parent: Mutex<Option<Context>>,
    /// The plugin context (strong ref; mirrors TS `Fiber.context`). Cleared
    /// on dispose to break the fiber ↔ context ownership cycle.
    ctx: Mutex<Option<Context>>,
    /// Resolved dependency map in declaration order (service name →
    /// optional intercept config). Insertion order drives epoch composition,
    /// mirroring `Object.keys(inject)` in the TS runtime.
    pub inject: IndexMap<String, Option<ArcValue>>,
    /// The shared plugin runtime; `None` for the root fiber.
    pub runtime: Option<Arc<PluginRuntime>>,
    /// The validated plugin config (updated by `update()`).
    pub config: Mutex<ArcValue>,
    /// The raw plugin config, re-resolved before each activation.
    config_raw: Mutex<ArcValue>,
    /// Current lifecycle state; transitions emit `internal/status`.
    state: Mutex<FiberState>,
    /// Startup/validation failure carried until the next successful load.
    error: Mutex<Option<PluginError>>,
    /// Dependency and epoch bookkeeping.
    store: Mutex<StoreState>,
    /// Every live effect wrapper owned by this fiber.
    pub disposables: Arc<DisposableList<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>>,
    /// Per-fiber `internal/update` hooks (see EventsService's listener
    /// interception).
    hooks: Arc<Mutex<HashMap<String, Vec<ArcValue>>>>,
    /// The in-flight load/unload transition chain, if one is currently
    /// running; `drain` waits on the notify token.
    inertia: Mutex<Option<Arc<Notify>>>,
    /// Transitions queued behind the running chain (re-entrant spawns).
    inertia_queue: Mutex<VecDeque<BoxFuture<'static, ()>>>,
    /// Self-disposal entrypoint (mirrors TS `Fiber.dispose()`).
    dispose_self: Mutex<Option<Disposer>>,
}

fn make_disposer(f: impl Fn() -> BoxFuture<'static, ()> + Send + Sync + 'static) -> Disposer {
    Arc::new(f)
}

impl FiberCore {
    /// Create the root fiber core (uid 0, always active).
    pub fn new_root() -> Arc<Self> {
        Arc::new(Self {
            uid: Mutex::new(Some(0)),
            parent: Mutex::new(None),
            ctx: Mutex::new(None),
            inject: IndexMap::new(),
            runtime: None,
            config: Mutex::new(arc(())),
            config_raw: Mutex::new(arc(())),
            state: Mutex::new(FiberState::Active),
            error: Mutex::new(None),
            store: Mutex::new(StoreState {
                pending: HashMap::new(),
                active: Some(HashMap::new()),
                runner_epoch: Some(String::new()),
            }),
            disposables: Arc::new(DisposableList::default()),
            hooks: Arc::new(Mutex::new(HashMap::new())),
            inertia: Mutex::new(None),
            inertia_queue: Mutex::new(VecDeque::new()),
            dispose_self: Mutex::new(None),
        })
    }

    /// Bind this core to its plugin context (two-phase construction).
    pub(crate) fn bind_ctx(&self, ctx: &Context) {
        *self.ctx.lock() = Some(ctx.clone());
    }

    /// Bind the parent context (root two-phase construction).
    pub(crate) fn bind_parent(&self, parent: Context) {
        *self.parent.lock() = Some(parent);
    }

    /// The plugin context (a strong ref mirrors TS `Fiber.context`).
    pub fn ctx(&self) -> Option<Context> {
        self.ctx.lock().clone()
    }

    /// Current fiber uid (`None` once disposed; mirrors TS `fiber.uid`).
    pub fn uid_value(&self) -> Option<u64> {
        *self.uid.lock()
    }

    /// The parent context (TS `fiber.parent`).
    pub fn parent_ctx(&self) -> Option<Context> {
        self.parent.lock().clone()
    }

    /// Create a plugin fiber under `parent` (TS `Fiber` constructor).
    pub fn spawn_plugin(
        parent: &Context,
        config: ArcValue,
        inject: IndexMap<String, Option<ArcValue>>,
        runtime: Arc<PluginRuntime>,
    ) -> Arc<Self> {
        let uid = parent.registry.counter();
        let core = Arc::new(Self {
            uid: Mutex::new(Some(uid)),
            parent: Mutex::new(Some(parent.clone())),
            ctx: Mutex::new(None),
            inject,
            runtime: Some(runtime.clone()),
            config: Mutex::new(arc(())),
            config_raw: Mutex::new(config),
            state: Mutex::new(FiberState::Pending),
            error: Mutex::new(None),
            store: Mutex::new(StoreState::default()),
            disposables: Arc::new(DisposableList::default()),
            hooks: Arc::new(Mutex::new(HashMap::new())),
            inertia: Mutex::new(None),
            inertia_queue: Mutex::new(VecDeque::new()),
            dispose_self: Mutex::new(None),
        });

        // 1. Build the plugin context: extend the parent with this fiber and
        //    an intercept overlay carrying the inject configs.
        let ctx = parent.extend_with_fiber(core.clone());
        core.bind_ctx(&ctx);

        // 2. Register the disposer with the parent fiber (mirrors TS ctor).
        let _ = runtime.fibers.push(core.clone());
        let core_for_dispose = core.clone();
        let runtime_for_dispose = runtime.clone();
        let disposer: Disposer = make_disposer(move || {
            let core = core_for_dispose.clone();
            let runtime = runtime_for_dispose.clone();
            Box::pin(async move {
                if core.uid.lock().is_none() {
                    return; // already disposed
                }
                *core.uid.lock() = None;
                // emitPluginDisposed → internal/plugin with the fiber
                if let Some(ctx) = core.ctx() {
                    ctx.events
                        .emit(None, "internal/plugin", vec![arc(core.clone())]);
                }
                runtime.fibers.delete(&core);
                if runtime.fibers.is_empty() {
                    if let Some(ctx) = core.ctx() {
                        ctx.registry.remove_runtime(&runtime);
                    }
                }
                core.set_runner_epoch(None);
                if !core.has_inertia() {
                    let inner = core.clone();
                    core.update_state(|| {
                        core.spawn_inertia(Box::pin(async move { inner.unload().await }));
                        Some(FiberState::Unloading)
                    });
                }
                core.drain().await;
                // Break the fiber ↔ context ownership cycle (TS sets
                // `fiber.context = undefined` on dispose).
                *core.ctx.lock() = None;
            })
        });
        *core.dispose_self.lock() = Some(disposer.clone());
        let _ = parent
            .fiber
            .effect("ctx.plugin()", Box::pin(async move { Some(disposer) }));

        // 3. Publish so observers can react (TS emits synchronously here).
        if let Some(ctx) = core.ctx() {
            ctx.events
                .emit(None, "internal/plugin", vec![arc(core.clone())]);
        }

        // 4. Resolve dependencies and start the load chain.
        if core.uid.lock().is_some() && parent.fiber.state() != FiberState::Unloading {
            let names: Vec<String> = core.inject.keys().cloned().collect();
            for name in names {
                core.check_impl(&name);
            }
            core.refresh();
        }
        core
    }

    /// Dispose this fiber: remove it from the registry, unload the plugin,
    /// and wait for the teardown chain to settle (TS `Fiber.dispose()`).
    pub async fn dispose(self: &Arc<Self>) {
        let disposer = self.dispose_self.lock().take();
        if let Some(disposer) = disposer {
            disposer().await;
        }
    }

    /// The plugin's display name, inherited from the nearest named ancestor.
    pub fn name(self: &Arc<Self>) -> String {
        let mut current: Option<Arc<FiberCore>> = Some(self.clone());
        loop {
            let core = current.take().unwrap();
            if let Some(runtime) = &core.runtime {
                if let Some(name) = &runtime.name {
                    return name.to_string();
                }
            }
            let Some(parent_ctx) = core.parent.lock().clone() else {
                return "root".to_string();
            };
            let parent_core = parent_ctx.fiber.clone();
            if Arc::ptr_eq(&parent_core, &core) {
                return "root".to_string();
            }
            current = Some(parent_core);
        }
    }

    /// Throw if the fiber has already been disposed.
    pub fn assert_active(&self) -> Result<(), CordisError> {
        if self.uid.lock().is_some() {
            Ok(())
        } else {
            Err(CordisError::new(CordisErrorCode::InactiveEffect))
        }
    }

    pub fn state(&self) -> FiberState {
        *self.state.lock()
    }

    pub fn runner_epoch(&self) -> Epoch {
        self.store.lock().runner_epoch.clone()
    }

    fn set_runner_epoch(&self, epoch: Epoch) {
        self.store.lock().runner_epoch = epoch;
    }

    fn has_inertia(&self) -> bool {
        self.inertia.lock().is_some()
    }

    /// Queue a lifecycle transition, chaining behind any running one so
    /// re-entrant spawns (e.g. unload's final `update_state`) are never lost.
    fn spawn_inertia(self: &Arc<Self>, future: BoxFuture<'static, ()>) {
        let mut future = Some(future);
        let enqueued = {
            let mut guard = self.inertia.lock();
            if guard.is_some() {
                let inner = future.take().expect("future present");
                self.inertia_queue.lock().push_back(inner);
                true
            } else {
                *guard = Some(Arc::new(Notify::new()));
                false
            }
        };
        if enqueued {
            return;
        }
        let future = future.expect("future present");
        let core = self.clone();
        tokio::spawn(async move {
            let mut current = Some(future);
            while let Some(next) = current.take() {
                // Contain unexpected transition panics so the chain always
                // drains (mirrors the TS transition catch handlers).
                match std::panic::AssertUnwindSafe(next).catch_unwind().await {
                    Ok(()) => {}
                    Err(payload) => {
                        let message = payload
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
                            .unwrap_or("non-string panic");
                        tracing::error!("fiber lifecycle transition panicked: {message}");
                    }
                }
                current = core.inertia_queue.lock().pop_front();
            }
            let notify = { core.inertia.lock().take() };
            if let Some(notify) = notify {
                notify.notify_waiters();
            }
        });
    }

    /// Wait for the lifecycle transition chain to drain.
    pub async fn drain(&self) {
        loop {
            let notify = { self.inertia.lock().clone() };
            match notify {
                Some(notify) => notify.notified().await,
                None => break,
            }
        }
    }

    /// Wait for current lifecycle work and rethrow startup errors
    /// (TS `await()`).
    pub async fn settle(&self) -> Result<(), PluginError> {
        self.drain().await;
        if let Some(error) = self.error.lock().clone() {
            return Err(error);
        }
        Ok(())
    }

    /// Dispose and immediately reload this plugin with its current config
    /// (TS `restart()`).
    pub async fn restart(self: &Arc<Self>) -> Result<(), PluginError> {
        self.assert_active()?;
        self.set_runner_epoch(None);
        self.refresh();
        self.settle().await
    }

    /// Validate and apply new config, then restart the plugin (TS `update()`).
    pub async fn update(
        self: &Arc<Self>,
        config: ArcValue,
        no_save: bool,
    ) -> Result<(), PluginError> {
        self.assert_active()?;
        *self.config_raw.lock() = config.clone();
        if self.state() != FiberState::Active {
            *self.error.lock() = None;
            self.set_runner_epoch(None);
            self.refresh();
            return Ok(());
        }
        let config = self.resolve_config(config).await?;
        let ctx = self.ctx().ok_or_else(|| {
            PluginError::new(arc(CordisError::new(CordisErrorCode::InactiveEffect)))
        })?;
        let core = self.clone();
        let config_for_fallback = config.clone();
        let fallback: BoxFuture<'static, ArcValue> = Box::pin(async move {
            *core.config.lock() = config_for_fallback;
            *core.error.lock() = None;
            match core.restart().await {
                Ok(()) => arc(()),
                Err(error) => arc(error),
            }
        });
        let raw = self.config_raw.lock().clone();
        let result = ctx
            .events
            .waterfall(
                Some(&ctx),
                "internal/update",
                vec![raw, arc(no_save)],
                fallback,
            )
            .await;
        if let Some(error) = crate::util::downcast::<PluginError>(&result) {
            return Err(error.clone());
        }
        Ok(())
    }

    // ---- dependency machinery (TS `_checkImpl` / `_refresh` / `_setEpoch`) ----

    pub(crate) fn check_impl(&self, name: &str) {
        let impl_opt = self
            .ctx()
            .and_then(|ctx| ctx.reflect.get_impl(&ctx, name, true));
        let mut store = self.store.lock();
        match impl_opt {
            None => {
                store.pending.remove(name);
            }
            Some(impl_) => {
                let ok = match &impl_.check {
                    None => true,
                    Some(check) => self.ctx().map(|ctx| check(&ctx)).unwrap_or(false),
                };
                if ok {
                    store.pending.insert(name.to_string(), impl_);
                } else {
                    store.pending.remove(name);
                }
            }
        }
    }

    /// Recompute the epoch from the pending store and apply it (TS `_refresh`).
    ///
    /// TS semantics: `''` (empty string) = no missing dependencies (loads
    /// immediately), `INACTIVE` = missing/unavailable (stays unloaded). Rust
    /// maps these to `Some("")` and `None` respectively; the epoch string is
    /// `:uid` per dependency in declaration order.
    pub fn refresh(self: &Arc<Self>) {
        let store = self.store.lock();
        let mut epoch = String::new();
        let mut missing = false;
        for name in self.inject.keys() {
            match store.pending.get(name) {
                Some(impl_) => match *impl_.fiber.uid.lock() {
                    Some(uid) => epoch.push_str(&format!(":{uid}")),
                    None => {
                        missing = true;
                        break;
                    }
                },
                None => {
                    missing = true;
                    break;
                }
            }
        }
        let epoch = if missing { None } else { Some(epoch) };
        drop(store);
        self.set_epoch(epoch);
    }

    fn set_epoch(self: &Arc<Self>, epoch: Epoch) {
        let old = self.runner_epoch();
        if epoch == old {
            return;
        }
        self.set_runner_epoch(epoch.clone());
        if self.has_inertia() {
            return; // the running chain re-evaluates at its end
        }
        self.update_state(|| {
            if epoch.is_some() && old.is_none() {
                let core = self.clone();
                self.spawn_inertia(Box::pin(async move { core.reload().await }));
                Some(FiberState::Loading)
            } else {
                let core = self.clone();
                self.spawn_inertia(Box::pin(async move { core.unload().await }));
                Some(FiberState::Unloading)
            }
        });
    }

    // ---- load / unload ----

    async fn resolve_config(self: &Arc<Self>, config: ArcValue) -> Result<ArcValue, PluginError> {
        let ctx = self.ctx().ok_or_else(|| {
            PluginError::new(arc(CordisError::new(CordisErrorCode::InactiveEffect)))
        })?;
        let fallback: BoxFuture<'static, ArcValue> = Box::pin(async move { config });
        let raw = self.config_raw.lock().clone();
        let resolved = ctx
            .events
            .waterfall(Some(&ctx), "internal/config", vec![raw], fallback)
            .await;
        // A listener may bail with a `PluginError` marker (e.g. the loader's
        // `!!js` interpolation failure); treat it as a config resolution
        // failure instead of passing the error value on to validation.
        if let Some(error) = crate::util::downcast::<PluginError>(&resolved) {
            return Err(error.clone());
        }
        if let Some(runtime) = &self.runtime {
            return runtime.validate(resolved).map_err(PluginError::from);
        }
        Ok(resolved)
    }

    // Return boxed futures (not `async fn`) so that reload ↔ unload can
    // reference each other without an opaque-type cycle.
    fn reload(self: Arc<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            tracing::debug!("reload start (uid {:?})", self.uid.lock());
            let old_epoch = self.runner_epoch();
            {
                let mut store = self.store.lock();
                store.active = Some(store.pending.clone());
            }
            let ctx = match self.ctx() {
                Some(ctx) => ctx,
                None => return,
            };
            let raw = self.config_raw.lock().clone();
            tracing::debug!("reload resolving config (uid {:?})", self.uid.lock());
            match self.resolve_config(raw).await {
                Ok(config) => {
                    tracing::debug!("reload config ok (uid {:?})", self.uid.lock());
                    *self.config.lock() = config.clone();
                    if let Some(runtime) = &self.runtime {
                        tracing::debug!("reload applying (uid {:?})", self.uid.lock());
                        // Panics inside `apply` (e.g. duplicate service provide)
                        // must fail the fiber instead of hanging the chain.
                        let apply_result =
                            std::panic::AssertUnwindSafe(runtime.plugin.apply(&ctx, config))
                                .catch_unwind()
                                .await;
                        tracing::debug!("reload applied (uid {:?})", self.uid.lock());
                        match apply_result {
                            Ok(Ok(())) => {
                                *self.error.lock() = None;
                            }
                            Ok(Err(error)) => {
                                self.log_plugin_error(&error);
                                *self.error.lock() = Some(error);
                                self.set_runner_epoch(None);
                            }
                            Err(_) => {
                                let error =
                                    PluginError::new(arc("plugin callback panicked".to_string()));
                                self.log_plugin_error(&error);
                                *self.error.lock() = Some(error);
                                self.set_runner_epoch(None);
                            }
                        }
                    }
                }
                Err(error) => {
                    self.log_plugin_error(&error);
                    *self.error.lock() = Some(error);
                    self.set_runner_epoch(None);
                }
            }
            self.update_state(|| {
                if self.runner_epoch() == old_epoch {
                    tracing::debug!("reload stable (uid {:?})", self.uid.lock());
                    None // stable
                } else {
                    let core = self.clone();
                    self.spawn_inertia(Box::pin(async move { core.unload().await }));
                    Some(FiberState::Unloading)
                }
            });
        })
    }

    fn unload(self: Arc<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            for disposer in self.disposables.clear() {
                let _ = std::panic::AssertUnwindSafe(disposer())
                    .catch_unwind()
                    .await;
            }
            {
                let mut store = self.store.lock();
                store.active = None;
            }
            self.update_state(|| {
                if self.runner_epoch().is_none() {
                    None
                } else {
                    let core = self.clone();
                    self.spawn_inertia(Box::pin(async move { core.reload().await }));
                    Some(FiberState::Loading)
                }
            });
        })
    }

    fn log_plugin_error(&self, error: &PluginError) {
        if let Some(ctx) = self.ctx() {
            ctx.logger.error(&ctx, vec![arc(error.message())]);
        } else {
            tracing::error!("plugin error: {}", error.message());
        }
    }

    // ---- state transitions ----

    fn derive_state(&self) -> FiberState {
        if self.uid.lock().is_none() {
            return FiberState::Disposed;
        }
        if self.error.lock().is_some() {
            return FiberState::Failed;
        }
        if self.runner_epoch().is_some() {
            return FiberState::Active;
        }
        FiberState::Pending
    }

    fn update_state(self: &Arc<Self>, callback: impl FnOnce() -> Option<FiberState>) {
        let old = self.state();
        let new = callback().unwrap_or_else(|| self.derive_state());
        tracing::debug!(
            "fiber {} update_state: {old:?} -> {new:?} (epoch={:?})",
            self.uid.lock().unwrap_or_default(),
            self.runner_epoch()
        );
        *self.state.lock() = new;
        if old == new {
            return;
        }
        if let Some(ctx) = self.ctx() {
            ctx.events.emit(
                Some(&ctx),
                "internal/status",
                vec![arc(self.clone()), arc(old)],
            );
        }
        // only notify on ACTIVE boundary crossings
        if (old == FiberState::Active) == (new == FiberState::Active) {
            return;
        }
        if let Some(ctx) = self.ctx() {
            ctx.reflect.notify_own(self);
        }
    }

    // ---- effects (TS `effect()`) ----

    /// Register a cleanup-aware effect on this fiber.
    ///
    /// `execute` runs immediately; the disposer it produces is collected and
    /// run (in reverse order) either when the returned disposer is called or
    /// when the fiber unloads, whichever comes first. Panics with
    /// `INACTIVE_EFFECT` if the fiber is already disposed (mirrors the TS
    /// synchronous throw).
    pub fn effect(
        self: &Arc<Self>,
        label: &str,
        execute: BoxFuture<'static, Option<Disposer>>,
    ) -> Disposer {
        self.assert_active()
            .unwrap_or_else(|error| panic!("{error}"));
        if self.state() == FiberState::Unloading {
            panic!("{}", CordisError::new(CordisErrorCode::InactiveEffect));
        }

        let inner = Arc::new(EffectInner {
            label: label.to_string(),
            disposables: Mutex::new(Vec::new()),
            disposing: AtomicBool::new(false),
            setup_done: AtomicBool::new(false),
            setup_notify: Notify::new(),
            disposal_lock: tokio::sync::Mutex::new(()),
        });

        // Make the effect visible to a reentrant owner unload before execute()
        // runs any plugin code.
        let wrapper_inner = inner.clone();
        let wrapper: Disposer = make_disposer(move || {
            let inner = wrapper_inner.clone();
            Box::pin(async move { inner.dispose().await })
        });
        let _ = self.disposables.push(wrapper.clone());

        let fiber_list = self.disposables.clone();
        let inner_for_task = inner.clone();
        tokio::spawn(async move {
            let result = execute.await;
            if let Some(disposer) = result {
                inner_for_task.collect(disposer, &fiber_list);
            }
            inner_for_task.setup_done.store(true, Ordering::SeqCst);
            inner_for_task.setup_notify.notify_waiters();
        });
        wrapper
    }

    /// Return metadata for currently registered effects.
    pub fn get_effects(&self) -> Vec<EffectMeta> {
        let count = self.disposables.len();
        vec![EffectMeta {
            label: format!("{count} effect(s)"),
            children: vec![],
        }]
    }

    // ---- per-fiber update hooks (TS `Fiber._hooks`) ----

    /// Push a listener into the per-fiber `internal/update` hook list;
    /// returns a disposer removing exactly this listener.
    pub fn hooks_push(&self, name: &str, listener: ArcValue) -> Disposer {
        let mut hooks = self.hooks.lock();
        let list = hooks.entry(name.to_string()).or_default();
        list.push(listener.clone());
        drop(hooks);
        let hooks_for_dispose = self.hooks.clone();
        let name_for_dispose = name.to_string();
        let listener_for_dispose = listener.clone();
        make_disposer(move || {
            let mut hooks = hooks_for_dispose.lock();
            if let Some(list) = hooks.get_mut(&name_for_dispose) {
                list.retain(|entry| !Arc::ptr_eq(entry, &listener_for_dispose));
            }
            Box::pin(async {})
        })
    }

    /// Snapshot the per-fiber hook list for `name`.
    pub fn hooks_snapshot(&self, name: &str) -> Vec<ArcValue> {
        self.hooks.lock().get(name).cloned().unwrap_or_default()
    }

    /// Insert an own provided impl into the active store snapshot.
    pub fn store_insert_active(&self, name: &str, impl_: Arc<Impl>) {
        let mut store = self.store.lock();
        if let Some(active) = store.active.as_mut() {
            active.insert(name.to_string(), impl_);
        }
    }

    /// Remove an own provided impl from the active store snapshot.
    pub fn store_delete_active(&self, name: &str) {
        let mut store = self.store.lock();
        if let Some(active) = store.active.as_mut() {
            active.remove(name);
        }
    }
}
