//! Types shared by PTY backends, the owner-scoped registry, and tool
//! consumers. Runtime service code lives in `./index.rs`. Rust port of
//! `packages/terminal/terminal/src/types.ts`.
//!
//! # Deviations
//!
//! - `AbortSignal` collapses into the seam-wide cancellation predicate
//!   ([`TerminalAbort`]), matching the repo-wide collapse (no event target
//!   exists in Rust; backends poll the predicate at their own pace).
//! - `TerminalBackendCleanupError` becomes the structured
//!   [`TerminalBackendSpawnError`] — the backend `spawn` error channel
//!   carries the optional cleanup failure explicitly instead of extending
//!   `AggregateError`.
//! - `TerminalSessionId` is the repo brand shape ([`dsh_brand::Branded`]).

use std::fmt;
use std::sync::Arc;

use dsh_agent::Agent;
use dsh_brand::Branded;
use futures::future::BoxFuture;

/// Marker for the terminal-session brand (TS `TerminalSessionIdValue`).
pub enum TerminalSessionIdTag {}

/// Opaque identity minted by the terminal service for one live PTY session.
pub type TerminalSessionId = Branded<TerminalSessionIdTag>;

/// Brand one registry-minted string as a [`TerminalSessionId`] (TS
/// `TerminalSessionId(value)`).
pub fn terminal_session_id(value: impl Into<String>) -> TerminalSessionId {
    Branded::new(value)
}

/// The abort/cancellation predicate (the TS `AbortSignal` collapse).
pub type TerminalAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// Backend-reported failure to clean partial resources after unpublished
/// setup failed (TS `TerminalBackendCleanupError`). The spawn failure rides
/// the ordinary error channel; the optional cleanup failure is carried
/// beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalBackendSpawnError {
    /// Original setup or cancellation failure (TS `spawnError`).
    pub spawn_error: String,
    /// Failure that may leave backend-owned resources alive (TS
    /// `cleanupError`).
    pub cleanup_error: Option<String>,
}

impl TerminalBackendSpawnError {
    pub fn spawn(spawn_error: impl Into<String>) -> Self {
        Self {
            spawn_error: spawn_error.into(),
            cleanup_error: None,
        }
    }

    pub fn cleanup_failed(spawn_error: impl Into<String>, cleanup_error: impl Into<String>) -> Self {
        Self {
            spawn_error: spawn_error.into(),
            cleanup_error: Some(cleanup_error.into()),
        }
    }
}

impl fmt::Display for TerminalBackendSpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PTY backend startup and cleanup both failed")
    }
}

impl std::error::Error for TerminalBackendSpawnError {}

/// Why one interactive send returned control to its caller (TS
/// `TerminalWaitReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalWaitReason {
    StdinRead,
    InferredIdle,
    Timeout,
    SessionExit,
}

impl TerminalWaitReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            TerminalWaitReason::StdinRead => "stdin_read",
            TerminalWaitReason::InferredIdle => "inferred_idle",
            TerminalWaitReason::Timeout => "timeout",
            TerminalWaitReason::SessionExit => "session_exit",
        }
    }
}

/// Signals the model-facing PTY surface permits for foreground process
/// groups. Kept member-identical to `SubprocessTerminalSignal` in
/// `dsh-subprocess` without a cross-seam dependency; change both together
/// (TS `TerminalSignal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSignal {
    SigInt,
    SigTerm,
    SigKill,
    SigTstp,
    SigHup,
}

impl TerminalSignal {
    pub fn as_str(&self) -> &'static str {
        match self {
            TerminalSignal::SigInt => "SIGINT",
            TerminalSignal::SigTerm => "SIGTERM",
            TerminalSignal::SigKill => "SIGKILL",
            TerminalSignal::SigTstp => "SIGTSTP",
            TerminalSignal::SigHup => "SIGHUP",
        }
    }
}

/// Top-level PTY process status, independent of a send's wait reason (TS
/// `TerminalSessionStatus`).
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalSessionStatus {
    Running,
    Exited {
        exit_code: Option<i32>,
        signal: Option<String>,
    },
}

/// Request to create one owner-scoped PTY session (TS
/// `TerminalSpawnRequest`). `type_` is the TS `type` field (a Rust
/// keyword).
#[derive(Debug, Clone, Default)]
pub struct TerminalSpawnRequest {
    /// Registered backend type.
    pub type_: String,
    /// Optional owner-local display name.
    pub name: Option<String>,
    /// Optional initial working directory interpreted by the backend.
    pub cwd: Option<String>,
}

/// Fully identified request handed from the registry to a backend (TS
/// `TerminalBackendSpawnSpec`).
#[derive(Clone)]
pub struct TerminalBackendSpawnSpec {
    /// Registry-minted session identity.
    pub session_id: TerminalSessionId,
    /// Exact live owner for authority-aware backend setup.
    pub owner: Arc<dyn Agent>,
    /// Backend type that created the session.
    pub type_: String,
    /// Optional owner-local display name.
    pub name: Option<String>,
    /// Optional initial working directory interpreted by the backend.
    pub cwd: Option<String>,
    /// Cancellation of unpublished backend setup.
    pub signal: Option<TerminalAbort>,
}

/// Input for one line-oriented terminal interaction (TS
/// `TerminalSendRequest`). Carries a cancellation predicate, so no `Debug`.
#[derive(Clone)]
pub struct TerminalSendRequest {
    /// UTF-8 text to write.
    pub text: String,
    /// Whether to write the backend's Enter sequence after `text`.
    pub submit: bool,
    /// Cancellation for the wait; backends also interrupt the foreground
    /// command.
    pub signal: Option<TerminalAbort>,
}

/// Incremental output consumed from one live send operation (TS
/// `TerminalSendRead`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSendRead {
    /// Output produced since the previous operation read.
    pub delta: String,
    /// Whether unread operation output was dropped by the backend's bound.
    pub truncated: bool,
}

/// Settled result for one foreground or background send (TS
/// `TerminalSendResult`).
#[derive(Debug, Clone)]
pub struct TerminalSendResult {
    /// Bounded rendered terminal delta remaining at settlement.
    pub viewport: String,
    /// Why the wait returned; this does not imply arbitrary child-process
    /// exit.
    pub wait_reason: TerminalWaitReason,
    /// Top-level session status observed at settlement.
    pub session_status: TerminalSessionStatus,
    /// Whether output was dropped from the operation or retained scrollback.
    pub truncated: bool,
}

/// Live backend-owned send; exactly one may be active per PTY session (TS
/// `TerminalSendOperation`). Every `done()` call resolves to the same
/// outcome — implementors cache the settlement future internally (the TS
/// promise property has no cloneable Rust equivalent).
pub trait TerminalSendOperation: Send + Sync + 'static {
    /// Resolves after readiness, timeout, cancellation, or top-level process
    /// exit.
    fn done(&self) -> BoxFuture<'static, TerminalSendResult>;
    /// Consume output produced since the prior call.
    fn read_output(&self) -> TerminalSendRead;
    /// Request `SIGINT`; returns false after the operation settled.
    fn cancel(&self) -> bool;
}

/// Request for one backward scrollback page (TS `TerminalReadRequest`).
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalReadRequest {
    /// Offset from the newest retained line; defaults are backend-owned.
    pub offset: Option<u64>,
    /// Requested line count; backend limits still apply.
    pub count: Option<u64>,
}

/// Bounded scrollback page (TS `TerminalReadResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalReadResult {
    /// Retained text in chronological order.
    pub text: String,
    /// Number of lines currently retained.
    pub total_lines: u64,
    /// Inclusive newest-relative offset of the first returned line.
    pub line_begin: u64,
    /// Exclusive newest-relative offset after the returned page.
    pub line_end: u64,
    /// Whether older retained output or the requested result exceeded a
    /// bound.
    pub truncated: bool,
}

/// Result of delivering a signal to a verified foreground process group (TS
/// `TerminalSignalResult`). `delivered` is structurally always true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSignalResult {
    pub delivered: bool,
    /// Process group that received the signal.
    pub target_pgid: u32,
}

/// Owner-visible summary of one published PTY session (TS
/// `TerminalSessionSnapshot`).
#[derive(Debug, Clone)]
pub struct TerminalSessionSnapshot {
    /// Registry-minted identity used by every operation.
    pub session_id: TerminalSessionId,
    /// Optional owner-local display name.
    pub name: Option<String>,
    /// Backend type that created the session.
    pub type_: String,
    /// Top-level process id when the backend has one.
    pub pid: Option<u32>,
    /// Current top-level process status.
    pub status: TerminalSessionStatus,
}

/// Successful publication returned by the terminal service's `spawn` (TS
/// `TerminalSpawnResult`).
#[derive(Debug, Clone)]
pub struct TerminalSpawnResult {
    pub session_id: TerminalSessionId,
    pub name: Option<String>,
    pub type_: String,
    pub pid: Option<u32>,
    pub status: TerminalSessionStatus,
    /// Initial bounded output captured before publication.
    pub motd: String,
}

/// Backend-owned live session retained by the terminal service (TS
/// `TerminalBackendSession`). Async methods take `&self`; implementors own
/// their state through internal `Arc`s so the `'static` futures never
/// capture the borrow (the TS `this` closure equivalent).
pub trait TerminalBackendSession: Send + Sync + 'static {
    /// Initial bounded terminal output returned from `terminal_open`.
    fn motd(&self) -> String;
    /// Top-level process id when one exists.
    fn pid(&self) -> Option<u32>;
    /// Start one exclusive send operation.
    fn start_send(&self, request: &TerminalSendRequest) -> Arc<dyn TerminalSendOperation>;
    /// Read one bounded page from retained scrollback.
    fn read(&self, request: &TerminalReadRequest) -> TerminalReadResult;
    /// Signal the verified foreground process group.
    fn signal(
        &self,
        signal: TerminalSignal,
    ) -> BoxFuture<'static, Result<TerminalSignalResult, String>>;
    /// Observe top-level process status.
    fn status(&self) -> TerminalSessionStatus;
    /// Idempotently close the captured owned process tree and await
    /// quiescence.
    fn close(&self, reason: &str) -> BoxFuture<'static, Result<(), String>>;
}

/// Replaceable provider for one PTY session type (TS `TerminalBackend`).
pub trait TerminalBackend: Send + Sync + 'static {
    /// Stable type selected by [`TerminalSpawnRequest::type_`].
    fn type_(&self) -> String;
    /// Create an unpublished session or reject after cleaning partial
    /// resources; cleanup failure rides [`TerminalBackendSpawnError`].
    fn spawn(
        &self,
        spec: TerminalBackendSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>>;
}

// ---- service failure channel ----

/// Machine-routable PTY service failures (TS `TerminalErrorCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalErrorCode {
    DuplicateBackend,
    DuplicateName,
    ForeignSession,
    NoBackend,
    NoSession,
    OwnerNotLive,
    SendActive,
    ServiceDisposing,
}

impl TerminalErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            TerminalErrorCode::DuplicateBackend => "DUPLICATE_BACKEND",
            TerminalErrorCode::DuplicateName => "DUPLICATE_NAME",
            TerminalErrorCode::ForeignSession => "FOREIGN_SESSION",
            TerminalErrorCode::NoBackend => "NO_BACKEND",
            TerminalErrorCode::NoSession => "NO_SESSION",
            TerminalErrorCode::OwnerNotLive => "OWNER_NOT_LIVE",
            TerminalErrorCode::SendActive => "SEND_ACTIVE",
            TerminalErrorCode::ServiceDisposing => "SERVICE_DISPOSING",
        }
    }
}

/// Error carrying a stable [`TerminalErrorCode`] (TS `TerminalError`).
#[derive(Debug, Clone)]
pub struct TerminalError {
    pub message: String,
    pub code: TerminalErrorCode,
}

impl TerminalError {
    pub fn new(message: impl Into<String>, code: TerminalErrorCode) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TerminalError {}

/// The unified failure channel of the terminal service surface. Coded
/// failures carry a [`TerminalErrorCode`]; plain messages mirror the TS
/// `Error` throws; aggregates mirror the TS `AggregateError` throws.
#[derive(Debug, Clone)]
pub enum TerminalFailure {
    /// A [`TerminalError`]-coded failure.
    Coded(TerminalError),
    /// An ordinary message failure (TS `Error`).
    Plain(String),
    /// Caller-signal cancellation. The TS reason-object identity has no Rust
    /// equivalent and collapses into this variant.
    Aborted,
    /// An aggregate of sub-failures (TS `AggregateError`).
    Aggregate {
        message: String,
        failures: Vec<TerminalFailure>,
    },
}

impl TerminalFailure {
    /// The failure's rendered message (the TS `.message`).
    pub fn message(&self) -> &str {
        match self {
            TerminalFailure::Coded(error) => &error.message,
            TerminalFailure::Plain(message) => message,
            TerminalFailure::Aborted => "aborted",
            TerminalFailure::Aggregate { message, .. } => message,
        }
    }

    /// The coded failure kind, when present (the TS `.code`).
    pub fn code(&self) -> Option<TerminalErrorCode> {
        match self {
            TerminalFailure::Coded(error) => Some(error.code),
            _ => None,
        }
    }
}

impl fmt::Display for TerminalFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for TerminalFailure {}
