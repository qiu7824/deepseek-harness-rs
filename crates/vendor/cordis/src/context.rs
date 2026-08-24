//! Root and child dependency containers for Cordis plugins.
//!
//! Rust port of `vendor/cordis/src/context.ts`. The TS version is a Proxy;
//! Rust exposes the same operations as inherent methods and keeps named
//! services in a dynamic `Arc<dyn Any>` store.

use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use crate::events::{DispatchMode, Disposer, EventOptions, EventsService, Listener};
use crate::fiber::FiberCore;
use crate::logger::LoggerService;
use crate::reflect::{Accessor, ReflectService};
use crate::registry::{InjectCallback, InjectSpec, Plugin, RegistryService};
use crate::util::{ArcValue, BoxFuture, OverlayMap, arc};

static NEXT_ISOLATION_LABEL: AtomicU64 = AtomicU64::new(1);

pub type ContextFilter = Arc<dyn Fn(&Context) -> bool + Send + Sync>;

/// Allocate a fresh isolation label from the shared process-wide namespace.
/// Consumers that mint their own scope labels (the loader's realms) must use
/// this allocator so labels never collide with cordis-internal ones.
pub fn allocate_isolation_label() -> u64 {
    NEXT_ISOLATION_LABEL.fetch_add(1, Ordering::Relaxed)
}

/// Root and child dependency containers for Cordis plugins.
#[derive(Clone)]
pub struct Context {
    pub(crate) inner: Arc<ContextInner>,
}

/// Concrete context data. Public built-ins mirror the TS `ctx.fiber`,
/// `ctx.reflect`, `ctx.registry`, `ctx.events`, and `ctx.logger` properties.
pub struct ContextInner {
    root: OnceLock<Context>,
    /// The fiber that owns this context.
    pub fiber: Arc<FiberCore>,
    /// Reflection/service-resolution layer.
    pub reflect: Arc<ReflectService>,
    /// Plugin registry.
    pub registry: Arc<RegistryService>,
    /// Event bus.
    pub events: Arc<EventsService>,
    /// Logging service.
    pub logger: Arc<LoggerService>,
    isolate: Arc<OverlayMap<u64>>,
    intercept: Arc<OverlayMap<ArcValue>>,
    base_url: Mutex<Option<String>>,
    filter: Option<ContextFilter>,
    #[allow(dead_code)] // reserved for the TS shadow-context interop layer
    shadow: Option<Arc<ContextInner>>,
}

impl Deref for Context {
    type Target = ContextInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("fiber", &self.fiber.name())
            .finish()
    }
}

impl Context {
    /// Create the root context and install the built-in services.
    pub fn root() -> Self {
        let fiber = FiberCore::new_root();
        let reflect = Arc::new(ReflectService::default());
        let registry = Arc::new(RegistryService::default());
        let events = EventsService::new();
        let logger = LoggerService::new();
        let inner = Arc::new(ContextInner {
            root: OnceLock::new(),
            fiber: fiber.clone(),
            reflect,
            registry,
            events: events.clone(),
            logger: logger.clone(),
            isolate: Arc::new(OverlayMap::new()),
            intercept: Arc::new(OverlayMap::new()),
            base_url: Mutex::new(None),
            filter: None,
            shadow: None,
        });
        let ctx = Self { inner };
        let _ = ctx.inner.root.set(ctx.clone());
        fiber.bind_ctx(&ctx);
        fiber.bind_parent(ctx.clone());
        events.install(ctx.clone());
        logger.install(ctx.clone());

        // `logger` is both a built-in field and a named service. The root TS
        // constructor clears the built-in effects so they remain permanent;
        // do the same after registration.
        let _ = ctx.reflect.provide(&ctx, "logger", Some(arc(logger)), None);
        let _ = ctx.fiber.disposables.clear();
        ctx
    }

    /// The root context shared by every child.
    pub fn root_context(&self) -> Context {
        self.inner
            .root
            .get()
            .cloned()
            .unwrap_or_else(|| self.clone())
    }

    /// Base URL used to resolve relative plugin/module specifiers.
    pub fn base_url(&self) -> Option<String> {
        self.inner.base_url.lock().clone()
    }

    pub fn set_base_url(&self, value: Option<String>) {
        *self.inner.base_url.lock() = value;
    }

    fn child_with(
        &self,
        fiber: Arc<FiberCore>,
        isolate: Arc<OverlayMap<u64>>,
        intercept: Arc<OverlayMap<ArcValue>>,
        filter: Option<ContextFilter>,
        shadow: Option<Arc<ContextInner>>,
    ) -> Context {
        let root = OnceLock::new();
        let _ = root.set(self.root_context());
        Context {
            inner: Arc::new(ContextInner {
                root,
                fiber,
                reflect: self.reflect.clone(),
                registry: self.registry.clone(),
                events: self.events.clone(),
                logger: self.logger.clone(),
                isolate,
                intercept,
                base_url: Mutex::new(self.base_url()),
                filter,
                shadow,
            }),
        }
    }

    /// Create a child context owned by `fiber`, applying inject intercept
    /// configs over the parent's intercept chain.
    pub(crate) fn extend_with_fiber(&self, fiber: Arc<FiberCore>) -> Context {
        let intercept = Arc::new(OverlayMap::child(&self.intercept));
        for (name, config) in &fiber.inject {
            if let Some(config) = config {
                intercept.insert(name.clone(), config.clone());
            }
        }
        self.child_with(fiber, self.isolate.clone(), intercept, None, None)
    }

    /// Create a child context sharing this fiber with no overlay changes
    /// (TS `ctx.extend({})`).
    pub fn extend(&self) -> Context {
        self.child_with(
            self.fiber.clone(),
            self.isolate.clone(),
            self.intercept.clone(),
            None,
            None,
        )
    }

    /// Create a child context with an independent service scope for `name`.
    /// Passing the same explicit `label` joins scopes.
    pub fn isolate_with_label(&self, name: &str, label: Option<u64>) -> Context {
        let isolate = Arc::new(OverlayMap::child(&self.isolate));
        isolate.insert(
            name.to_string(),
            label.unwrap_or_else(allocate_isolation_label),
        );
        self.child_with(
            self.fiber.clone(),
            isolate,
            self.intercept.clone(),
            None,
            None,
        )
    }

    /// Create a child context with a fresh independent scope for `name`.
    pub fn isolate(&self, name: &str) -> Context {
        self.isolate_with_label(name, None)
    }

    /// Add service-specific intercept config for plugins started below this
    /// context.
    pub fn intercept(&self, name: &str, config: ArcValue) -> Context {
        let intercept = Arc::new(OverlayMap::child(&self.intercept));
        intercept.insert(name.to_string(), config);
        self.child_with(
            self.fiber.clone(),
            self.isolate.clone(),
            intercept,
            None,
            None,
        )
    }

    /// Context used by scoped internal/service dispatch.
    pub fn with_filter(&self, filter: ContextFilter) -> Context {
        self.child_with(
            self.fiber.clone(),
            self.isolate.clone(),
            self.intercept.clone(),
            Some(filter),
            None,
        )
    }

    /// Whether this dispatch context's filter accepts a target listener ctx.
    pub fn filter_allows(&self, target: &Context) -> bool {
        self.filter.as_ref().map(|f| f(target)).unwrap_or(true)
    }

    /// Current label of a service in this context's isolation chain.
    pub fn isolate_label(&self, name: &str) -> Option<u64> {
        self.isolate.get(name)
    }

    /// Allocate a root isolation label for a first-time service.
    ///
    /// Mirrors `reflect.ts` `provide()`: the first provide allocates the label
    /// on the *root* context (`this.ctx.root[isolate][name] ??= Symbol(name)`),
    /// while `isolate()` stores its label on the child overlay. Allocation is
    /// atomic so concurrent first provides share one label.
    pub fn isolate_label_ensure(&self, name: &str) -> u64 {
        if let Some(label) = self.isolate_label(name) {
            return label;
        }
        let label = allocate_isolation_label();
        let root = self.root_context();
        root.isolate.insert_if_absent(name, label)
    }

    /// Merge intercept values for a service, ancestors first.
    pub fn intercept_chain(&self, name: &str) -> Vec<ArcValue> {
        self.intercept.chain(name)
    }

    // ---- lifecycle / registry mixins ----

    /// Register a cleanup-aware effect on this context's fiber.
    pub fn effect(&self, label: &str, execute: BoxFuture<'static, Option<Disposer>>) -> Disposer {
        self.fiber.effect(label, execute)
    }

    /// Load a plugin in the current context.
    pub fn plugin(&self, plugin: Arc<dyn Plugin>, config: ArcValue) -> Arc<FiberCore> {
        self.registry.plugin(self, plugin, config)
    }

    /// Run a callback once the requested services are available.
    pub fn inject(&self, deps: InjectSpec, callback: InjectCallback) -> Arc<FiberCore> {
        self.registry.inject(self, deps, callback)
    }

    // ---- event mixins ----

    pub async fn on(&self, name: &str, listener: Arc<Listener>, options: EventOptions) -> Disposer {
        self.events.on(self, name, listener, options).await
    }

    pub async fn once(
        &self,
        name: &str,
        listener: Arc<Listener>,
        options: EventOptions,
    ) -> Disposer {
        self.events.once(self, name, listener, options).await
    }

    pub fn emit(&self, name: &str, args: Vec<ArcValue>) {
        self.events.emit(Some(self), name, args);
    }

    /// Resolve the listener snapshot for one dispatch without invoking it
    /// (the TS internal `ctx.events.dispatch()`: internal pre-hooks run inline,
    /// then the filtered listeners are returned for the caller to run).
    pub fn collect(
        &self,
        mode: DispatchMode,
        name: &str,
        args: &[ArcValue],
    ) -> Vec<(Context, Arc<Listener>)> {
        self.events.collect(mode, Some(self), name, args)
    }

    pub async fn parallel(&self, name: &str, args: Vec<ArcValue>) {
        self.events.parallel(Some(self), name, args).await;
    }

    pub async fn serial(&self, name: &str, args: Vec<ArcValue>) -> Option<ArcValue> {
        self.events.serial(Some(self), name, args).await
    }

    pub async fn bail(&self, name: &str, args: Vec<ArcValue>) -> Option<ArcValue> {
        self.events.bail(Some(self), name, args).await
    }

    pub fn waterfall(
        &self,
        name: &str,
        args: Vec<ArcValue>,
        fallback: BoxFuture<'static, ArcValue>,
    ) -> BoxFuture<'static, ArcValue> {
        self.events.waterfall(Some(self), name, args, fallback)
    }

    // ---- reflect mixins ----

    pub fn accessor(&self, name: &str, accessor: Arc<Accessor>) -> Disposer {
        self.reflect.accessor(self, name, accessor)
    }

    pub fn mixin(&self, source: &str, keys: Vec<String>) -> Disposer {
        self.reflect.mixin(self, source, keys)
    }

    // ---- logger callable-service equivalent ----

    pub fn named_logger(&self, name: Option<&str>) -> crate::logger::Logger {
        self.logger.logger(self, name)
    }
}
