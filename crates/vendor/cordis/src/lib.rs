//! Cordis meta-framework — Rust port of `@deepseek-ai/cordis` v4.0.1.
//!
//! # Architecture
//!
//! - [`Context`] is a root/child dependency container.
//! - [`ReflectService`] owns named service implementations and isolation.
//! - [`RegistryService`] starts plugins as [`FiberCore`] instances.
//! - [`FiberCore`] tracks dependency epochs and reversible effects.
//! - [`EventsService`] implements emit/parallel/serial/bail/waterfall dispatch.
//! - [`LoggerService`] provides named structured loggers and exporters.
//!
//! The TS implementation obtains dynamic member access through a Proxy. Rust
//! keeps the same runtime names and lifecycle semantics while exposing the
//! mixed-in operations as inherent `Context` methods.

pub mod context;
pub mod error;
pub mod events;
pub mod fiber;
pub mod logger;
pub mod reflect;
pub mod registry;
pub mod service;
pub mod util;

pub use context::{Context, allocate_isolation_label};
pub use error::{AggregateError, CordisError, CordisErrorCode, PluginError, ValidationError};
pub use events::{
    DispatchMode, Disposer, EventOptions, EventsService, Hook, Listener, ListenerOutcome,
    ListenerWrap, NextFn, make_disposer,
};
pub use fiber::{EffectMeta, FiberCore, FiberState};
pub use logger::{
    Exporter, Logger, LoggerIntercept, LoggerLevel, LoggerService, LoggerType, Message,
};
pub use reflect::{Accessor, Impl, Property, ReflectService};
pub use registry::{InjectSpec, Plugin, PluginRuntime, RegistryService};
pub use service::{CheckFn, Service};
pub use util::{
    ArcValue, BoxFuture, DisposableList, OverlayMap, arc, downcast, downcast_arc, symbols,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering as MemOrder};

    fn test_ctx() -> Context {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .try_init();
        Context::root()
    }

    /// Test plugin: counts loads/unloads, optionally provides a service.
    struct CounterPlugin {
        name: &'static str,
        runs: Arc<AtomicU32>,
        unloads: Arc<AtomicU32>,
        provides: Option<(&'static str, ArcValue)>,
        requires: &'static [&'static str],
    }

    #[async_trait::async_trait]
    impl Plugin for CounterPlugin {
        fn name(&self) -> Option<&'static str> {
            Some(self.name)
        }

        fn inject(&self) -> InjectSpec {
            InjectSpec::new(self.requires.iter().copied())
        }

        async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
            self.runs.fetch_add(1, MemOrder::SeqCst);
            let unloads = self.unloads.clone();
            ctx.effect(
                "test-unload",
                Box::pin(async move {
                    Some(events::make_disposer(move || {
                        let unloads = unloads.clone();
                        Box::pin(async move {
                            unloads.fetch_add(1, MemOrder::SeqCst);
                        })
                    }))
                }),
            );
            if let Some((name, value)) = &self.provides {
                ctx.provide(name, Some(value.clone()));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn root_exposes_logger_service() {
        let ctx = test_ctx();
        assert!(ctx.get("logger", true).is_some());
        assert!(
            ctx.get_typed::<Arc<LoggerService>>("logger", true)
                .is_some()
        );
        assert!(ctx.get("missing", true).is_none());
    }

    #[tokio::test]
    async fn typed_get_executes_registered_accessors() {
        let ctx = test_ctx();
        ctx.accessor(
            "computed",
            Arc::new(Accessor {
                get: Arc::new(|_caller| arc(42i32)),
                set: None,
            }),
        );
        assert_eq!(
            *ctx.get_typed::<i32>("computed", true).expect("computed"),
            42
        );
    }

    #[tokio::test]
    async fn plugin_loads_and_provides() {
        let ctx = test_ctx();
        let runs = Arc::new(AtomicU32::new(0));
        let plugin = Arc::new(CounterPlugin {
            name: "p1",
            runs: runs.clone(),
            unloads: Arc::new(AtomicU32::new(0)),
            provides: Some(("foo", arc(42i32))),
            requires: &[],
        });
        let fiber = ctx.plugin(plugin, arc(()));
        fiber.settle().await.expect("plugin loads");
        assert_eq!(fiber.state(), FiberState::Active);
        assert_eq!(runs.load(MemOrder::SeqCst), 1);
        assert_eq!(*ctx.get_typed::<i32>("foo", true).expect("foo"), 42);

        fiber.dispose().await;
        assert_eq!(fiber.state(), FiberState::Disposed);
        assert!(ctx.get("foo", true).is_none());
    }

    #[tokio::test]
    async fn plugin_waits_for_dependencies() {
        let ctx = test_ctx();
        let runs = Arc::new(AtomicU32::new(0));
        let dep = Arc::new(CounterPlugin {
            name: "dep",
            runs: runs.clone(),
            unloads: Arc::new(AtomicU32::new(0)),
            provides: None,
            requires: &["foo"],
        });
        let fiber = ctx.plugin(dep, arc(()));
        assert_eq!(fiber.state(), FiberState::Pending);

        let provider = Arc::new(CounterPlugin {
            name: "provider",
            runs: Arc::new(AtomicU32::new(0)),
            unloads: Arc::new(AtomicU32::new(0)),
            provides: Some(("foo", arc(7i32))),
            requires: &[],
        });
        let provider_fiber = ctx.plugin(provider, arc(()));
        provider_fiber.settle().await.unwrap();
        fiber.settle().await.expect("dep loads after foo");
        assert_eq!(fiber.state(), FiberState::Active);
        assert_eq!(runs.load(MemOrder::SeqCst), 1);
    }

    #[tokio::test]
    async fn dependency_change_reloads_plugin() {
        let ctx = test_ctx();
        let runs = Arc::new(AtomicU32::new(0));
        let unloads = Arc::new(AtomicU32::new(0));
        let dep = Arc::new(CounterPlugin {
            name: "dep",
            runs: runs.clone(),
            unloads: unloads.clone(),
            provides: None,
            requires: &["foo"],
        });
        let fiber = ctx.plugin(dep, arc(()));

        let provider = Arc::new(CounterPlugin {
            name: "provider",
            runs: Arc::new(AtomicU32::new(0)),
            unloads: Arc::new(AtomicU32::new(0)),
            provides: Some(("foo", arc(7i32))),
            requires: &[],
        });
        let pf = ctx.plugin(provider, arc(()));
        pf.settle().await.unwrap();
        fiber.settle().await.unwrap();
        assert_eq!(runs.load(MemOrder::SeqCst), 1);

        // Removing foo unloads the dependent.
        pf.dispose().await;
        fiber.settle().await.unwrap();
        assert_eq!(unloads.load(MemOrder::SeqCst), 1);
        assert_eq!(fiber.state(), FiberState::Pending);

        // Providing foo again reloads it.
        let provider2 = Arc::new(CounterPlugin {
            name: "provider2",
            runs: Arc::new(AtomicU32::new(0)),
            unloads: Arc::new(AtomicU32::new(0)),
            provides: Some(("foo", arc(9i32))),
            requires: &[],
        });
        let pf2 = ctx.plugin(provider2, arc(()));
        pf2.settle().await.unwrap();
        fiber.settle().await.unwrap();
        assert_eq!(runs.load(MemOrder::SeqCst), 2);
        assert_eq!(fiber.state(), FiberState::Active);
        assert_eq!(*ctx.get_typed::<i32>("foo", true).unwrap(), 9);
    }

    #[tokio::test]
    async fn duplicate_provide_fails_fiber() {
        let ctx = test_ctx();
        let provider = Arc::new(CounterPlugin {
            name: "p1",
            runs: Arc::new(AtomicU32::new(0)),
            unloads: Arc::new(AtomicU32::new(0)),
            provides: Some(("foo", arc(1i32))),
            requires: &[],
        });
        let pf = ctx.plugin(provider, arc(()));
        pf.settle().await.unwrap();

        let dupe = Arc::new(CounterPlugin {
            name: "p2",
            runs: Arc::new(AtomicU32::new(0)),
            unloads: Arc::new(AtomicU32::new(0)),
            provides: Some(("foo", arc(2i32))),
            requires: &[],
        });
        let dupe_fiber = ctx.plugin(dupe, arc(()));
        assert!(dupe_fiber.settle().await.is_err());
        assert_eq!(dupe_fiber.state(), FiberState::Failed);
    }

    #[tokio::test]
    async fn emit_and_once_semantics() {
        let ctx = test_ctx();
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        ctx.on(
            "ping",
            Arc::new(move |_ctx, _args| {
                let h = h.clone();
                Box::pin(async move {
                    h.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        ctx.emit("ping", vec![]);
        // emit is fire-and-forget: give the listener task time to run
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(hits.load(MemOrder::SeqCst), 1);

        let once_hits = Arc::new(AtomicU32::new(0));
        let h2 = once_hits.clone();
        ctx.once(
            "tick",
            Arc::new(move |_ctx, _args| {
                let h = h2.clone();
                Box::pin(async move {
                    h.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        ctx.emit("tick", vec![]);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        ctx.emit("tick", vec![]);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(once_hits.load(MemOrder::SeqCst), 1);
    }

    #[tokio::test]
    async fn serial_stops_on_bail() {
        let ctx = test_ctx();
        let calls = Arc::new(AtomicU32::new(0));
        let c1 = calls.clone();
        ctx.on(
            "seq",
            Arc::new(move |_ctx, _args| {
                let c = c1.clone();
                Box::pin(async move {
                    c.fetch_add(1, MemOrder::SeqCst);
                    Some(arc("first".to_string()))
                })
            }),
            EventOptions::default(),
        )
        .await;
        let c2 = calls.clone();
        ctx.on(
            "seq",
            Arc::new(move |_ctx, _args| {
                let c = c2.clone();
                Box::pin(async move {
                    c.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        let result = ctx.serial("seq", vec![]).await;
        assert_eq!(
            *downcast::<String>(result.as_ref().unwrap()).unwrap(),
            "first"
        );
        assert_eq!(calls.load(MemOrder::SeqCst), 1);
    }

    #[tokio::test]
    async fn waterfall_chains_listeners() {
        let ctx = test_ctx();
        let log = Arc::new(std::sync::Mutex::new(String::new()));
        let l1 = log.clone();
        ctx.on(
            "chain",
            Arc::new(move |_ctx, args| {
                let l = l1.clone();
                Box::pin(async move {
                    l.lock().unwrap().push('A');
                    let next = downcast::<NextFn>(args.last().unwrap()).expect("next arg");
                    next.call().await;
                    l.lock().unwrap().push('a');
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        let l2 = log.clone();
        ctx.on(
            "chain",
            Arc::new(move |_ctx, args| {
                let l = l2.clone();
                Box::pin(async move {
                    l.lock().unwrap().push('B');
                    let next = downcast::<NextFn>(args.last().unwrap()).expect("next arg");
                    next.call().await;
                    l.lock().unwrap().push('b');
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        let log_fallback = log.clone();
        let fallback: BoxFuture<'static, ArcValue> = Box::pin(async move {
            log_fallback.lock().unwrap().push('0');
            arc(())
        });
        let _result = ctx.waterfall("chain", vec![], fallback).await;
        assert_eq!(*log.lock().unwrap(), "AB0ba");
    }

    #[tokio::test]
    async fn effect_disposer_is_idempotent() {
        let ctx = test_ctx();
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let disposer = ctx.effect(
            "e",
            Box::pin(async move {
                let h = h.clone();
                Some(events::make_disposer(move || {
                    let h = h.clone();
                    Box::pin(async move {
                        h.fetch_add(1, MemOrder::SeqCst);
                    })
                }))
            }),
        );
        disposer().await;
        disposer().await;
        assert_eq!(hits.load(MemOrder::SeqCst), 1);
    }

    #[tokio::test]
    async fn isolate_scopes_separate_implementations() {
        let ctx = test_ctx();
        let provider = Arc::new(CounterPlugin {
            name: "provider",
            runs: Arc::new(AtomicU32::new(0)),
            unloads: Arc::new(AtomicU32::new(0)),
            provides: Some(("foo", arc(1i32))),
            requires: &[],
        });
        let pf = ctx.plugin(provider, arc(()));
        pf.settle().await.unwrap();

        let isolated = ctx.isolate("foo");
        let provider2 = Arc::new(CounterPlugin {
            name: "provider2",
            runs: Arc::new(AtomicU32::new(0)),
            unloads: Arc::new(AtomicU32::new(0)),
            provides: Some(("foo", arc(2i32))),
            requires: &[],
        });
        let pf2 = isolated.plugin(provider2, arc(()));
        pf2.settle().await.unwrap();

        assert_eq!(*ctx.get_typed::<i32>("foo", true).unwrap(), 1);
        assert_eq!(*isolated.get_typed::<i32>("foo", true).unwrap(), 2);
    }
}
