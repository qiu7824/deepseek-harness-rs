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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_is_a_branded_string() {
        let revision = session_persistence_revision("r1");
        assert_eq!(revision.as_str(), "r1");
        let json = serde_json::to_string(&revision).unwrap();
        assert_eq!(json, "\"r1\"");
        let back: SessionPersistenceRevision = serde_json::from_str(&json).unwrap();
        assert_eq!(back, revision);
    }
}
