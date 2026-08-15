//! Abstract durable session persistence seam (`ctx.sessionPersistence`) for
//! the DeepSeek Harness. Rust port of
//! `@deepseek-ai/dsh-session-persistence`.

pub mod coordinator;
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
pub use index::{
    SessionInspection, SessionLocation, SessionPersistence, SessionPersistenceApi,
    SessionPersistenceSnapshot, SessionRawArtifact, SessionReadFromResult,
};
pub use preparations::{
    DiscardOutcome, PreparedSource, PreparedSourceLoader, PreparationEntry,
    SessionPreparationReservation, SessionPreparations,
};
pub use revision::{
    SessionPersistenceRevision, SessionPersistenceRevisionTag, session_persistence_revision,
};
pub use write_behind::{
    SessionWriteBehind, SessionWriteBehindOptions,
};
