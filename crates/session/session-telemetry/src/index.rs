//! SessionTelemetryBackend Service Definition for the DeepSeek Harness. Rust
//! port of `packages/session/session-telemetry/src/index.ts`.
//!
//! This package owns the CAPTURE side of session-event reporting. Everything
//! downstream of [`SessionTelemetrySink::emit`] — batching, retry, queueing —
//! is the reporting SDK's territory.

use std::sync::Arc;

use cordis::Context;

/// Severity of a telemetry record, pre-mapped at capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTelemetrySeverity {
    Info,
    Warn,
    Error,
}

impl SessionTelemetrySeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionTelemetrySeverity::Info => "info",
            SessionTelemetrySeverity::Warn => "warn",
            SessionTelemetrySeverity::Error => "error",
        }
    }
}

/// The outbound channel: ledger (session-log mirror) or ops (operational
/// signal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTelemetryChannel {
    Ledger,
    Ops,
}

impl SessionTelemetryChannel {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionTelemetryChannel::Ledger => "ledger",
            SessionTelemetryChannel::Ops => "ops",
        }
    }
}

/// One attribute value: string or number.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeValue {
    Str(String),
    Num(f64),
}

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        AttributeValue::Str(value.to_string())
    }
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        AttributeValue::Str(value)
    }
}

impl From<u64> for AttributeValue {
    fn from(value: u64) -> Self {
        AttributeValue::Num(value as f64)
    }
}

/// One logical record handed to a backend — the capture contract's whole
/// outbound vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTelemetryRecord {
    pub channel: SessionTelemetryChannel,
    /// Unix epoch milliseconds.
    pub time: i64,
    pub severity: SessionTelemetrySeverity,
    /// Minimal identity attributes, insertion-ordered.
    pub attributes: Vec<(String, AttributeValue)>,
    /// The complete payload (deep copy of the session event's `data`, or the
    /// op payload).
    pub body: serde_json::Value,
}

/// The minimum backend contract the coordinator requires.
#[async_trait::async_trait]
pub trait SessionTelemetrySink: Send + Sync {
    /// Hand one record to the backend's pipeline (non-blocking enqueue).
    fn emit(&self, record: SessionTelemetryRecord);

    /// Optional hint that a turn ended.
    fn flush(&self) {}

    /// Forward the fiber's disposal to the SDK.
    async fn shutdown(&self) -> Result<(), String>;
}

/// Deployment-selected session-sharing policy disclosed by a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTelemetrySharingStatus {
    Full,
    FeedbackOnly,
    Disabled,
}

impl SessionTelemetrySharingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionTelemetrySharingStatus::Full => "full",
            SessionTelemetrySharingStatus::FeedbackOnly => "feedback-only",
            SessionTelemetrySharingStatus::Disabled => "disabled",
        }
    }
}

/// Loadable form of the backend contract: one implementation per context.
pub trait SessionTelemetryBackend: SessionTelemetrySink {
    /// The deployment-selected session-sharing policy.
    fn sharing(&self) -> SessionTelemetrySharingStatus;

    /// The composing context (for the coordinator).
    fn ctx(&self) -> &Context;
}

impl cordis::Service for dyn SessionTelemetryBackend {
    fn service_name(&self) -> &'static str {
        "sessionTelemetry"
    }
}

/// Install the capture coordinator for one backend (the TS backend
/// constructor composes `SessionTelemetryCoordinator`), and register the
/// backend as the `ctx.sessionTelemetry` service (the TS declaration).
pub fn install_telemetry_backend(
    ctx: &Context,
    backend: Arc<dyn SessionTelemetryBackend>,
    capture: SessionTelemetryCapture,
) {
    ctx.register_service(backend.clone());
    let coordinator = crate::coordinator::SessionTelemetryCoordinator::new(ctx, backend, capture);
    let _ = coordinator;
}

/// Whether capture follows live events or reads the canonical log only when
/// requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTelemetryCapture {
    Live,
    OnDemand,
}
