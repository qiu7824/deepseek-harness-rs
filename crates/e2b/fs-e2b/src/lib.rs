//! E2B provider for the filesystem capability seam. Paths, contents, and
//! atomic staging files remain inside the shared remote sandbox. Rust port
//! of `packages/e2b/fs-e2b`.
//!
//! # Deviations
//!
//! - The per-call sandbox policy argument is ignored (the remote sandbox
//!   boundary is the confinement).
//! - `stream_text` returns a boxed stream of decoded strings with a
//!   hand-rolled incremental UTF-8 decoder (the TS `TextDecoder`
//!   `{ stream: true }` semantics).
//! - The tests port a core subset of the TS spec (identity/metadata,
//!   reads, streams, guarded writes/edits); the full FakeRemote corpus
//!   arrives with later rounds.

pub mod index;
pub mod invariant;

pub use index::{E2bFileSystem, FS_E2B_INJECT, FS_E2B_NAME, FsE2bPlugin};
