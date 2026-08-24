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
