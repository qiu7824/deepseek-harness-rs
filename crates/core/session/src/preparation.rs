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
        Self {
            session,
            options,
            released: false,
        }
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
