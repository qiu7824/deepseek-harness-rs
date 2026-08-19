//! Vocabulary for the filesystem Service Definition (`ctx.fs`): the opaque
//! target/version identities, the metadata `stat` returns, the write-intent
//! and outcome shapes, the literal-edit request/outcome, and the typed error
//! taxonomy. Rust port of `packages/fs/fs/src/types.ts`.

use std::sync::Arc;

use dsh_brand::Branded;
use dsh_llm::HarnessError;

/// Marker for the filesystem target key brand.
#[doc(hidden)]
#[allow(dead_code)]
pub enum FsTargetKeyTag {}

/// Opaque key for stale guards and target lookup (TS `FsTargetKey`). The
/// local backend uses a realpath-like string; a remote backend might use a
/// workspace URI or file id. Consumers MUST NOT parse it or assume it is a
/// local absolute path.
pub type FsTargetKey = Branded<FsTargetKeyTag>;

/// Brand a string as an [`FsTargetKey`]. For backend use only — a consumer
/// never manufactures a key, it receives one from `resolve()`. No
/// validation is performed.
pub fn fs_target_key(key: impl Into<String>) -> FsTargetKey {
    FsTargetKey::new(key)
}

/// Marker for the filesystem version brand.
#[doc(hidden)]
#[allow(dead_code)]
pub enum FsVersionTag {}

/// Opaque file-version token — the freshness token a write/edit guards
/// against (TS `FsVersion`). The policy layer records it for stale checks;
/// consumers MUST NOT interpret this token.
pub type FsVersion = Branded<FsVersionTag>;

/// Brand a string as an [`FsVersion`]. For backend use only; no validation
/// is performed.
pub fn fs_version(version: impl Into<String>) -> FsVersion {
    FsVersion::new(version)
}

/// One authoritative observation of a target (TS `FsObservation`). A present
/// observation carries the version used by guarded replacement; an absent
/// observation authorizes only a guarded create, never an edit.
#[derive(Debug, Clone, PartialEq)]
pub enum FsObservation {
    Present { version: FsVersion },
    Absent,
}

/// A path resolved by a backend into a stable identity (TS `FsTarget`).
/// `resolve()` produces this; every other operation takes it.
#[derive(Debug, Clone, PartialEq)]
pub struct FsTarget {
    /// Opaque key for stale guards and target lookup.
    pub target_key: FsTargetKey,
    /// Path for model/UI-facing output. May be a local absolute path,
    /// workspace-relative path, or remote URI depending on the backend.
    pub display_path: String,
}

/// The target kind vocabulary (the TS inline `type` unions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsInfoType {
    File,
    Directory,
    Other,
}

impl FsInfoType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FsInfoType::File => "file",
            FsInfoType::Directory => "directory",
            FsInfoType::Other => "other",
        }
    }
}

/// The path-level kind vocabulary (adds `symlink`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsPathInfoType {
    File,
    Directory,
    Symlink,
    Other,
}

impl FsPathInfoType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FsPathInfoType::File => "file",
            FsPathInfoType::Directory => "directory",
            FsPathInfoType::Symlink => "symlink",
            FsPathInfoType::Other => "other",
        }
    }
}

/// Metadata about a target — what [`crate::index::FileSystem::stat`]
/// returns (TS `FsInfo`). `version` is the freshness token; `None` from
/// `stat` means the target is absent.
#[derive(Debug, Clone, PartialEq)]
pub struct FsInfo {
    /// Opaque freshness token of the target right now.
    pub version: FsVersion,
    /// Whether the target is a regular file, a directory, or something else.
    pub kind: FsInfoType,
    /// Byte size of a regular file, when the backend can report it.
    pub size: Option<u64>,
}

/// Metadata about a path without following the final path component when it
/// is a symbolic link (TS `FsPathInfo`). Can report `symlink` so consumers
/// with trust-boundary rules can reject repository-owned links before
/// resolving a target.
#[derive(Debug, Clone, PartialEq)]
pub struct FsPathInfo {
    /// Opaque freshness token of the path entry right now.
    pub version: FsVersion,
    /// Whether the path entry is a regular file, directory, symlink, or
    /// other.
    pub kind: FsPathInfoType,
    /// Byte size of the path entry, when the backend can report it.
    pub size: Option<u64>,
}

/// One direct child returned by [`crate::index::FileSystem::list_dir`] (TS
/// `FsDirEntry`). Listing returns metadata and resolved targets only; it
/// must not read file contents.
#[derive(Debug, Clone, PartialEq)]
pub struct FsDirEntry {
    /// Basename of the child inside the listed directory.
    pub name: String,
    /// Whether the child is a regular file, a directory, or something else.
    pub kind: FsInfoType,
    /// Resolved child target for follow-up operations.
    pub target: FsTarget,
    /// Opaque freshness token when the backend can report metadata cheaply.
    pub version: Option<FsVersion>,
    /// Byte size of a regular file, when the backend can report it.
    pub size: Option<u64>,
}

/// Guarded write intent (TS `FsWriteIntent`). `CreateIfAbsent` rejects an
/// existing target with `FS_NOT_OBSERVED`; `ReplaceIfVersion` rejects
/// absence or mismatch with `FS_STALE_VERSION`. Omitting the intent from
/// `writeText` means unconditional create-or-overwrite.
#[derive(Debug, Clone, PartialEq)]
pub enum FsWriteIntent {
    CreateIfAbsent,
    ReplaceIfVersion { version: FsVersion },
}

/// The write operation discriminator (the TS inline `operation` union).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsWriteOperation {
    Create,
    Update,
}

/// Outcome of a full-file write (TS `FsWriteOutcome`).
#[derive(Debug, Clone, PartialEq)]
pub struct FsWriteOutcome {
    /// Whether the write created a new file or replaced an existing one.
    pub operation: FsWriteOperation,
    /// Opaque version of the file after the write.
    pub version: FsVersion,
    /// The file's content BEFORE the write, or `None` when the file did not
    /// exist (a create) or the backend declined a contextual basis.
    /// LF-normalized storage text (the diff basis), never a diff.
    pub before: Option<String>,
    /// The file's content AFTER the write, LF-normalized to share
    /// `before`'s diff basis.
    pub after: String,
}

/// A literal-replacement edit request (TS `FsEditRequest`).
#[derive(Debug, Clone, PartialEq)]
pub struct FsEditRequest {
    /// Literal non-empty text to replace. Must match exactly (after
    /// line-ending normalization).
    pub old_string: String,
    /// Literal replacement text. An empty string deletes the matched text.
    pub new_string: String,
    /// Replace every match instead of requiring exactly one.
    pub replace_all: bool,
}

/// Outcome of a literal edit (TS `FsEditOutcome`).
#[derive(Debug, Clone, PartialEq)]
pub struct FsEditOutcome {
    /// Opaque version of the file after the edit.
    pub version: FsVersion,
    /// The file's content BEFORE the edit (LF-normalized storage text,
    /// never a diff).
    pub before: String,
    /// The file's content AFTER the edit.
    pub after: String,
}

/// Stable, machine-routable codes for filesystem failures (TS
/// `FsErrorCode`). Carried on [`FsError`]; the tool registry exposes
/// `{ name, code }` on `isError` results so retry/permission/UI layers can
/// branch without parsing messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FsErrorCode {
    FsNotFound,
    FsNotDirectory,
    FsNotText,
    FsNotRegularFile,
    FsTooLarge,
    FsPermissionDenied,
    FsSandboxDenied,
    FsIoError,
    FsStaleVersion,
    FsNotObserved,
    FsAmbiguousEdit,
    FsEditNotFound,
    FsAborted,
}

impl FsErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            FsErrorCode::FsNotFound => "FS_NOT_FOUND",
            FsErrorCode::FsNotDirectory => "FS_NOT_DIRECTORY",
            FsErrorCode::FsNotText => "FS_NOT_TEXT",
            FsErrorCode::FsNotRegularFile => "FS_NOT_REGULAR_FILE",
            FsErrorCode::FsTooLarge => "FS_TOO_LARGE",
            FsErrorCode::FsPermissionDenied => "FS_PERMISSION_DENIED",
            FsErrorCode::FsSandboxDenied => "FS_SANDBOX_DENIED",
            FsErrorCode::FsIoError => "FS_IO_ERROR",
            FsErrorCode::FsStaleVersion => "FS_STALE_VERSION",
            FsErrorCode::FsNotObserved => "FS_NOT_OBSERVED",
            FsErrorCode::FsAmbiguousEdit => "FS_AMBIGUOUS_EDIT",
            FsErrorCode::FsEditNotFound => "FS_EDIT_NOT_FOUND",
            FsErrorCode::FsAborted => "FS_ABORTED",
        }
    }
}

/// Typed filesystem error (TS `FsError`). Extends the harness error channel
/// so it carries a stable [`FsErrorCode`] and chains `cause`. `dsh-fs` owns
/// this vocabulary so backends and the policy layer raise the same codes
/// instead of each inventing message strings.
#[derive(Debug)]
pub struct FsError {
    pub error: HarnessError,
    /// The stable machine-routable filesystem failure code.
    pub code: FsErrorCode,
}

impl FsError {
    pub fn new(message: impl Into<String>, code: FsErrorCode) -> Self {
        Self {
            error: HarnessError::new(message, code.as_str()),
            code,
        }
    }

    pub fn with_cause(
        message: impl Into<String>,
        code: FsErrorCode,
        cause: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self {
            error: HarnessError::with_cause(message, code.as_str(), cause),
            code,
        }
    }

    /// The chained cause, when present.
    pub fn cause(&self) -> Option<&(dyn std::error::Error + Send + Sync)> {
        self.error.cause.as_deref()
    }
}

impl std::fmt::Display for FsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error.message)
    }
}

impl std::error::Error for FsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

/// The cancellation predicate used across the seam (the TS `AbortSignal`
/// collapse).
pub type AbortPredicate = Arc<dyn Fn() -> bool + Send + Sync>;
