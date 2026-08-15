//! Ownership of one unpublished Session before registry publication.
//! Rust port of `packages/core/session/src/preparation.ts`.
//!
//! Disposal is synchronous and idempotent: `Drop` runs the release callback
//! exactly once (the TS `[Symbol.dispose]` contract); [`SessionPreparation::dispose`]
//! exposes the same release explicitly.

use crate::Session;

/// Options for a preparation whose provider retains unpublished state.
#[derive(Default)]
pub struct SessionPreparationOptions {
    /// Release provider-owned state when the Session was not published.
    pub release: Option<Box<dyn FnOnce() + Send>>,
}

/// One exact unpublished Session and the provider state that keeps it
/// usable. Providers decide whether release returns the Session to a cache
/// or discards it; publication may consume that state before disposal,
/// making the callback a no-op.
pub struct SessionPreparation {
    /// The exact Session to use for setup and publication.
    pub session: Session,
    options: SessionPreparationOptions,
    released: bool,
}

impl SessionPreparation {
    /// Wrap an unpublished Session in one preparation lifetime.
    pub fn create(session: Session, options: SessionPreparationOptions) -> Self {
        Self { session, options, released: false }
    }

    /// Release provider state once when this preparation leaves its caller.
    pub fn dispose(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        if let Some(release) = self.options.release.take() {
            release();
        }
    }

    pub fn is_released(&self) -> bool {
        self.released
    }
}

impl Drop for SessionPreparation {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SESSION_FORMAT_VERSION, session_id};
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn release_runs_exactly_once() {
        let releases = std::sync::Arc::new(AtomicU32::new(0));
        let session = Session::create(session_id("s1"), None, None).unwrap();
        let count = releases.clone();
        let mut preparation = SessionPreparation::create(
            session,
            SessionPreparationOptions {
                release: Some(Box::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                })),
            },
        );
        preparation.dispose();
        preparation.dispose();
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        drop(preparation);
        assert_eq!(releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn drop_releases_unpublished_preparation() {
        let releases = std::sync::Arc::new(AtomicU32::new(0));
        let session = Session::create(session_id("s1"), None, None).unwrap();
        let count = releases.clone();
        let preparation = SessionPreparation::create(
            session,
            SessionPreparationOptions {
                release: Some(Box::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                })),
            },
        );
        drop(preparation);
        assert_eq!(releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn preparation_without_release_is_a_noop() {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        let mut preparation =
            SessionPreparation::create(session, SessionPreparationOptions::default());
        preparation.dispose();
        assert!(preparation.is_released());
    }

    #[test]
    fn prepared_session_is_unpublished() {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        assert_eq!(session.header().version, SESSION_FORMAT_VERSION);
        assert_eq!(session.events().len(), 0);
    }
}
