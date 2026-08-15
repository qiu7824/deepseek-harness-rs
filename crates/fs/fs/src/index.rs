//! Filesystem Service Definition for one execution world. Rust port of
//! `packages/fs/fs/src/index.ts`. Backends own stable target identity,
//! process paths and file URIs, containment, text reads, decoding, binary
//! rejection, and atomic mutations. Read windows and observed-state policy
//! stay in consumer and policy plugins; `editText` remains here so version
//! check, literal match, and rewrite share one critical section.
//!
//! # Deviations
//!
//! - `AbortSignal` collapses into the seam-wide [`AbortPredicate`]
//!   (`Arc<dyn Fn() -> bool>`).
//! - `streamText` returns a `futures` boxed stream instead of an
//!   `AsyncIterable`.
//! - The three event declarations (`fs/write-intent` waterfall,
//!   `fs/edit-intent` waterfall, `fs/observed` emit) are documented here;
//!   consumers wire them through `ctx.on`/`ctx.waterfall` like the other
//!   ported seams.

use cordis::Service;

use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode};

use crate::types::{
    AbortPredicate, FsDirEntry, FsEditOutcome, FsEditRequest, FsError, FsInfo, FsPathInfo,
    FsTarget, FsVersion, FsWriteIntent, FsWriteOutcome,
};

/// Options for [`FileSystem::resolve`] (the TS inline `opts`).
#[derive(Clone, Default)]
pub struct ResolveOptions {
    /// Base directory for relative paths; overrides the backend's default.
    pub cwd: Option<String>,
    /// Cancellation for the mapping round-trip.
    pub signal: Option<AbortPredicate>,
}

/// Options for [`FileSystem::lstat`] (the TS inline `opts`).
#[derive(Debug, Clone, Default)]
pub struct LstatOptions {
    /// Base directory for relative paths; overrides the backend's default.
    pub cwd: Option<String>,
}

/// The edit guard (the TS inline `{ version: FsVersion }`).
#[derive(Debug, Clone, PartialEq)]
pub struct FsEditGuard {
    pub version: FsVersion,
}

/// Abstract filesystem provider (TS `FileSystem`). Targets must preserve
/// identity across aliases; reads expose regular UTF-8 text or typed
/// errors, listings are stable and content-free, and mutations are atomic.
/// Optional guards add stale protection without changing the unguarded
/// provider contract.
#[async_trait::async_trait]
pub trait FileSystem: Send + Sync + 'static {
    /// The sandbox mode this backend enforces on mutations BY DEFAULT, or
    /// `None` when it does not confine at all — the capability fact the tool
    /// layer reads to advertise the escalation fields honestly. The base
    /// implementation and the bare local backend report `None`; a sandboxing
    /// backend (`dsh-fs-sandbox`) overrides it with the deployment default.
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        None
    }

    /// Resolve a model/plugin-supplied path into a stable [`FsTarget`]. May
    /// perform I/O (a remote/sandboxed backend may need a round-trip to map
    /// a path to a stable identity), hence async even though the local
    /// backend only normalizes + realpaths.
    ///
    /// Relative paths resolve against `opts.cwd`.
    async fn resolve(
        &self,
        path: &str,
        opts: Option<&ResolveOptions>,
    ) -> Result<FsTarget, FsError>;

    /// Return the canonical absolute path a subprocess in this filesystem's
    /// execution world can open. The path is deliberately separate from
    /// [`FsTarget::target_key`]: consumers may pass this value to another OS
    /// capability, but must continue treating the target key as opaque.
    fn process_path(&self, target: &FsTarget) -> String;

    /// Return the canonical `file:` URI for a target in this filesystem's
    /// execution world. Backends own URI encoding because the host platform
    /// may differ from the execution platform.
    fn file_url(&self, target: &FsTarget) -> String;

    /// Test canonical containment without exposing or parsing backend
    /// target keys. Both targets must come from this provider.
    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool;

    /// Return target metadata, or `None` when the target does not exist.
    /// Metadata only, never content.
    async fn stat(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsInfo>, FsError>;

    /// Return path metadata without following the final path component when
    /// it is a symbolic link. This is intentionally path-shaped, not
    /// target-shaped: [`FileSystem::resolve`] follows symlinks to produce
    /// the stable identity used by normal reads/writes, while `lstat` lets a
    /// consumer reject the path itself before that follow happens.
    async fn lstat(
        &self,
        path: &str,
        opts: Option<&LstatOptions>,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsPathInfo>, FsError>;

    /// Read the whole regular text file as a single decoded string.
    async fn read_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<String, FsError>;

    /// Stream the whole regular text file as decoded text chunks (same text
    /// semantics as [`FileSystem::read_text`], for large files). The backend
    /// owns cross-chunk UTF-8 decoding and binary rejection so the policy
    /// layer never touches raw bytes.
    async fn stream_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<futures::stream::BoxStream<'static, Result<String, FsError>>, FsError>;

    /// Read the whole regular file as raw bytes with no decoding or binary
    /// rejection. The bound lives at this seam so a backend can never buffer
    /// an unbounded file: a target known or discovered to exceed `max_bytes`
    /// fails with `FS_TOO_LARGE` instead of returning a truncated result.
    async fn read_bytes(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError>;

    /// List direct children of a directory in stable name order. Returns
    /// resolved child targets plus cheap metadata only; never reads file
    /// contents.
    async fn list_dir(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<Vec<FsDirEntry>, FsError>;

    /// Atomically create or replace UTF-8 text. `expected` guards intent and
    /// staleness; omission allows unconditional overwrite.
    ///
    /// `sandbox_policy` is the per-call mode and workspace root this write
    /// runs under; a sandboxing backend fences the write by it, the bare
    /// backend ignores it. Omit to leave the backend its own default.
    async fn write_text(
        &self,
        target: &FsTarget,
        content: &str,
        expected: Option<&FsWriteIntent>,
        signal: Option<AbortPredicate>,
        sandbox_policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsWriteOutcome, FsError>;

    /// Atomically edit literal text. When supplied, the version guard is
    /// checked before matching so stale content reports `FS_STALE_VERSION`;
    /// omission edits the current content without a freshness precondition.
    async fn edit_text(
        &self,
        target: &FsTarget,
        edit: &FsEditRequest,
        expected: Option<&FsEditGuard>,
        signal: Option<AbortPredicate>,
        sandbox_policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsEditOutcome, FsError>;
}

impl Service for dyn FileSystem {
    fn service_name(&self) -> &'static str {
        "fs"
    }
}
