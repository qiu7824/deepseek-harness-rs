//! Opaque revision identity for lightweight persistence observations.
//! Rust port of `packages/session/session-persistence/src/revision.ts`.

use dsh_brand::Branded;

/// Backend-owned token that identifies both one storage source and one
/// revision of a persisted session log.
#[doc(hidden)]
pub enum SessionPersistenceRevisionTag {}
pub type SessionPersistenceRevision = Branded<SessionPersistenceRevisionTag>;

/// Brand a backend revision for the provider-neutral persistence contract
/// (TS `SessionPersistenceRevision(value)`).
pub fn session_persistence_revision(value: impl Into<String>) -> SessionPersistenceRevision {
    Branded::new(value)
}
