//! Session-event telemetry capture seam for the DeepSeek Harness. Rust port
//! of `@deepseek-ai/dsh-session-telemetry`.

pub mod coordinator;
pub mod index;
pub mod invariant;

pub use coordinator::SessionTelemetryCoordinator;
pub use index::{
    AttributeValue, SessionTelemetryBackend, SessionTelemetryCapture,
    SessionTelemetryChannel, SessionTelemetryRecord, SessionTelemetrySeverity,
    SessionTelemetrySharingStatus, SessionTelemetrySink, install_telemetry_backend,
};
