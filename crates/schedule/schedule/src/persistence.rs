//! Schedule-owned use of the shared session durability barrier. Rust port
//! of `packages/schedule/schedule/src/persistence.ts`.

use std::sync::Arc;

use cordis::Context;
use dsh_session::{Session, SessionStore};

/// Failure to prove that the current live prefix reached a persistence
/// listener.
#[derive(Debug, Clone)]
pub struct SchedulePersistenceError {
    pub message: String,
}

impl SchedulePersistenceError {
    pub fn new() -> Self {
        Self {
            message: "Schedule persistence did not complete.".to_string(),
        }
    }
}

impl std::fmt::Display for SchedulePersistenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SchedulePersistenceError {}

/// Require one successful shared persistence checkpoint.
pub async fn flush_schedule_persistence(
    ctx: &Context,
    session: &Session,
) -> Result<(), SchedulePersistenceError> {
    let Some(store) = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .map(|slot| slot.as_ref().clone())
    else {
        return Err(SchedulePersistenceError::new());
    };
    match store.flush(session).await {
        Ok(true) => Ok(()),
        _ => Err(SchedulePersistenceError::new()),
    }
}
