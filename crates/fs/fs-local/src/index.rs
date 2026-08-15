//! Host-filesystem implementation of `ctx.fs`. Realpath-derived target
//! identity makes aliases share stale guards, and writes through a symlink
//! update its target without replacing the link. Rust port of
//! `packages/fs/fs-local/src/index.ts`.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::Context;
use parking_lot::Mutex;

use dsh_fs::{
    AbortPredicate, FsDirEntry, FsEditOutcome, FsEditRequest, FsError, FsErrorCode, FsInfo,
    FsInfoType, FsPathInfo, FsPathInfoType, FsTarget, FsVersion, FsWriteIntent,
    FsWriteOperation, FsWriteOutcome, FileSystem, LstatOptions, ResolveOptions, fs_version,
};
use dsh_sandbox::SandboxExecutionPolicy;

use crate::fsio::{
    FsIoInternals, LocalTarget, PathKind, PathLinkKind, apply_literal_edit, list_directory,
    normalize_line_endings, probe, probe_no_follow, read_for_edit, read_text_for_diff,
    read_whole_bytes, read_whole_text, resolve_local_target, restore_line_endings,
    stream_whole_text, write_file_atomic,
};

/// Configuration for the local filesystem backend (TS `Config`).
#[derive(Debug, Clone)]
pub struct Config {
    /// Base directory for relative paths. Defaults to the process cwd.
    pub cwd: Option<String>,
    /// Exclusive UTF-8 byte limit on each overwrite-diff side. Defaults to
    /// 10 MiB.
    pub diff_basis_max_bytes: Option<u64>,
}

const DEFAULT_DIFF_BASIS_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// The runtime's safe allocation/decode maximum the config is capped by.
const MAX_DIFF_BASIS_BYTES: u64 = 1 << 30;

/// The host-filesystem backend. Reads resolve relative paths from
/// [`Config::cwd`] (a resolution default, NOT a containment boundary);
/// enforce containment with a stricter backend or a `tools/execute`
/// permission plugin.
pub struct LocalFileSystem {
    /// Validated config (defaults applied before construction).
    pub config: ResolvedConfig,
    /// Test hook forwarded to fsio for atomic-publication boundaries.
    pub internals: FsIoInternals,
    /// Per-targetKey tail lock: serializes mutating ops so the
    /// read→guard→write window can't interleave, making concurrent
    /// writes/edits deterministically ordered.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// The fully-resolved config (TS `ResolvedConfig`).
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub cwd: String,
    pub diff_basis_max_bytes: u64,
}

impl LocalFileSystem {
    /// Construct the backend and validate the config WITHOUT registering a
    /// service (the TS `super(ctx, config)` half). A wrapping backend
    /// (`dsh-fs-sandbox`) builds through this and registers its own erased
    /// handle.
    pub fn build(config: Config) -> Result<Arc<Self>, String> {
        let cwd = config
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().map(|cwd| cwd.to_string_lossy().into_owned()).unwrap_or_else(|_| ".".to_string()));
        let diff_basis_max_bytes = config.diff_basis_max_bytes.unwrap_or(DEFAULT_DIFF_BASIS_MAX_BYTES);
        if diff_basis_max_bytes == 0 || diff_basis_max_bytes > MAX_DIFF_BASIS_BYTES {
            return Err(format!(
                "fs-local: diffBasisMaxBytes must be a positive integer no greater than {MAX_DIFF_BASIS_BYTES}"
            ));
        }
        Ok(Arc::new(Self {
            config: ResolvedConfig { cwd, diff_basis_max_bytes },
            internals: FsIoInternals::default(),
            locks: Mutex::new(HashMap::new()),
        }))
    }

    /// Construct the backend, validate the config, and register as
    /// `ctx.fs` (the TS constructor + `super(ctx)` collapse).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let backend = Self::build(config)?;
        let erased: Arc<dyn FileSystem> = backend.clone();
        ctx.register_service(erased);
        Ok(backend)
    }

    /// Run `op` with exclusive access to `target_key` (FIFO per key).
    async fn with_lock<T, F>(&self, target_key: &str, op: F) -> Result<T, FsError>
    where
        F: std::future::Future<Output = Result<T, FsError>>,
    {
        let lock = {
            let mut locks = self.locks.lock();
            locks
                .entry(target_key.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        op.await
    }

    /// The post-write version probe; falls back to a sentinel when a
    /// concurrent unlink removed the target between rename and stat.
    fn version_after_write(&self, after: Option<dsh_fs::FsVersion>, target: &FsTarget) -> FsVersion {
        after.unwrap_or_else(|| fs_version(format!("missing:{}", target.target_key)))
    }
}

#[async_trait::async_trait]
impl FileSystem for LocalFileSystem {
    async fn resolve(&self, path: &str, opts: Option<&ResolveOptions>) -> Result<FsTarget, FsError> {
        if opts.and_then(|opts| opts.signal.as_ref()).is_some_and(|signal| signal()) {
            return Err(FsError::new("resolve aborted", FsErrorCode::FsAborted));
        }
        let cwd = opts.and_then(|opts| opts.cwd.as_deref()).unwrap_or(&self.config.cwd);
        let local = resolve_local_target(cwd, path).await?;
        if opts.and_then(|opts| opts.signal.as_ref()).is_some_and(|signal| signal()) {
            return Err(FsError::new("resolve aborted", FsErrorCode::FsAborted));
        }
        Ok(FsTarget {
            target_key: local.target_key,
            display_path: local.display_path,
        })
    }

    fn process_path(&self, target: &FsTarget) -> String {
        target.target_key.to_string()
    }

    fn file_url(&self, target: &FsTarget) -> String {
        // The TS `pathToFileURL` percent-encodes the path into a file:// URL.
        let path = self.process_path(target);
        format!("file:///{}", path.replace('\\', "/").split('/').map(|segment| {
            let mut encoded = String::new();
            for byte in segment.as_bytes() {
                match byte {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' => {
                        encoded.push(*byte as char)
                    }
                    _ => encoded.push_str(&format!("%{byte:02X}")),
                }
            }
            encoded
        }).collect::<Vec<_>>().join("/"))
    }

    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        let parent = self.process_path(parent);
        let child = self.process_path(child);
        if parent == child {
            return true;
        }
        let sep = std::path::MAIN_SEPARATOR;
        let prefix = format!("{parent}{sep}");
        child.starts_with(&prefix)
    }

    async fn stat(&self, target: &FsTarget, signal: Option<AbortPredicate>) -> Result<Option<FsInfo>, FsError> {
        if signal.as_ref().is_some_and(|signal| signal()) {
            return Err(FsError::new("stat aborted", FsErrorCode::FsAborted));
        }
        let info = probe(target.target_key.as_str()).await?;
        if signal.as_ref().is_some_and(|signal| signal()) {
            return Err(FsError::new("stat aborted", FsErrorCode::FsAborted));
        }
        Ok(info.map(|info| FsInfo {
            version: info.version,
            kind: match info.kind {
                PathKind::File => FsInfoType::File,
                PathKind::Directory => FsInfoType::Directory,
                PathKind::Other => FsInfoType::Other,
            },
            size: Some(info.size),
        }))
    }

    async fn lstat(
        &self,
        path: &str,
        opts: Option<&LstatOptions>,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsPathInfo>, FsError> {
        if signal.as_ref().is_some_and(|signal| signal()) {
            return Err(FsError::new("lstat aborted", FsErrorCode::FsAborted));
        }
        if path.trim().is_empty() {
            return Err(FsError::new("file_path must be a non-empty string", FsErrorCode::FsNotFound));
        }
        let cwd = opts.and_then(|opts| opts.cwd.as_deref()).unwrap_or(&self.config.cwd);
        let joined = Path::new(cwd).join(path);
        let info = probe_no_follow(&joined.to_string_lossy()).await?;
        if signal.as_ref().is_some_and(|signal| signal()) {
            return Err(FsError::new("lstat aborted", FsErrorCode::FsAborted));
        }
        Ok(info.map(|info| FsPathInfo {
            version: info.version,
            kind: match info.kind {
                PathLinkKind::File => FsPathInfoType::File,
                PathLinkKind::Directory => FsPathInfoType::Directory,
                PathLinkKind::Symlink => FsPathInfoType::Symlink,
                PathLinkKind::Other => FsPathInfoType::Other,
            },
            size: Some(info.size),
        }))
    }

    async fn read_text(&self, target: &FsTarget, signal: Option<AbortPredicate>) -> Result<String, FsError> {
        let local = LocalTarget {
            display_path: target.display_path.clone(),
            target_key: target.target_key.clone(),
        };
        read_whole_text(&local, signal.as_ref()).await
    }

    async fn stream_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<futures::stream::BoxStream<'static, Result<String, FsError>>, FsError> {
        let local = LocalTarget {
            display_path: target.display_path.clone(),
            target_key: target.target_key.clone(),
        };
        stream_whole_text(&local, signal.as_ref()).await
    }

    async fn read_bytes(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        let local = LocalTarget {
            display_path: target.display_path.clone(),
            target_key: target.target_key.clone(),
        };
        read_whole_bytes(&local, signal.as_ref(), max_bytes, &self.internals).await
    }

    async fn list_dir(&self, target: &FsTarget, signal: Option<AbortPredicate>) -> Result<Vec<FsDirEntry>, FsError> {
        let local = LocalTarget {
            display_path: target.display_path.clone(),
            target_key: target.target_key.clone(),
        };
        let entries = list_directory(&local, signal.as_ref()).await?;
        Ok(entries
            .into_iter()
            .map(|entry| FsDirEntry {
                name: entry.name,
                kind: match entry.kind {
                    PathKind::File => FsInfoType::File,
                    PathKind::Directory => FsInfoType::Directory,
                    PathKind::Other => FsInfoType::Other,
                },
                target: FsTarget {
                    target_key: entry.target.target_key,
                    display_path: entry.target.display_path,
                },
                version: entry.version,
                size: entry.size,
            })
            .collect())
    }

    async fn write_text(
        &self,
        target: &FsTarget,
        content: &str,
        expected: Option<&FsWriteIntent>,
        signal: Option<AbortPredicate>,
        _sandbox_policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsWriteOutcome, FsError> {
        self.with_lock(target.target_key.as_str(), async {
            let existing = probe(target.target_key.as_str()).await?;
            if existing.as_ref().is_some_and(|info| info.kind != PathKind::File) {
                return Err(FsError::new(
                    format!("cannot write \"{}\": not a regular file", target.display_path),
                    FsErrorCode::FsNotRegularFile,
                ));
            }
            match expected {
                Some(FsWriteIntent::ReplaceIfVersion { version }) => {
                    let Some(existing) = &existing else {
                        return Err(FsError::new(
                            format!("cannot write \"{}\": file no longer exists", target.display_path),
                            FsErrorCode::FsStaleVersion,
                        ));
                    };
                    if &existing.version != version {
                        return Err(FsError::new(
                            format!("cannot write \"{}\": file changed since it was read", target.display_path),
                            FsErrorCode::FsStaleVersion,
                        ));
                    }
                }
                Some(FsWriteIntent::CreateIfAbsent) => {
                    if existing.is_some() {
                        return Err(FsError::new(
                            format!("cannot overwrite existing \"{}\" without reading it first", target.display_path),
                            FsErrorCode::FsNotObserved,
                        ));
                    }
                }
                None => {}
            }

            let diffable = existing.is_some()
                && (content.len() as u64) < self.config.diff_basis_max_bytes;
            let before = if diffable {
                read_text_for_diff(
                    target.target_key.as_str(),
                    self.config.diff_basis_max_bytes,
                    signal.as_ref(),
                )
                .await?
            } else {
                None
            };
            let create_guard = match expected {
                Some(FsWriteIntent::CreateIfAbsent) => Some(LocalTarget {
                    display_path: target.display_path.clone(),
                    target_key: target.target_key.clone(),
                }),
                _ => None,
            };
            write_file_atomic(
                target.target_key.as_str(),
                content,
                existing.as_ref().map(|info| info.mode),
                signal.as_ref(),
                &self.internals,
                create_guard.as_ref(),
            )
            .await?;
            let after = probe(target.target_key.as_str()).await?;
            Ok(FsWriteOutcome {
                operation: if existing.is_some() { FsWriteOperation::Update } else { FsWriteOperation::Create },
                version: self.version_after_write(after.as_ref().map(|info| info.version.clone()), target),
                before,
                // LF-normalized to share the diff basis with `before` (also
                // LF): a CRLF overwrite must not read as every line changed.
                after: normalize_line_endings(content),
            })
        })
        .await
    }

    async fn edit_text(
        &self,
        target: &FsTarget,
        edit: &FsEditRequest,
        expected: Option<&dsh_fs::FsEditGuard>,
        signal: Option<AbortPredicate>,
        _sandbox_policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsEditOutcome, FsError> {
        self.with_lock(target.target_key.as_str(), async {
            let existing = probe(target.target_key.as_str()).await?;
            let Some(existing) = existing else {
                return Err(FsError::new(
                    format!("cannot edit \"{}\": file changed since it was read", target.display_path),
                    FsErrorCode::FsStaleVersion,
                ));
            };
            if existing.kind != PathKind::File {
                return Err(FsError::new(
                    format!("cannot edit \"{}\": not a regular file", target.display_path),
                    FsErrorCode::FsNotRegularFile,
                ));
            }
            if let Some(guard) = expected {
                if existing.version != guard.version {
                    return Err(FsError::new(
                        format!("cannot edit \"{}\": file changed since it was read", target.display_path),
                        FsErrorCode::FsStaleVersion,
                    ));
                }
            }
            let (original, line_endings) =
                read_for_edit(target.target_key.as_str(), &target.display_path, signal.as_ref()).await?;
            let (edited, _replacements) = apply_literal_edit(
                &original,
                &edit.old_string,
                &edit.new_string,
                edit.replace_all,
                &target.display_path,
            )?;
            let content = restore_line_endings(&edited, line_endings);
            write_file_atomic(
                target.target_key.as_str(),
                &content,
                Some(existing.mode),
                signal.as_ref(),
                &self.internals,
                None,
            )
            .await?;
            let after = probe(target.target_key.as_str()).await?;
            Ok(FsEditOutcome {
                version: self.version_after_write(after.as_ref().map(|info| info.version.clone()), target),
                before: original,
                after: edited,
            })
        })
        .await
    }
}

use std::path::Path;

