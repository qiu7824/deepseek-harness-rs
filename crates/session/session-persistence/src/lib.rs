//! Abstract durable session persistence seam (`ctx.sessionPersistence`) for
//! the DeepSeek Harness. Rust port of
//! `@deepseek-ai/dsh-session-persistence`.

pub mod coordinator;
pub mod history_window;
pub mod index;
pub mod invariant;
pub mod preparations;
pub mod revision;
pub mod write_behind;

pub use coordinator::{
    DEFAULT_PREPARED_SESSION_CACHE_SIZE, DEFAULT_WRITE_BATCH_MAX_DELAY_MS,
    MAX_WRITE_BATCH_DELAY_MS, PersistenceBackend, PersistenceCoordinator,
    PersistenceCoordinatorOptions, SessionFormatUnsupportedError,
    SessionPersistenceCorruptionError, StoredPrefix, StoredSuffix, session_format_version_refusal,
};
pub use history_window::{HistoryWindowSelection, HistoryWindowTooLarge, select_history_window};
pub use index::{
    SessionEventChunk, SessionInspection, SessionListMetadata, SessionLocation, SessionPersistence,
    SessionPersistenceApi, SessionPersistenceSnapshot, SessionRawArtifact,
    SessionReadForwardWindowRequest, SessionReadFromResult, SessionReadWindowRequest,
    SessionReadWindowResult, SessionUserMessageEvents,
};
pub use preparations::{
    DiscardOutcome, PreparationEntry, PreparedSource, PreparedSourceLoader,
    SessionPreparationReservation, SessionPreparations,
};
pub use revision::{
    SessionPersistenceRevision, SessionPersistenceRevisionTag, session_persistence_revision,
};
pub use write_behind::{SessionWriteBehind, SessionWriteBehindOptions};
