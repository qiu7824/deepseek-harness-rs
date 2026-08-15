//! Owner-scoped persistent PTY session seam: registry-minted ids, replaceable
//! backends, exclusive sends, bounded reads, signals, and awaited cleanup.
//! Rust port of `packages/terminal/terminal`.

pub mod index;
pub mod invariant;
pub mod types;

pub use index::{SessionRecord, TerminalSessionService, owner_key};
pub use types::{
    TerminalAbort, TerminalBackend, TerminalBackendSession, TerminalBackendSpawnError,
    TerminalBackendSpawnSpec, TerminalError, TerminalErrorCode, TerminalFailure,
    TerminalReadRequest, TerminalReadResult, TerminalSendOperation, TerminalSendRead,
    TerminalSendRequest, TerminalSendResult, TerminalSessionId, TerminalSessionIdTag,
    TerminalSessionSnapshot, TerminalSessionStatus, TerminalSignal, TerminalSignalResult,
    TerminalSpawnRequest, TerminalSpawnResult, TerminalWaitReason, terminal_session_id,
};
