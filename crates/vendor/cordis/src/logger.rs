//! Logger facade, logger service, message, and exporter types.
//!
//! Rust port of `vendor/cordis/src/logger.ts` (facade/severity surface).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::context::Context;
use crate::util::{ArcValue, arc, downcast};

/// Logger method name and severity category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggerType {
    Error,
    Info,
    Warn,
    Debug,
}

impl LoggerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Debug => "debug",
        }
    }
}

/// Numeric severity used when exporters decide whether to emit a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LoggerLevel {
    Error = 0,
    Info = 1,
    Warn = 2,
    Debug = 3,
}

impl LoggerLevel {
    pub fn from_usize(value: usize) -> Self {
        match value {
            0 => Self::Error,
            2 => Self::Warn,
            3 => Self::Debug,
            _ => Self::Info,
        }
    }
}

/// Structured log record delivered to exporters.
#[derive(Debug, Clone)]
pub struct Message {
    pub sn: u64,
    pub ts: u64,
    pub name: String,
    pub r#type: LoggerType,
    pub level: LoggerLevel,
    pub args: Vec<ArcValue>,
}

/// Sink that receives structured log messages.
pub trait Exporter: Send + Sync {
    /// Default maximum level exported when no per-name level is set.
    fn default_level(&self) -> LoggerLevel {
        LoggerLevel::Info
    }

    /// Optional per-name level overrides.
    fn levels(&self) -> &HashMap<String, LoggerLevel> {
        static EMPTY: std::sync::LazyLock<HashMap<String, LoggerLevel>> =
            std::sync::LazyLock::new(HashMap::new);
        &EMPTY
    }

    fn export(&self, message: &Message);
}

/// Logger facade for one named subsystem.
#[derive(Clone)]
pub struct Logger {
    pub name: String,
    pub level: Option<LoggerLevel>,
    service: Arc<LoggerService>,
}

impl Logger {
    fn method(&self, r#type: LoggerType, level: LoggerLevel) -> impl Fn(&Self, Vec<ArcValue>) + '_ {
        move |this: &Self, args: Vec<ArcValue>| {
            let sn = this.service.sn.fetch_add(1, Ordering::Relaxed) + 1;
            let ts = chrono::Utc::now().timestamp_millis() as u64;
            for exporter in this.service.exporters.lock().values() {
                let target = exporter
                    .levels()
                    .get(&this.name)
                    .copied()
                    .or(this.level)
                    .unwrap_or_else(|| exporter.default_level());
                if target < level {
                    continue;
                }
                let message = Message {
                    sn,
                    ts,
                    name: this.name.clone(),
                    r#type,
                    level,
                    args: args.clone(),
                };
                exporter.export(&message);
            }
        }
    }

    pub fn error(&self, args: Vec<ArcValue>) {
        (self.method(LoggerType::Error, LoggerLevel::Error))(self, args);
    }

    pub fn warn(&self, args: Vec<ArcValue>) {
        (self.method(LoggerType::Warn, LoggerLevel::Warn))(self, args);
    }

    pub fn info(&self, args: Vec<ArcValue>) {
        (self.method(LoggerType::Info, LoggerLevel::Info))(self, args);
    }

    pub fn debug(&self, args: Vec<ArcValue>) {
        (self.method(LoggerType::Debug, LoggerLevel::Debug))(self, args);
    }
}

/// Logger service configuration merged from context intercepts.
#[derive(Debug, Clone, Default)]
pub struct LoggerIntercept {
    pub name: Option<String>,
    pub level: Option<usize>,
}

/// Built-in logging service (`ctx.logger`).
pub struct LoggerService {
    pub ctx: std::sync::OnceLock<Context>,
    buffer_size: usize,
    pub buffer: Mutex<Vec<Message>>,
    pub exporters: Mutex<HashMap<u64, Arc<dyn Exporter>>>,
    sn_exporter: AtomicU64,
    sn: AtomicU64,
}

impl LoggerService {
    pub fn new() -> Arc<Self> {
        let service = Arc::new(Self {
            ctx: std::sync::OnceLock::new(),
            buffer_size: 1000,
            buffer: Mutex::new(Vec::new()),
            exporters: Mutex::new(HashMap::new()),
            sn_exporter: AtomicU64::new(0),
            sn: AtomicU64::new(0),
        });
        let buffer_exporter = BufferExporter {
            service: Arc::downgrade(&service),
        };
        service.add_exporter(Arc::new(buffer_exporter));
        service
    }

    /// Bind the root context (two-phase root construction).
    pub fn install(&self, ctx: Context) {
        let _ = self.ctx.set(ctx);
    }

    fn add_exporter(&self, exporter: Arc<dyn Exporter>) {
        let sn = self.sn_exporter.fetch_add(1, Ordering::Relaxed) + 1;
        self.exporters.lock().insert(sn, exporter);
    }

    /// Register an exporter owned by the calling fiber.
    pub fn exporter(
        &self,
        caller: &Context,
        exporter: Arc<dyn Exporter>,
    ) -> crate::events::Disposer {
        let sn = self.sn_exporter.fetch_add(1, Ordering::Relaxed) + 1;
        self.exporters.lock().insert(sn, exporter);
        let service = self.arc_self();
        let disposer = crate::events::make_disposer(move || {
            let service = service.clone();
            Box::pin(async move {
                service.exporters.lock().remove(&sn);
            })
        });
        let _ = caller.fiber.disposables.push(disposer.clone());
        disposer
    }

    /// Resolve the merged intercept config for the `logger` service.
    pub fn resolve_config(&self, caller: &Context) -> LoggerIntercept {
        let mut config = LoggerIntercept::default();
        for value in caller.intercept_chain("logger") {
            if let Some(name) = downcast::<String>(&value) {
                config.name = Some(name.clone());
            }
        }
        config
    }

    /// Create a logger facade for the given name (defaults to the fiber name).
    pub fn logger(&self, caller: &Context, name: Option<&str>) -> Logger {
        let config = self.resolve_config(caller);
        let fiber_name = caller.fiber.name();
        let name = name
            .or(config.name.as_deref())
            .unwrap_or(fiber_name.as_str())
            .to_string();
        Logger {
            name,
            level: config.level.map(LoggerLevel::from_usize),
            service: self.arc_self(),
        }
    }

    /// Log directly under the calling fiber's derived name.
    pub fn error(&self, caller: &Context, args: Vec<ArcValue>) {
        self.logger(caller, None).error(args);
    }

    pub fn warn(&self, caller: &Context, args: Vec<ArcValue>) {
        self.logger(caller, None).warn(args);
    }

    pub fn info(&self, caller: &Context, args: Vec<ArcValue>) {
        self.logger(caller, None).info(args);
    }

    pub fn debug(&self, caller: &Context, args: Vec<ArcValue>) {
        self.logger(caller, None).debug(args);
    }

    /// Recover this service's owning `Arc` via the root context store.
    fn arc_self(&self) -> Arc<Self> {
        let Some(root) = self.ctx.get() else {
            unreachable!("logger service must be installed before use")
        };
        let value = root.get("logger", true).unwrap_or_else(|| arc(()));
        crate::util::downcast_arc::<Arc<Self>>(&value)
            .map(|arc_ref| arc_ref.as_ref().clone())
            .unwrap_or_else(|| unreachable!("logger service must be reachable through ctx"))
    }
}

/// Exporter pushing messages into the bounded service buffer
/// (port of the ctor-installed exporter).
struct BufferExporter {
    service: std::sync::Weak<LoggerService>,
}

impl Exporter for BufferExporter {
    fn export(&self, message: &Message) {
        let Some(service) = self.service.upgrade() else {
            return;
        };
        let mut buffer = service.buffer.lock();
        buffer.push(message.clone());
        if buffer.len() > service.buffer_size {
            let excess = buffer.len() - service.buffer_size;
            buffer.drain(..excess);
        }
    }
}
