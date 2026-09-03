//! Shared immutable-header checks for logical session source observers.
//! Rust port of `packages/session-query/session-query/src/sources.ts`.

use dsh_session::SessionHeader;

use crate::config::{SessionQueryError, SessionQueryErrorCode};

/// Reject incompatible observations of one logical session source (TS
/// `assertSessionHeadersCompatible`).
pub fn assert_session_headers_compatible(
    a: &SessionHeader,
    b: &SessionHeader,
) -> Result<(), SessionQueryError> {
    if a.version != b.version
        || a.id != b.id
        || a.created_at != b.created_at
        || a.cwd != b.cwd
        || a.parent_session != b.parent_session
        || a.is_seeded != b.is_seeded
        || a.delegation_depth.unwrap_or(0) != b.delegation_depth.unwrap_or(0)
    {
        return Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQuerySourceConflict,
            format!("session source headers conflict for session \"{}\"", a.id),
        ));
    }
    Ok(())
}
