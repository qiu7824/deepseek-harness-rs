//! Host-filesystem implementation of `ctx.fs`. Rust port of
//! `@deepseek-ai/dsh-fs-local`.

pub mod fsio;
pub mod index;
pub mod invariant;
pub mod win32;

pub use fsio::{
    FsAbort, FsIoInternals, LineEndings, LocalDirEntry, LocalTarget, PathInfo, PathKind,
    PathLinkInfo, PathLinkKind, StagedPaths, apply_literal_edit, list_directory,
    normalize_line_endings, probe, probe_no_follow, read_for_edit, read_text_for_diff,
    read_whole_bytes, read_whole_text, resolve_local_target, restore_line_endings,
    stream_whole_text, write_file_atomic,
};
pub use index::{Config, LocalFileSystem, ResolvedConfig};
