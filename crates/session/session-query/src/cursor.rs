//! Opaque cursor identity for session-search pagination. Rust port of
//! `packages/session-query/session-query/src/cursor.ts`.

use dsh_brand::Branded;

/// The brand marker for [`SessionSearchCursor`].
#[doc(hidden)]
pub enum SessionSearchCursorTag {}

/// Provider-owned opaque continuation token returned by session search.
pub type SessionSearchCursor = Branded<SessionSearchCursorTag>;

/// Brand an encoded provider cursor for the public search contract.
pub fn session_search_cursor(value: impl Into<String>) -> SessionSearchCursor {
    SessionSearchCursor::new(value)
}
