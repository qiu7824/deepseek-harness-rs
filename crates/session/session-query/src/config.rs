//! Public configuration and typed failures for the combined session-query
//! service. Rust port of
//! `packages/session-query/session-query/src/config.ts`.

/// Default maximum `before`/`after` raw-event window.
pub const SESSION_QUERY_READ_WINDOW_MAX: u64 = 50;

/// Default maximum number of concurrent persisted-log inspections in one
/// batch read.
pub const SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY: usize = 4;

/// Backend-independent configuration inherited by every session-query
/// implementation.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Maximum accepted raw read context on either side. Defaults to 50.
    pub read_window_max: Option<u64>,
    /// Maximum concurrent persisted-log inspections in one batch read.
    /// Defaults to 4.
    pub persisted_inspect_concurrency: Option<usize>,
}

/// Stable machine-routable failure taxonomy (TS `SessionQueryErrorCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionQueryErrorCode {
    SessionQueryAborted,
    SessionQueryCorruptSession,
    SessionQueryEventNotFound,
    SessionQueryIndexFailed,
    SessionQueryInvalidConfig,
    SessionQueryInvalidCursor,
    SessionQueryInvalidFilter,
    SessionQueryInvalidLimit,
    SessionQueryInvalidQuery,
    SessionQueryInvalidLineage,
    SessionQueryInvalidSurface,
    SessionQueryInvalidWindow,
    SessionQueryPersistenceFailed,
    SessionQuerySearchDisabled,
    SessionQuerySessionNotFound,
    SessionQueryStaleCursor,
    SessionQuerySourceConflict,
}

impl SessionQueryErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionQueryErrorCode::SessionQueryAborted => "SESSION_QUERY_ABORTED",
            SessionQueryErrorCode::SessionQueryCorruptSession => "SESSION_QUERY_CORRUPT_SESSION",
            SessionQueryErrorCode::SessionQueryEventNotFound => "SESSION_QUERY_EVENT_NOT_FOUND",
            SessionQueryErrorCode::SessionQueryIndexFailed => "SESSION_QUERY_INDEX_FAILED",
            SessionQueryErrorCode::SessionQueryInvalidConfig => "SESSION_QUERY_INVALID_CONFIG",
            SessionQueryErrorCode::SessionQueryInvalidCursor => "SESSION_QUERY_INVALID_CURSOR",
            SessionQueryErrorCode::SessionQueryInvalidFilter => "SESSION_QUERY_INVALID_FILTER",
            SessionQueryErrorCode::SessionQueryInvalidLimit => "SESSION_QUERY_INVALID_LIMIT",
            SessionQueryErrorCode::SessionQueryInvalidQuery => "SESSION_QUERY_INVALID_QUERY",
            SessionQueryErrorCode::SessionQueryInvalidLineage => "SESSION_QUERY_INVALID_LINEAGE",
            SessionQueryErrorCode::SessionQueryInvalidSurface => "SESSION_QUERY_INVALID_SURFACE",
            SessionQueryErrorCode::SessionQueryInvalidWindow => "SESSION_QUERY_INVALID_WINDOW",
            SessionQueryErrorCode::SessionQueryPersistenceFailed => "SESSION_QUERY_PERSISTENCE_FAILED",
            SessionQueryErrorCode::SessionQuerySearchDisabled => "SESSION_QUERY_SEARCH_DISABLED",
            SessionQueryErrorCode::SessionQuerySessionNotFound => "SESSION_QUERY_SESSION_NOT_FOUND",
            SessionQueryErrorCode::SessionQueryStaleCursor => "SESSION_QUERY_STALE_CURSOR",
            SessionQueryErrorCode::SessionQuerySourceConflict => "SESSION_QUERY_SOURCE_CONFLICT",
        }
    }
}

/// Typed session-query failure whose `code` is one closed taxonomy member
/// (the TS `SessionQueryError` extends `HarnessError`; the shape is
/// re-implemented to avoid the llm→query dependency edge).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionQueryError {
    pub code: SessionQueryErrorCode,
    pub message: String,
}

impl SessionQueryError {
    pub fn new(code: SessionQueryErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SessionQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessionQueryError {}
