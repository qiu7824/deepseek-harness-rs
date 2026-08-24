#![allow(
    clippy::type_complexity,
    clippy::collapsible_if,
    clippy::useless_conversion,
    clippy::manual_clamp
)]
// Testable atomic-publication hooks intentionally retain their exact callback shapes and sequencing.

//! Cordis-free local filesystem mechanics. Rust port of
//! `packages/fs/fs-local/src/fsio.ts`. This provider layer returns validated
//! UTF-8 text, streams large files, and rejects binary data; line windows
//! belong to `dsh-tool-fs`. Writes stage an exclusive owner-only file in a
//! private sibling directory and atomically publish it.
//!
//! # Deviations
//!
//! - `AbortSignal` collapses into the seam-wide cancellation predicate
//!   (`FsAbort = Arc<dyn Fn() -> bool + Send + Sync>`), checked at the same
//!   points the TS checks `signal.aborted`.
//! - `versionOf` uses `dev:ino:size:mtimeNs:ctimeNs` on Unix and a
//!   size/modified/created approximation on Windows (the Rust std layer
//!   exposes no file index); the TS formats are matched on POSIX.
//! - The Windows DACL copy/secure replacement boundaries are simplified
//!   no-ops until the sandbox-windows-acl milestone (recorded in
//!   `docs/porting/cordis-rust-notes.md`); the seam injection points remain
//!   so tests pin the same failure choreography.

use std::path::Path;
use std::sync::Arc;

use dsh_fs::{FsError, FsErrorCode, FsVersion, fs_target_key, fs_version};

use crate::win32::{copy_file_dacl_win32, replace_file_win32};

/// The abort/cancellation predicate (the TS `AbortSignal` collapse).
pub type FsAbort = Arc<dyn Fn() -> bool + Send + Sync>;

const BINARY_SAMPLE_BYTES: usize = 8192;
// Bound one non-abortable read so cancellation is observed between chunks.
const DIFF_BASIS_READ_CHUNK_BYTES: usize = 64 * 1024;

fn is_not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

/// Test hook: lets specs pin the atomic-write temp names, override native
/// boundaries, and observe the staged temp file before publication (the TS
/// `FsIoInternals`).
#[derive(Default)]
pub struct FsIoInternals {
    /// Override the host platform for native-publication unit coverage.
    pub platform: Option<String>,
    /// Override the generated private staging-dir name (relative to the
    /// target dir).
    pub temp_dir_name: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>,
    /// Override the generated temp-file name (relative to the private
    /// staging dir).
    pub temp_name: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>,
    /// Override the Win32 DACL copy boundary.
    pub copy_file_dacl: Option<Arc<dyn Fn(&Path, &Path) -> BoxFuture + Send + Sync>>,
    /// Override the Win32 security-preserving replacement boundary.
    pub replace_file: Option<Arc<dyn Fn(&Path, &Path) -> BoxFuture + Send + Sync>>,
    /// Override the hard-link no-replace publication boundary.
    pub link_file: Option<Arc<dyn Fn(&Path, &Path) -> BoxFuture + Send + Sync>>,
    /// Override target inspection after guarded publication fails.
    pub inspect_publication_target: Option<Arc<dyn Fn(&Path) -> BoxFuture + Send + Sync>>,
    /// Override staging-directory removal for commit-point failure coverage.
    pub remove_staging_dir: Option<Arc<dyn Fn(&Path) -> BoxFuture + Send + Sync>>,
    /// Test hook after the temp file is written/synced but before final
    /// chmod+publication.
    pub inspect_temp: Option<Arc<dyn Fn(&StagedPaths) -> BoxFuture + Send + Sync>>,
    /// Test hook after raw-read stat preflight and before bounded content
    /// I/O.
    pub inspect_read_bytes_after_stat: Option<Arc<dyn Fn(&LocalTarget) -> BoxFuture + Send + Sync>>,
}

/// The staged temp paths handed to [`FsIoInternals::inspect_temp`].
#[derive(Debug, Clone, PartialEq)]
pub struct StagedPaths {
    pub staging_dir: String,
    pub temp_path: String,
}

/// A hook result carrying a boxed future (seam ergonomics).
pub type BoxFuture = futures::future::BoxFuture<'static, Result<(), String>>;

/// A resolved local path: the absolute path shown to callers and its
/// realpath identity (TS `LocalTarget`).
#[derive(Debug, Clone, PartialEq)]
pub struct LocalTarget {
    /// Absolute path (symlinks not resolved) 鈥?used for display.
    pub display_path: String,
    /// Realpath identity 鈥?used as the stable target key and the I/O path.
    pub target_key: dsh_fs::FsTargetKey,
}

/// Result of probing a path (TS `PathInfo`); `None` when it does not exist.
#[derive(Debug, Clone, PartialEq)]
pub struct PathInfo {
    pub version: FsVersion,
    pub mode: u32,
    pub kind: PathKind,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    File,
    Directory,
    Other,
}

/// Result of probing a path without following the final symlink component
/// (TS `PathLinkInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct PathLinkInfo {
    pub version: FsVersion,
    pub mode: u32,
    pub kind: PathLinkKind,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathLinkKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// One local directory child with a resolved target and cheap metadata (TS
/// `LocalDirEntry`).
#[derive(Debug, Clone, PartialEq)]
pub struct LocalDirEntry {
    pub name: String,
    pub kind: PathKind,
    pub target: LocalTarget,
    pub version: Option<FsVersion>,
    pub size: Option<u64>,
}

/// Opaque version token from high-resolution identity and freshness
/// metadata (TS `versionOf`).
fn version_of(info: &std::fs::Metadata) -> FsVersion {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs_version(format!(
            "{}:{}:{}:{}:{}",
            info.dev(),
            info.ino(),
            info.size(),
            info.mtime_nsec(),
            info.ctime_nsec()
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        fs_version(format!(
            "{}:{}:{}",
            info.file_size(),
            info.last_write_time(),
            info.creation_time()
        ))
    }
}

fn abort(verb: &str) -> FsError {
    FsError::new(format!("{verb} aborted"), FsErrorCode::FsAborted)
}

fn throw_if_aborted(signal: Option<&FsAbort>, verb: &str) -> Result<(), FsError> {
    if signal.is_some_and(|signal| signal()) {
        return Err(abort(verb));
    }
    Ok(())
}

/// Resolve a path to its absolute display path and realpath identity. For a
/// missing target, realpath the nearest existing ancestor and append the
/// missing suffix, preserving identity across symlinked ancestors before and
/// after creation.
pub async fn resolve_local_target(cwd: &str, path: &str) -> Result<LocalTarget, FsError> {
    if path.trim().is_empty() {
        return Err(FsError::new(
            "file_path must be a non-empty string",
            FsErrorCode::FsNotFound,
        ));
    }
    let display_path = {
        let joined = Path::new(cwd).join(path);
        std::path::absolute(&joined)
            .unwrap_or(joined)
            .to_string_lossy()
            .into_owned()
    };
    match tokio::fs::canonicalize(&display_path).await {
        Ok(real) => {
            return Ok(LocalTarget {
                target_key: fs_target_key(real.to_string_lossy().into_owned()),
                display_path,
            });
        }
        Err(error) => {
            // A path component is a file, not a directory: the target can
            // neither exist nor be created 鈥?surface the structured taxonomy
            // instead of a raw error.
            if let Some(parent) = Path::new(&display_path).parent() {
                if tokio::fs::metadata(parent)
                    .await
                    .is_ok_and(|meta| !meta.is_dir())
                {
                    return Err(FsError::new(
                        format!(
                            "cannot resolve \"{display_path}\": a parent path segment is not a directory"
                        ),
                        FsErrorCode::FsNotFound,
                    ));
                }
            }
            if !is_not_found(&error) {
                return Err(io_to_fs_error(error));
            }
        }
    }
    // File absent: realpath the nearest existing ancestor and re-append the
    // missing suffix so the key is stable across creation of those dirs.
    let mut missing: Vec<String> = vec![
        Path::new(&display_path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ];
    let mut ancestor = Path::new(&display_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    loop {
        match tokio::fs::canonicalize(&ancestor).await {
            Ok(real_ancestor) => {
                if tokio::fs::metadata(&real_ancestor)
                    .await
                    .is_ok_and(|meta| !meta.is_dir())
                {
                    return Err(FsError::new(
                        format!(
                            "cannot resolve \"{display_path}\": a parent path segment is not a directory"
                        ),
                        FsErrorCode::FsNotFound,
                    ));
                }
                let mut key = real_ancestor;
                for segment in missing.iter().rev() {
                    key = key.join(segment);
                }
                return Ok(LocalTarget {
                    target_key: fs_target_key(key.to_string_lossy().into_owned()),
                    display_path,
                });
            }
            Err(error) => {
                if !is_not_found(&error) {
                    return Err(io_to_fs_error(error));
                }
                let parent = ancestor
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| ancestor.clone());
                if parent == ancestor {
                    return Ok(LocalTarget {
                        target_key: fs_target_key(display_path.clone()),
                        display_path,
                    });
                }
                missing.push(
                    ancestor
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                ancestor = parent;
            }
        }
    }
}

fn kind_of(info: &std::fs::Metadata) -> PathKind {
    if info.is_file() {
        PathKind::File
    } else if info.is_dir() {
        PathKind::Directory
    } else {
        PathKind::Other
    }
}

fn link_kind_of(info: &std::fs::Metadata) -> PathLinkKind {
    if info.file_type().is_symlink() {
        PathLinkKind::Symlink
    } else if info.is_file() {
        PathLinkKind::File
    } else if info.is_dir() {
        PathLinkKind::Directory
    } else {
        PathLinkKind::Other
    }
}

/// Probe a path for its version, mode, type, and size. `None` if absent
/// (the path 鈥?or a parent segment 鈥?does not exist).
pub async fn probe(absolute_path: &str) -> Result<Option<PathInfo>, FsError> {
    match tokio::fs::metadata(absolute_path).await {
        Ok(info) => Ok(Some(PathInfo {
            version: version_of(&info),
            #[cfg(unix)]
            mode: {
                use std::os::unix::fs::PermissionsExt;
                info.permissions().mode() & 0o777
            },
            #[cfg(windows)]
            mode: 0,
            kind: kind_of(&info),
            size: info.len(),
        })),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(io_to_fs_error(error)),
    }
}

/// Probe a path without following the final symlink component.
pub async fn probe_no_follow(absolute_path: &str) -> Result<Option<PathLinkInfo>, FsError> {
    match tokio::fs::symlink_metadata(absolute_path).await {
        Ok(info) => Ok(Some(PathLinkInfo {
            version: version_of(&info),
            #[cfg(unix)]
            mode: {
                use std::os::unix::fs::PermissionsExt;
                info.permissions().mode() & 0o777
            },
            #[cfg(windows)]
            mode: 0,
            kind: link_kind_of(&info),
            size: info.len(),
        })),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(io_to_fs_error(error)),
    }
}

fn listing_io_error(display_path: &str, error: std::io::Error) -> FsError {
    let error = std::io::Error::from(error);
    match error.kind() {
        std::io::ErrorKind::NotFound => FsError::with_cause(
            format!("cannot list \"{display_path}\": not found"),
            FsErrorCode::FsNotFound,
            Box::new(error),
        ),
        std::io::ErrorKind::PermissionDenied => FsError::with_cause(
            format!("cannot list \"{display_path}\": permission denied"),
            FsErrorCode::FsPermissionDenied,
            Box::new(error),
        ),
        _ => FsError::with_cause(
            format!("cannot list \"{display_path}\": {error}"),
            FsErrorCode::FsIoError,
            Box::new(error),
        ),
    }
}

async fn resolve_listed_child_target(
    parent: &LocalTarget,
    name: &str,
) -> Result<LocalTarget, FsError> {
    let identity = resolve_local_target(parent.target_key.as_str(), name).await?;
    Ok(LocalTarget {
        display_path: Path::new(&parent.display_path)
            .join(name)
            .to_string_lossy()
            .into_owned(),
        target_key: identity.target_key,
    })
}

/// List direct children of a directory in stable name order. Each child
/// includes a resolved target plus stat metadata when still available; file
/// contents are never read.
pub async fn list_directory(
    target: &LocalTarget,
    signal: Option<&FsAbort>,
) -> Result<Vec<LocalDirEntry>, FsError> {
    throw_if_aborted(signal, "list")?;
    let info = probe(target.target_key.as_str()).await.map_err(|error| {
        let io = std::io::Error::other(error.to_string());
        listing_io_error(&target.display_path, io)
    })?;
    let Some(info) = info else {
        return Err(FsError::new(
            format!("cannot list \"{}\": not found", target.display_path),
            FsErrorCode::FsNotFound,
        ));
    };
    if info.kind != PathKind::Directory {
        return Err(FsError::new(
            format!("cannot list \"{}\": not a directory", target.display_path),
            FsErrorCode::FsNotDirectory,
        ));
    }

    let mut names: Vec<String> = Vec::new();
    let read = tokio::fs::read_dir(target.target_key.as_str()).await;
    let mut entries = match read {
        Ok(entries) => entries,
        Err(error) => return Err(listing_io_error(&target.display_path, error)),
    };
    loop {
        match entries.next_entry().await {
            Ok(Some(entry)) => names.push(entry.file_name().to_string_lossy().into_owned()),
            Ok(None) => break,
            Err(error) => return Err(listing_io_error(&target.display_path, error)),
        }
    }
    throw_if_aborted(signal, "list")?;
    names.sort();

    let mut result = Vec::new();
    for name in names {
        throw_if_aborted(signal, "list")?;
        let child_target = match resolve_listed_child_target(target, &name).await {
            Ok(child) => child,
            Err(error) => {
                let io = std::io::Error::other(error.to_string());
                return Err(listing_io_error(
                    &Path::new(&target.display_path)
                        .join(&name)
                        .to_string_lossy(),
                    io,
                ));
            }
        };
        let child_info = match probe(child_target.target_key.as_str()).await {
            Ok(info) => info,
            Err(error) => {
                let io = std::io::Error::other(error.to_string());
                return Err(listing_io_error(
                    &Path::new(&target.display_path)
                        .join(&name)
                        .to_string_lossy(),
                    io,
                ));
            }
        };
        result.push(LocalDirEntry {
            name,
            kind: child_info
                .as_ref()
                .map(|info| info.kind)
                .unwrap_or(PathKind::Other),
            target: child_target,
            version: child_info.as_ref().map(|info| info.version.clone()),
            size: child_info
                .as_ref()
                .filter(|info| info.kind == PathKind::File)
                .map(|info| info.size),
        });
        throw_if_aborted(signal, "list")?;
    }
    Ok(result)
}

fn not_text_error(verb: &str, display_path: &str) -> FsError {
    FsError::new(
        format!("cannot {verb} \"{display_path}\": invalid UTF-8 text"),
        FsErrorCode::FsNotText,
    )
}

async fn stat_regular_file(
    target: &LocalTarget,
    verb: &str,
    signal: Option<&FsAbort>,
) -> Result<std::fs::Metadata, FsError> {
    throw_if_aborted(signal, verb)?;
    match tokio::fs::metadata(target.target_key.as_str()).await {
        Ok(info) if info.is_file() => Ok(info),
        Ok(_) => Err(FsError::new(
            format!(
                "cannot {verb} \"{}\": not a regular file",
                target.display_path
            ),
            FsErrorCode::FsNotRegularFile,
        )),
        Err(error) if is_not_found(&error) => Err(FsError::new(
            format!("cannot {verb} \"{}\": not found", target.display_path),
            FsErrorCode::FsNotFound,
        )),
        Err(error) => Err(io_to_fs_error(error)),
    }
}

/// Read a whole regular UTF-8 text file into a single decoded string.
/// Rejects non-regular files, invalid UTF-8, and NUL-byte binary samples.
pub async fn read_whole_text(
    target: &LocalTarget,
    signal: Option<&FsAbort>,
) -> Result<String, FsError> {
    stat_regular_file(target, "read", signal).await?;
    let raw = tokio::fs::read(target.target_key.as_str())
        .await
        .map_err(|error| {
            if is_not_found(&error) {
                FsError::new(
                    format!("cannot read \"{}\": not found", target.display_path),
                    FsErrorCode::FsNotFound,
                )
            } else {
                io_to_fs_error(error)
            }
        })?;
    throw_if_aborted(signal, "read")?;
    if raw.iter().take(BINARY_SAMPLE_BYTES).any(|byte| *byte == 0) {
        return Err(FsError::new(
            format!("cannot read \"{}\": binary file", target.display_path),
            FsErrorCode::FsNotText,
        ));
    }
    String::from_utf8(raw).map_err(|_| not_text_error("read", &target.display_path))
}

/// Read a whole regular file as raw bytes with no decoding or binary
/// rejection. `max_bytes` bounds the complete content: the stat size
/// short-circuits an oversized file before any content I/O, and the stream
/// reads at most one byte beyond the cap so a file growing after stat cannot
/// cause unbounded buffering.
pub async fn read_whole_bytes(
    target: &LocalTarget,
    signal: Option<&FsAbort>,
    max_bytes: u64,
    internals: &FsIoInternals,
) -> Result<Vec<u8>, FsError> {
    let info = stat_regular_file(target, "read", signal).await?;
    if info.len() > max_bytes {
        return Err(FsError::new(
            format!(
                "cannot read \"{}\": {} bytes exceeds the {max_bytes}-byte limit",
                target.display_path,
                info.len()
            ),
            FsErrorCode::FsTooLarge,
        ));
    }
    if let Some(hook) = &internals.inspect_read_bytes_after_stat {
        (hook)(target)
            .await
            .map_err(|error| FsError::new(error, FsErrorCode::FsIoError))?;
    }
    let mut file = tokio::fs::File::open(target.target_key.as_str())
        .await
        .map_err(|error| {
            if is_not_found(&error) {
                FsError::new(
                    format!("cannot read \"{}\": not found", target.display_path),
                    FsErrorCode::FsNotFound,
                )
            } else {
                io_to_fs_error(error)
            }
        })?;
    let mut chunks = Vec::new();
    let mut bytes: u64 = 0;
    let mut buffer = vec![0u8; (max_bytes as usize).min(64 * 1024).max(1)];
    loop {
        use tokio::io::AsyncReadExt;
        throw_if_aborted(signal, "read")?;
        let read = file.read(&mut buffer).await.map_err(io_to_fs_error)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > max_bytes {
            return Err(FsError::new(
                format!(
                    "cannot read \"{}\": content exceeds the {max_bytes}-byte limit",
                    target.display_path
                ),
                FsErrorCode::FsTooLarge,
            ));
        }
        chunks.extend_from_slice(&buffer[..read]);
    }
    Ok(chunks)
}

/// Incremental strict UTF-8 chunk decoder (the TS streaming `TextDecoder`
/// `{ fatal: true, stream: true }` equivalent): incomplete trailing
/// sequences ride into the next chunk; invalid bytes reject.
struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    fn push(&mut self, chunk: &[u8], verb: &str, display_path: &str) -> Result<String, FsError> {
        let mut combined = std::mem::take(&mut self.pending);
        combined.extend_from_slice(chunk);
        match std::str::from_utf8(&combined) {
            Ok(text) => {
                self.pending = Vec::new();
                Ok(text.to_string())
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if error.error_len().is_none() {
                    // Incomplete trailing sequence: hold it for the next chunk.
                    let held = combined.split_off(valid);
                    let text = String::from_utf8(combined)
                        .map_err(|_| not_text_error(verb, display_path))?;
                    self.pending = held;
                    Ok(text)
                } else {
                    Err(not_text_error(verb, display_path))
                }
            }
        }
    }

    fn finish(&mut self, verb: &str, display_path: &str) -> Result<String, FsError> {
        if self.pending.is_empty() {
            return Ok(String::new());
        }
        Err(not_text_error(verb, display_path))
    }
}

/// Stream a whole regular UTF-8 text file as decoded text chunks. Same text
/// semantics as [`read_whole_text`], but never holds the whole file in
/// memory.
pub async fn stream_whole_text(
    target: &LocalTarget,
    signal: Option<&FsAbort>,
) -> Result<futures::stream::BoxStream<'static, Result<String, FsError>>, FsError> {
    stat_regular_file(target, "read", signal).await?;
    let file = match tokio::fs::File::open(target.target_key.as_str()).await {
        Ok(file) => file,
        Err(error) if is_not_found(&error) => {
            return Err(FsError::new(
                format!("cannot read \"{}\": not found", target.display_path),
                FsErrorCode::FsNotFound,
            ));
        }
        Err(error) => return Err(io_to_fs_error(error)),
    };
    let display_path = target.display_path.clone();
    let signal = signal.cloned();
    let stream = futures::stream::try_unfold(
        (file, Utf8StreamDecoder::new(), 0usize),
        move |(mut file, mut decoder, mut sampled)| {
            let signal = signal.clone();
            let display_path = display_path.clone();
            async move {
                use tokio::io::AsyncReadExt;
                if signal.as_ref().is_some_and(|signal| signal()) {
                    return Err(abort("read"));
                }
                let mut buffer = vec![0u8; 64 * 1024];
                let read = file.read(&mut buffer).await.map_err(io_to_fs_error)?;
                if read == 0 {
                    let tail = decoder.finish("read", &display_path)?;
                    if tail.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some((tail, (file, decoder, sampled))));
                }
                let chunk = &buffer[..read];
                if sampled < BINARY_SAMPLE_BYTES {
                    let take = chunk.len().min(BINARY_SAMPLE_BYTES - sampled);
                    if chunk[..take].contains(&0) {
                        return Err(FsError::new(
                            format!("cannot read \"{display_path}\": binary file"),
                            FsErrorCode::FsNotText,
                        ));
                    }
                    sampled += take;
                }
                let text = decoder.push(chunk, "read", &display_path)?;
                Ok(Some((text, (file, decoder, sampled))))
            }
        },
    );
    Ok(Box::pin(stream))
}

/// Line ending style detected before LF normalization (TS `LineEndings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEndings {
    Lf,
    Crlf,
}

/// Collapse CRLF to LF 鈥?the canonical in-memory form every edit/diff basis
/// uses. Lone `\r` bytes (not followed by `\n`) are left untouched.
pub fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n")
}

fn detect_line_endings(raw: &str) -> LineEndings {
    let sample: String = raw.chars().take(4096).collect();
    let crlf_count = sample.match_indices("\r\n").count();
    let lf_count = sample.matches('\n').count() - crlf_count;
    if crlf_count > lf_count {
        LineEndings::Crlf
    } else {
        LineEndings::Lf
    }
}

/// Convert LF-normalized content back to the line-ending style detected at
/// read time, for write-back.
pub fn restore_line_endings(content: &str, line_endings: LineEndings) -> String {
    match line_endings {
        LineEndings::Lf => content.to_string(),
        LineEndings::Crlf => normalize_line_endings(content).replace('\n', "\r\n"),
    }
}

fn count_occurrences(content: &str, needle: &str) -> usize {
    content.match_indices(needle).count()
}

/// Read and decode a file for editing: rejects binaries, returns
/// LF-normalized content plus the original line-ending style for write-back.
pub async fn read_for_edit(
    absolute_path: &str,
    display_path: &str,
    signal: Option<&FsAbort>,
) -> Result<(String, LineEndings), FsError> {
    throw_if_aborted(signal, "edit")?;
    let buffer = tokio::fs::read(absolute_path).await.map_err(|error| {
        if is_not_found(&error) {
            FsError::new(
                format!("cannot edit \"{display_path}\": not found"),
                FsErrorCode::FsNotFound,
            )
        } else {
            io_to_fs_error(error)
        }
    })?;
    throw_if_aborted(signal, "edit")?;
    if buffer.contains(&0) {
        return Err(FsError::new(
            format!("cannot edit \"{display_path}\": binary file"),
            FsErrorCode::FsNotText,
        ));
    }
    let raw = String::from_utf8(buffer).map_err(|_| not_text_error("edit", display_path))?;
    Ok((normalize_line_endings(&raw), detect_line_endings(&raw)))
}

/// Best-effort overwrite diff basis. Binary, invalid UTF-8, a file at/above
/// the byte limit, or a file deleted/made unreadable after the caller's
/// preflight returns `None` so the write still succeeds and presentation
/// falls back to a whole-file diff. The bound is enforced on the opened
/// descriptor rather than a prior path stat.
pub async fn read_text_for_diff(
    absolute_path: &str,
    max_bytes: u64,
    signal: Option<&FsAbort>,
) -> Result<Option<String>, FsError> {
    throw_if_aborted(signal, "read")?;
    let outcome: Result<Option<String>, std::io::Error> = async {
        use tokio::io::AsyncReadExt;
        let mut handle = tokio::fs::File::open(absolute_path).await?;
        throw_if_aborted(signal, "read").map_err(std::io::Error::other)?;
        let info = handle.metadata().await?;
        throw_if_aborted(signal, "read").map_err(std::io::Error::other)?;
        if !info.is_file() {
            return Ok(None);
        }
        if info.len() >= max_bytes {
            return Ok(None);
        }
        let opened_size = info.len();
        let mut buffer = vec![0u8; (opened_size as usize) + 1];
        let mut total = 0usize;
        while total < buffer.len() {
            throw_if_aborted(signal, "read").map_err(std::io::Error::other)?;
            let length = (buffer.len() - total).min(DIFF_BASIS_READ_CHUNK_BYTES);
            let read = handle.read(&mut buffer[total..total + length]).await?;
            if read == 0 {
                break;
            }
            total += read;
        }
        drop(handle);
        throw_if_aborted(signal, "read").map_err(std::io::Error::other)?;
        if total != opened_size as usize {
            return Ok(None);
        }
        let basis = &buffer[..total];
        if basis.contains(&0) {
            return Ok(None);
        }
        match std::str::from_utf8(basis) {
            Ok(text) => Ok(Some(normalize_line_endings(text))),
            Err(_) => Ok(None),
        }
    }
    .await;
    match outcome {
        Ok(basis) => Ok(basis),
        // A descriptor-phase errno 鈥?deleted or made unreadable after the
        // caller's preflight, or a faulted read 鈥?costs only the optional
        // basis: a committed write must not fail for a presentation-only
        // pre-read.
        Err(_) => Ok(None),
    }
}

/// Apply a literal replacement to LF-normalized content. Empty or missing
/// search text throws `FS_EDIT_NOT_FOUND`; multiple matches throw
/// `FS_AMBIGUOUS_EDIT` unless `replace_all` is true.
pub fn apply_literal_edit(
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    display_path: &str,
) -> Result<(String, usize), FsError> {
    let old_norm = normalize_line_endings(old_string);
    if old_norm.is_empty() {
        return Err(FsError::new(
            "old_string must be a non-empty string",
            FsErrorCode::FsEditNotFound,
        ));
    }
    let new_norm = normalize_line_endings(new_string);
    let replacements = count_occurrences(content, &old_norm);
    if replacements == 0 {
        return Err(FsError::new(
            format!("old_string was not found in \"{display_path}\""),
            FsErrorCode::FsEditNotFound,
        ));
    }
    if !replace_all && replacements > 1 {
        return Err(FsError::new(
            format!(
                "old_string matched {replacements} times in \"{display_path}\"; provide a more specific old_string or set replace_all to true"
            ),
            FsErrorCode::FsAmbiguousEdit,
        ));
    }
    Ok((content.replace(&old_norm, &new_norm), replacements))
}

async fn remove_staging_dir_or_throw(
    staging_dir: &Path,
    failure: FsError,
    internals: &FsIoInternals,
) -> FsError {
    if let Some(hook) = &internals.remove_staging_dir {
        if let Err(cleanup) = (hook)(staging_dir).await {
            return FsError::new(
                format!("write failed ({failure}) and temp cleanup failed ({cleanup})"),
                FsErrorCode::FsNotFound,
            );
        }
    } else {
        let _ = tokio::fs::remove_dir_all(staging_dir).await;
    }
    failure
}

async fn default_link_file(existing: &Path, new: &Path) -> Result<(), String> {
    // A fast synchronous syscall; blocking a worker briefly is cheaper than
    // a spawn and works on every runtime flavor.
    std::fs::hard_link(existing, new).map_err(|error| error.to_string())
}

async fn default_inspect_publication_target(path: &Path) -> Result<(), String> {
    let meta = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| error.to_string())?;
    let _ = meta;
    Ok(())
}

async fn default_remove_staging_dir(path: &Path) -> Result<(), String> {
    tokio::fs::remove_dir_all(path)
        .await
        .map_err(|error| error.to_string())
}

/// Atomically replace a file through a private, synced staging file in the
/// same directory. POSIX protects the staging directory and file with `0700`
/// and `0600`. A new Windows file inherits the destination directory's DACL
/// (simplified; see the module deviations).
pub async fn write_file_atomic(
    absolute_path: &str,
    content: &str,
    mode: Option<u32>,
    signal: Option<&FsAbort>,
    internals: &FsIoInternals,
    create_if_absent: Option<&LocalTarget>,
) -> Result<(), FsError> {
    throw_if_aborted(signal, "write")?;
    let directory = Path::new(absolute_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|error| FsError::new(format!("write failed: {error}"), FsErrorCode::FsIoError))?;

    throw_if_aborted(signal, "write")?;
    let basename = Path::new(absolute_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staging_dir_name = match &internals.temp_dir_name {
        Some(hook) => hook(absolute_path),
        None => format!(
            ".{basename}.{}.{}.tmpdir",
            std::process::id(),
            uuid::Uuid::new_v4()
        ),
    };
    let staging_dir = directory.join(staging_dir_name);
    let temp_name = match &internals.temp_name {
        Some(hook) => hook(absolute_path),
        None => format!("{basename}.tmp"),
    };
    let temp_path = staging_dir.join(temp_name);
    let platform = internals
        .platform
        .clone()
        .unwrap_or_else(|| std::env::consts::OS.to_string());

    let staging_created = {
        match tokio::fs::create_dir(&staging_dir).await {
            Ok(()) => true,
            Err(error) => {
                return Err(FsError::new(
                    format!("write failed: {error}"),
                    FsErrorCode::FsIoError,
                ));
            }
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            tokio::fs::set_permissions(&staging_dir, std::fs::Permissions::from_mode(0o700)).await;
    }

    let write_outcome: Result<(), FsError> = async {
        let mut file = match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .await
        {
            Ok(file) => file,
            Err(error) => {
                return Err(FsError::new(
                    format!("write failed: {error}"),
                    FsErrorCode::FsIoError,
                ));
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .await;
        }
        if platform == "windows" && mode.is_some() {
            // Simplified Windows boundary: the DACL copy is a no-op until the
            // sandbox-windows-acl milestone.
            let _ = copy_file_dacl_win32(Path::new(absolute_path), &temp_path).await;
        }
        {
            use tokio::io::AsyncWriteExt;
            file.write_all(content.as_bytes()).await.map_err(|error| {
                FsError::new(format!("write failed: {error}"), FsErrorCode::FsIoError)
            })?;
            file.sync_all().await.map_err(|error| {
                FsError::new(format!("write failed: {error}"), FsErrorCode::FsIoError)
            })?;
        }
        if let Some(hook) = &internals.inspect_temp {
            (hook)(&StagedPaths {
                staging_dir: staging_dir.to_string_lossy().into_owned(),
                temp_path: temp_path.to_string_lossy().into_owned(),
            })
            .await
            .map_err(|error| FsError::new(error, FsErrorCode::FsIoError))?;
        }
        if let Some(_mode) = mode {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = file
                    .set_permissions(std::fs::Permissions::from_mode(_mode))
                    .await;
            }
        }
        drop(file);

        throw_if_aborted(signal, "write")?;
        if let Some(create_target) = create_if_absent {
            let linked = match &internals.link_file {
                Some(hook) => (hook)(&temp_path, Path::new(absolute_path)).await,
                None => default_link_file(&temp_path, Path::new(absolute_path)).await,
            };
            if let Err(error) = linked {
                // Inspect the target entry after failure so a collision is
                // not confused with missing hard-link support.
                let inspected = match &internals.inspect_publication_target {
                    Some(hook) => (hook)(Path::new(absolute_path)).await,
                    None => default_inspect_publication_target(Path::new(absolute_path)).await,
                };
                if inspected.is_ok() {
                    let kind = tokio::fs::metadata(absolute_path).await.ok().map(|meta| {
                        if meta.is_file() {
                            PathKind::File
                        } else {
                            PathKind::Other
                        }
                    });
                    if kind != Some(PathKind::File) {
                        return Err(FsError::new(
                            format!(
                                "cannot write \"{}\": not a regular file",
                                create_target.display_path
                            ),
                            FsErrorCode::FsNotRegularFile,
                        ));
                    }
                    return Err(FsError::new(
                        format!(
                            "cannot overwrite existing \"{}\" without reading it first",
                            create_target.display_path
                        ),
                        FsErrorCode::FsNotObserved,
                    ));
                }
                if tokio::fs::symlink_metadata(absolute_path).await.is_ok() {
                    return Err(FsError::new(
                        format!(
                            "cannot overwrite existing \"{}\" without reading it first",
                            create_target.display_path
                        ),
                        FsErrorCode::FsNotObserved,
                    ));
                }
                return Err(FsError::new(
                    format!("cannot write \"{}\": {error}", create_target.display_path),
                    FsErrorCode::FsIoError,
                ));
            }
        } else if platform == "windows" && mode.is_some() {
            // Simplified Windows replacement: remove-then-rename (the ACL
            // preservation lands with the windows-acl milestone).
            if let Err(error) = replace_file_win32(Path::new(absolute_path), &temp_path).await {
                return Err(FsError::new(
                    format!("write failed: {error}"),
                    FsErrorCode::FsIoError,
                ));
            }
        } else if let Err(error) = tokio::fs::rename(&temp_path, absolute_path).await {
            return Err(FsError::new(
                format!("write failed: {error}"),
                FsErrorCode::FsIoError,
            ));
        }
        // The target is committed; owner-only staging residue cannot turn
        // that write into a failure.
        match &internals.remove_staging_dir {
            Some(hook) => {
                let _ = (hook)(&staging_dir).await;
            }
            None => {
                let _ = default_remove_staging_dir(&staging_dir).await;
            }
        }
        Ok(())
    }
    .await;

    match write_outcome {
        Ok(()) => Ok(()),
        Err(failure) => {
            if !staging_created {
                return Err(failure);
            }
            Err(remove_staging_dir_or_throw(&staging_dir, failure, internals).await)
        }
    }
}

fn io_to_fs_error(error: std::io::Error) -> FsError {
    match error.kind() {
        std::io::ErrorKind::NotFound => FsError::new(error.to_string(), FsErrorCode::FsNotFound),
        std::io::ErrorKind::PermissionDenied => {
            FsError::new(error.to_string(), FsErrorCode::FsPermissionDenied)
        }
        _ => FsError::new(error.to_string(), FsErrorCode::FsIoError),
    }
}
