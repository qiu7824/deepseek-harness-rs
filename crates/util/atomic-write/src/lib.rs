//! Zero-dependency atomic file replacement and writer coordination. Rust
//! port of `@deepseek-ai/dsh-atomic-write`.
//!
//! `write_file_atomic` writes a random-suffix sibling with exclusive create
//! and the caller's permission bits, then renames it over the target, so
//! readers observe either the old or the new complete content.
//! `with_file_lock` serializes cross-process writers through a
//! create-exclusive `<file>.lock` sibling.
//!
//! # Deviations
//!
//! - Permission bits apply on Unix only (Rust std cannot chmod on Windows
//!   without extra crates); the `mode`/`dir_mode` arguments are validated and
//!   otherwise no-ops on Windows.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

/// Filesystem options for [`write_file_atomic`].
#[derive(Debug, Clone, Copy)]
pub struct WriteFileAtomicOptions {
    /// Permission bits stamped on the fresh temp inode.
    pub mode: u32,
    /// Permission bits for parent directories this call creates.
    pub dir_mode: Option<u32>,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_suffix() -> String {
    let pid = std::process::id();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{pid:x}{counter:08x}")
}

/// Replace `filename` with `content` in one atomic step, creating parent
/// directories (TS `writeFileAtomic`).
pub async fn write_file_atomic(
    filename: &Path,
    content: &[u8],
    options: WriteFileAtomicOptions,
) -> std::io::Result<()> {
    let parent = filename.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent).await?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        if let Some(mode) = options.dir_mode {
            builder.mode(mode);
        }
        builder.create(parent).await?;
    }
    #[cfg(not(unix))]
    {
        let _ = options.dir_mode;
        fs::create_dir_all(parent).await?;
    }

    let temp = filename.with_file_name(format!(
        "{}.{}.tmp",
        filename
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        temp_suffix()
    ));
    let result = async {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .await?;
        file.write_all(content).await?;
        file.flush().await?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(options.mode)).await?;
        }
        #[cfg(not(unix))]
        {
            let _ = options.mode;
        }
        fs::rename(&temp, filename).await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temp).await;
    }
    result
}

fn is_already_exists(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::AlreadyExists
}

const LOCK_RETRY_INITIAL_MS: u64 = 20;
const LOCK_RETRY_MAX_MS: u64 = 200;
const LOCK_TIMEOUT_MS: u64 = 2_000;

/// Hold the cross-process writer lock for `filename` around one operation
/// (TS `withFileLock`).
pub async fn with_file_lock<T>(
    filename: &Path,
    operation: impl std::future::Future<Output = T>,
) -> Result<T, std::io::Error> {
    let lock_path = filename.with_file_name(format!(
        "{}.lock",
        filename
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    let deadline = Instant::now() + Duration::from_millis(LOCK_TIMEOUT_MS);
    let mut delay = LOCK_RETRY_INITIAL_MS;
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .await
        {
            Ok(mut file) => {
                let _ = file
                    .write_all(format!("{}\n", std::process::id()).as_bytes())
                    .await;
                drop(file);
                break;
            }
            Err(error) if is_already_exists(&error) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "atomic-write: timed out waiting for the writer lock at {}",
                    lock_path.display()
                ),
            ));
        }
        tokio::time::sleep(Duration::from_millis(delay)).await;
        delay = (delay * 2).min(LOCK_RETRY_MAX_MS);
    }
    let result = operation.await;
    let _ = fs::remove_file(&lock_path).await;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dsh-atomic-write-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn writes_atomically_and_replaces() {
        let dir = temp_dir("replace");
        let target = dir.join("file.txt");
        std::fs::write(&target, "old").unwrap();
        write_file_atomic(&target, b"new content", WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: None,
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new content");
        // no temp litter
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn creates_parent_directories() {
        let dir = temp_dir("parents");
        let target = dir.join("a/b/file.txt");
        write_file_atomic(&target, b"x", WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: None,
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "x");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn file_lock_serializes() {
        let dir = temp_dir("lock");
        let target = dir.join("data.txt");
        std::fs::write(&target, "0").unwrap();
        let operations = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let target1 = target.clone();
        let ops1 = operations.clone();
        let handle1 = tokio::spawn(async move {
            with_file_lock(&target1, async move {
                ops1.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(30)).await;
                ops1.fetch_add(1, Ordering::SeqCst);
            })
            .await
        });
        let target2 = target.clone();
        let ops2 = operations.clone();
        let handle2 = tokio::spawn(async move {
            with_file_lock(&target2, async move {
                ops2.fetch_add(1, Ordering::SeqCst);
            })
            .await
        });
        let (r1, r2) = tokio::join!(handle1, handle2);
        r1.unwrap().unwrap();
        r2.unwrap().unwrap();
        assert_eq!(operations.load(Ordering::SeqCst), 3);
        assert!(!dir.join("data.txt.lock").exists(), "lock released");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
