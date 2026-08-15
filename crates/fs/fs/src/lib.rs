//! Filesystem Service Definition (`ctx.fs`). Rust port of
//! `@deepseek-ai/dsh-fs`.

pub mod index;
pub mod invariant;
pub mod types;

pub use index::{FileSystem, FsEditGuard, LstatOptions, ResolveOptions};
pub use invariant::{check_dispatch, validate_target};
pub use types::{
    AbortPredicate, FsDirEntry, FsEditOutcome, FsEditRequest, FsError, FsErrorCode, FsInfo,
    FsInfoType, FsObservation, FsPathInfo, FsPathInfoType, FsTarget, FsTargetKey, FsTargetKeyTag,
    FsVersion, FsVersionTag, FsWriteIntent, FsWriteOperation, FsWriteOutcome, fs_target_key,
    fs_version,
};
