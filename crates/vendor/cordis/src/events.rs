//! Event bus, dispatch modes, and listener registration.
//!
//! Rust port of `vendor/cordis/src/events.ts`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;

use futures::future::join_all;
use parking_lot::Mutex;

use crate::context::Context;
use crate::error::AggregateError;
use crate::util::{ArcValue, BoxFuture, arc, downcast, is_bailed};

/// Event dispatch strategy used by the event service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchMode {
    /// Run listeners concurrently, ignoring results (fire-and-forget).
    Emit,
    /// Await all listeners together; failures aggregate.
    Parallel,
    /// Await listeners in order until one bails.
    Serial,
    /// Run listeners in order until one bails.
    Bail,
    /// Compose listeners around a final `next` continuation.
    Waterfall,
}

fn mode_name(mode: DispatchMode) -> &'static str {
    match mode {
        DispatchMode::Emit | DispatchMode::Parallel => "emit",
        DispatchMode::Serial => "serial",
        DispatchMode::Bail => "bail",
        DispatchMode::Waterfall => "waterfall",
    }
}

/// Options accepted by `ctx.on()` and `ctx.once()`.
#[derive(Debug, Clone, Default)]
pub struct EventOptions {
    /// Add the listener before existing listeners for the same event.
    pub prepend: bool,
    /// Receive the event regardless of context filter checks.
    pub global: bool,
}

impl EventOptions {
    pub fn prepend(mut self, value: bool) -> Self {
        self.prepend = value;
        self
    }

    pub fn global(mut self, value: bool) -> Self {
        self.global = value;
        self
    }
}

/// Listener outcome: `None` (no bail), or a bail value.
pub type ListenerOutcome = Option<ArcValue>;

/// Registered listener function: receives the dispatch context and arguments.
/// The final argument of a waterfall dispatch is the `next` continuation
/// (see [`NextFn`]).
pub type Listener =
    dyn Fn(&Context, Vec<ArcValue>) -> BoxFuture<'static, ListenerOutcome> + Send + Sync;

/// Registered listener record stored by the event service.
pub struct Hook {
    pub ctx: Context,
    pub callback: Arc<Listener>,
    pub prepend: bool,
    pub global: bool,
}

/// Sized wrapper that lets listener values travel through the type-erased
/// hook/fiber stores (`dyn Listener` itself cannot satisfy `Any`).
pub struct ListenerWrap(pub Arc<Listener>);

/// Waterfall continuation handed to listeners: call it to invoke the next
/// listener (finally the built-in behavior); not calling it vetoes the chain.
/// Calling it more than once yields a dummy value (the continuation is
/// single-use, matching the TS `next` contract).
type NextCallback = Box<dyn FnOnce() -> BoxFuture<'static, ArcValue> + Send>;
type NextSlot = Arc<Mutex<Option<NextCallback>>>;

pub struct NextFn(NextSlot);

impl NextFn {
    /// Invoke the continuation (at most once).
    pub fn call(&self) -> BoxFuture<'static, ArcValue> {
        match self.0.lock().take() {
            Some(next) => next(),
            None => Box::pin(async { dummy_value() }),
        }
    }
}

/// A disposer function; runs cleanup exactly once and settles when done.
pub type Disposer = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// Build a disposer from a closure.
pub fn make_disposer(f: impl Fn() -> BoxFuture<'static, ()> + Send + Sync + 'static) -> Disposer {
    Arc::new(f)
}

fn dummy_value() -> ArcValue {
    arc(())
}

type HookMap = Arc<Mutex<HashMap<String, Vec<Hook>>>>;

/// Event bus installed as `ctx.events` and mixed into every context.
pub struct EventsService {
    hooks: HookMap,
    ctx: OnceLock<Context>,
}

impl EventsService {
    /// Create the empty event bus; `install` binds the root context and
    /// installs the built-in internal listeners.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            hooks: HookMap::default(),
            ctx: OnceLock::new(),
        })
    }

    /// Bind the root context and install the built-in internal listeners.
    pub fn install(self: &Arc<Self>, ctx: Context) {
        let _ = self.ctx.set(ctx.clone());
        self.install_internal();
    }

    /// Port of the two listeners installed by the TS constructor:
    ///
    /// 1. `internal/listener`: intercept registration of the special
    ///    `internal/update` event so per-fiber update hooks are kept on the
    ///    registering fiber (the disposer replaces global registration).
    fn install_internal(&self) {
        self.hooks
            .lock()
            .entry("internal/listener".to_string())
            .or_default()
            .push(Hook {
                ctx: self
                    .ctx
                    .get()
                    .cloned()
                    .expect("events service must be installed before listeners"),
                callback: Arc::new(|ctx: &Context, args: Vec<ArcValue>| {
                    let name = args
                        .first()
                        .and_then(|v| downcast::<String>(v))
                        .cloned()
                        .unwrap_or_default();
                    if name == "internal/update" {
                        let global = args
                            .get(2)
                            .and_then(|v| downcast::<EventOptions>(v))
                            .map(|o| o.global)
                            .unwrap_or(false);
                        if !global
                            && let Some(listener) = args
                                .get(1)
                                .and_then(crate::util::downcast_arc::<ListenerWrap>)
                                .map(|wrap| wrap.0.clone())
                        {
                            let wrapped = ListenerWrap(listener);
                            let remove = ctx.fiber.hooks_push("internal/update", arc(wrapped));
                            return Box::pin(async move { Some(arc(remove)) });
                        }
                    }
                    Box::pin(async move { None })
                }),
                prepend: false,
                global: true,
            });
    }

    /// Hooks registered on the dispatch context's fiber for `internal/update`.
    pub fn update_hooks(ctx: &Context) -> Vec<Arc<Listener>> {
        ctx.fiber
            .hooks_snapshot("internal/update")
            .into_iter()
            .filter_map(|value| crate::util::downcast_arc::<ListenerWrap>(&value))
            .map(|wrap| wrap.0.clone())
            .collect()
    }

    /// Resolve listeners for one dispatch and apply context filtering.
    ///
    /// The TS `dispatch()` runs the `internal/dispatch` pre-hook synchronously
    /// (a `bail` dispatch) before resolving the listener snapshot. This port
    /// keeps that ordering: the internal listeners run INLINE (blocked to
    /// completion, panics contained) so pre-commit validation observers (the
    /// session invariant companion) stage their transitions before any
    /// resolved listener can run. Internal listeners must therefore not await
    /// work that depends on the current task.
    pub fn collect(
        &self,
        mode: DispatchMode,
        this_arg: Option<&Context>,
        name: &str,
        args: &[ArcValue],
    ) -> Vec<(Context, Arc<Listener>)> {
        if !name.starts_with("internal/")
            && let Some(root) = self.ctx.get()
        {
            let dispatch_args = vec![
                arc(mode_name(mode).to_string()),
                arc(name.to_string()),
                arc(args.to_vec()),
                match this_arg {
                    Some(ctx) => arc(ctx.clone()),
                    None => arc(String::from("<none>")),
                },
            ];
            let internal = self.collect(
                DispatchMode::Bail,
                Some(root),
                "internal/dispatch",
                &dispatch_args,
            );
            for (ctx, callback) in internal {
                let future = callback(&ctx, dispatch_args.clone());
                // TS runs this pre-hook synchronously; failures are
                // contained per listener.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    futures::executor::block_on(future)
                }));
            }
        }

        let hooks = self.hooks.lock();
        let Some(list) = hooks.get(name) else {
            return Vec::new();
        };
        let mut result = Vec::new();
        for hook in list {
            let accept = hook.global
                || match this_arg {
                    None => true,
                    Some(ctx) => ctx.filter_allows(&hook.ctx),
                };
            if accept {
                // TS runs listeners with `thisArg ?? hook.ctx`: the dispatch
                // context takes precedence over the registering context.
                let listener_ctx = match this_arg {
                    Some(ctx) => ctx.clone(),
                    None => hook.ctx.clone(),
                };
                result.push((listener_ctx, hook.callback.clone()));
            }
        }
        result
    }

    fn emit_raw(&self, this_arg: Option<&Context>, name: &str, args: Vec<ArcValue>) {
        for (ctx, callback) in self.collect(DispatchMode::Emit, this_arg, name, &args) {
            let future = callback(&ctx, args.clone());
            tokio::spawn(async move {
                let _ = future.await;
            });
        }
    }

    /// Run listeners concurrently, ignoring results (fire-and-forget).
    pub fn emit(&self, this_arg: Option<&Context>, name: &str, args: Vec<ArcValue>) {
        self.emit_raw(this_arg, name, args);
    }

    /// Run listeners concurrently and wait for all of them; failures
    /// aggregate into an [`AggregateError`] panic, mirroring the TS throw.
    pub async fn parallel(&self, this_arg: Option<&Context>, name: &str, args: Vec<ArcValue>) {
        let futures: Vec<_> = self
            .collect(DispatchMode::Parallel, this_arg, name, &args)
            .into_iter()
            .map(|(ctx, callback)| {
                let args = args.clone();
                tokio::spawn(async move { callback(&ctx, args).await })
            })
            .collect();
        let results = join_all(futures).await;
        let mut errors = Vec::new();
        for result in results {
            match result {
                Ok(_) => {}
                Err(reason) => errors.push(arc(reason)),
            }
        }
        if !errors.is_empty() {
            panic!("{}", AggregateError::new(errors));
        }
    }

    /// Run listeners in order, awaiting each, until one returns a bail value.
    pub async fn serial(
        &self,
        this_arg: Option<&Context>,
        name: &str,
        args: Vec<ArcValue>,
    ) -> Option<ArcValue> {
        for (ctx, callback) in self.collect(DispatchMode::Serial, this_arg, name, &args) {
            let result = callback(&ctx, args.clone()).await;
            if is_bailed(result.as_ref()) {
                return result;
            }
        }
        None
    }

    /// Run listeners in order until one returns a bail value.
    ///
    /// Note: listeners are futures in Rust, so this awaits each in turn —
    /// the TS version runs purely synchronously. Semantics are identical
    /// modulo the async boundary (documented deviation).
    pub async fn bail(
        &self,
        this_arg: Option<&Context>,
        name: &str,
        args: Vec<ArcValue>,
    ) -> Option<ArcValue> {
        for (ctx, callback) in self.collect(DispatchMode::Bail, this_arg, name, &args) {
            let result = callback(&ctx, args.clone()).await;
            if is_bailed(result.as_ref()) {
                return result;
            }
        }
        None
    }

    /// Compose listeners around the final `next` callback.
    ///
    /// `args` carries the listener arguments; `fallback` is the innermost
    /// continuation (the built-in behavior). Listeners run outermost-first;
    /// a listener that does not call `next()` vetoes the rest of the chain,
    /// and its own return value becomes the waterfall result. Per-fiber
    /// `internal/update` hooks run before the shared listeners.
    pub fn waterfall(
        &self,
        this_arg: Option<&Context>,
        name: &str,
        args: Vec<ArcValue>,
        fallback: BoxFuture<'static, ArcValue>,
    ) -> BoxFuture<'static, ArcValue> {
        struct ChainState {
            listeners: Vec<(Context, Arc<Listener>, Vec<ArcValue>)>,
            fallback: Mutex<Option<BoxFuture<'static, ArcValue>>>,
        }

        let mut listeners: Vec<(Context, Arc<Listener>)> = Vec::new();
        if name == "internal/update"
            && let Some(ctx) = this_arg
        {
            for hook in Self::update_hooks(ctx) {
                listeners.push((ctx.clone(), hook));
            }
        }
        listeners.extend(self.collect(DispatchMode::Waterfall, this_arg, name, &args));

        let state = Arc::new(ChainState {
            listeners: listeners
                .into_iter()
                .map(|(ctx, callback)| (ctx, callback, args.clone()))
                .collect(),
            fallback: Mutex::new(Some(fallback)),
        });

        fn call(state: Arc<ChainState>, index: usize) -> BoxFuture<'static, ArcValue> {
            Box::pin(async move {
                if index >= state.listeners.len() {
                    let fallback = state.fallback.lock().take();
                    return match fallback {
                        Some(future) => future.await,
                        None => dummy_value(),
                    };
                }
                let (ctx, callback, args) = state.listeners[index].clone();
                let state_for_next = state.clone();
                let next = NextFn(Arc::new(Mutex::new(Some(Box::new(move || {
                    call(state_for_next, index + 1)
                })))));
                let mut full_args = args;
                full_args.push(arc(next));
                let result = callback(&ctx, full_args).await;
                result.unwrap_or_else(dummy_value)
            })
        }

        call(state, 0)
    }

    /// Store a listener record as an effect on the calling fiber.
    pub fn register(
        &self,
        caller: &Context,
        label: &str,
        name: &str,
        callback: Arc<Listener>,
        options: &EventOptions,
    ) -> Disposer {
        let ctx = caller.clone();
        let name = name.to_string();
        let options = options.clone();
        let hooks = self.hooks.clone();
        let callback_for_dispose = callback.clone();
        let name_for_dispose = name.clone();
        let disposer = make_disposer(move || {
            let hooks = hooks.clone();
            let name = name_for_dispose.clone();
            let callback = callback_for_dispose.clone();
            Box::pin(async move {
                let mut hooks = hooks.lock();
                if let Some(list) = hooks.get_mut(&name) {
                    list.retain(|h| !Arc::ptr_eq(&h.callback, &callback));
                }
            })
        });
        {
            let hook = Hook {
                ctx: ctx.clone(),
                callback: callback.clone(),
                prepend: options.prepend,
                global: options.global,
            };
            let mut hooks = self.hooks.lock();
            let list = hooks.entry(name.clone()).or_default();
            if options.prepend {
                list.insert(0, hook);
            } else {
                list.push(hook);
            }
        }
        let _ = ctx.fiber.disposables.push(disposer.clone());
        let _ = label;
        disposer
    }

    /// Register an event listener owned by the calling fiber.
    ///
    /// The listener is removed automatically when the fiber unloads.
    /// Panics with `INACTIVE_EFFECT` if the fiber is already disposed
    /// (mirrors the TS synchronous throw).
    pub async fn on(
        &self,
        caller: &Context,
        name: &str,
        listener: Arc<Listener>,
        options: EventOptions,
    ) -> Disposer {
        caller
            .fiber
            .assert_active()
            .unwrap_or_else(|error| panic!("{error}"));

        // handle special events (internal/listener bail interception)
        let args = vec![
            arc(name.to_string()),
            arc(ListenerWrap(listener.clone())),
            arc(options.clone()),
        ];
        let intercepted = self.bail(Some(caller), "internal/listener", args).await;
        if let Some(result) = intercepted {
            return crate::util::downcast_arc::<Disposer>(&result)
                .map(|arc_disposer| (*arc_disposer).clone())
                .unwrap_or_else(noop_disposer);
        }

        let label = format!(
            "ctx.on({})",
            serde_json::to_string(name).unwrap_or_default()
        );
        self.register(caller, &label, name, listener, &options)
    }

    /// Register an event listener that disposes itself after the first call.
    pub async fn once(
        &self,
        caller: &Context,
        name: &str,
        listener: Arc<Listener>,
        options: EventOptions,
    ) -> Disposer {
        let slot = Arc::new(Mutex::new(None::<Disposer>));
        let slot_for_listener = slot.clone();
        let listener_ref = listener.clone();
        let wrapper: Arc<Listener> = Arc::new(move |ctx: &Context, args: Vec<ArcValue>| {
            let slot = slot_for_listener.clone();
            let listener = listener_ref.clone();
            let ctx = ctx.clone();
            Box::pin(async move {
                let taken = { slot.lock().take() };
                if let Some(dispose) = taken {
                    dispose().await;
                }
                listener(&ctx, args).await
            })
        });
        let disposer = self.on(caller, name, wrapper, options).await;
        *slot.lock() = Some(disposer.clone());
        disposer
    }
}

fn noop_disposer() -> Disposer {
    make_disposer(|| Box::pin(async {}))
}
