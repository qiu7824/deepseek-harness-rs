//! Atomic file replacement and writer coordination. Rust
//! port of `@deepseek-ai/dsh-atomic-write`.
//!
//! `write_file_atomic` writes a random-suffix sibling with exclusive create
//! and the caller's permission bits, then renames it over the target, so
//! readers observe either the old or the new complete content.
//! `with_file_lock` serializes cross-process writers through an OS-owned
//! lock on a persistent `<file>.lock` sibling. Cancellation and process exit
//! release ownership without leaving a stale lock to block future writes.
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

#[cfg(windows)]
fn is_transient_windows_replace_error(error: &std::io::Error) -> bool {
    // Windows may deny rename-over-target while a reader temporarily omits
    // FILE_SHARE_DELETE. Preserve permanent failures by retrying only the
    // platform's access/sharing/lock denial codes and keeping a hard deadline.
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

async fn rename_over_target(temp: &Path, filename: &Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        return fs::rename(temp, filename).await;
    }

    #[cfg(windows)]
    {
        const RETRY_INITIAL_MS: u64 = 10;
        const RETRY_MAX_MS: u64 = 50;
        const RETRY_TIMEOUT_MS: u64 = 500;

        let deadline = Instant::now() + Duration::from_millis(RETRY_TIMEOUT_MS);
        let mut delay = RETRY_INITIAL_MS;
        loop {
            match fs::rename(temp, filename).await {
                Ok(()) => return Ok(()),
                Err(error)
                    if is_transient_windows_replace_error(&error) && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    delay = (delay * 2).min(RETRY_MAX_MS);
                }
                Err(error) => return Err(error),
            }
        }
    }
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
            fs::set_permissions(&temp, std::fs::Permissions::from_mode(options.mode)).await?;
        }
        #[cfg(not(unix))]
        {
            let _ = options.mode;
        }
        rename_over_target(&temp, filename).await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temp).await;
    }
    result
}

const LOCK_RETRY_INITIAL_MS: u64 = 20;
const LOCK_RETRY_MAX_MS: u64 = 200;
const LOCK_TIMEOUT_MS: u64 = 2_000;
const LOCK_PROTOCOL: &[u8] = b"dsh-os-lock-v1\n";

enum LockMarker {
    Native,
    Legacy { pid: u32, suffix_offset: u64 },
    Unknown,
}

fn lock_marker(bytes: &[u8]) -> LockMarker {
    if bytes == LOCK_PROTOCOL {
        return LockMarker::Native;
    }
    if bytes.len() > 128 {
        return LockMarker::Unknown;
    }
    let Some(end) = bytes.iter().position(|byte| *byte == b'\n') else {
        return LockMarker::Unknown;
    };
    // A complete legacy PID ends with a newline. Never guess a process ID
    // from a writer's partially written first line.
    if end == 0 || !bytes[..end].iter().all(u8::is_ascii_digit) {
        return LockMarker::Unknown;
    }
    let Some(pid) = std::str::from_utf8(&bytes[..end])
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid > 0)
    else {
        return LockMarker::Unknown;
    };
    let suffix = &bytes[end + 1..];
    if suffix == LOCK_PROTOCOL {
        LockMarker::Native
    } else if LOCK_PROTOCOL.starts_with(suffix) {
        LockMarker::Legacy {
            pid,
            suffix_offset: (end + 1) as u64,
        }
    } else {
        LockMarker::Unknown
    }
}

struct LockCandidate {
    file: std::fs::File,
    path: std::path::PathBuf,
}

impl Drop for LockCandidate {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn prepare_lock_candidate(lock_path: &Path) -> std::io::Result<LockCandidate> {
    use std::io::Write;
    loop {
        let path = lock_path.with_file_name(format!(
            "{}.{}.tmp",
            lock_path.file_name().unwrap_or_default().to_string_lossy(),
            temp_suffix()
        ));
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let mut candidate = LockCandidate { file, path };
        candidate.file.write_all(LOCK_PROTOCOL)?;
        candidate.file.sync_data()?;
        return Ok(candidate);
    }
}

fn open_lock_file(lock_path: &Path) -> std::io::Result<std::fs::File> {
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
    {
        Ok(file) => return Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    // Publish only a complete synced marker. A crash before publication can
    // leave an unused candidate, never an ownerless empty lock at the target.
    let candidate = prepare_lock_candidate(lock_path)?;
    match std::fs::hard_link(&candidate.path, lock_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    // A concurrent creator may have won; always open the published inode.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
}

#[cfg(windows)]
fn legacy_owner_alive(pid: u32) -> std::io::Result<bool> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // Only a positively absent process permits migration. Access denied and
    // other failures leave ownership uncertain and therefore fail closed.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                Ok(false)
            } else {
                Err(error)
            };
        }
        let mut code = 0;
        let result = GetExitCodeProcess(handle, &mut code);
        let error = (result == 0).then(std::io::Error::last_os_error);
        CloseHandle(handle);
        match error {
            Some(error) => Err(error),
            None => Ok(code == 259), // STILL_ACTIVE; an ambiguous exit code stays blocked.
        }
    }
}

#[cfg(unix)]
fn legacy_owner_alive(pid: u32) -> std::io::Result<bool> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "legacy lock PID is outside the platform process range",
        )
    })?;
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(not(any(unix, windows)))]
fn legacy_owner_alive(_: u32) -> std::io::Result<bool> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "legacy process ownership cannot be checked on this platform",
    ))
}

fn lock_recovery_error(path: &Path, detail: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "atomic-write: cannot safely recover legacy writer lock at {}: {detail}. Stop all application instances, verify that no writer is running, then move this lock file aside and retry.",
            path.display()
        ),
    )
}

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
    // Never unlink the sidecar: another waiter may already hold an open
    // handle to the same file, and replacing its inode would split the lock.
    let deadline = Instant::now() + Duration::from_millis(LOCK_TIMEOUT_MS);
    let mut delay = LOCK_RETRY_INITIAL_MS;
    let file = loop {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut file = match open_lock_file(&lock_path) {
            Ok(file) => file,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let mut ambiguous = false;
        match file.try_lock() {
            Ok(()) => {
                let mut marker = Vec::new();
                Read::take(&mut file, 129).read_to_end(&mut marker)?;
                match lock_marker(&marker) {
                    LockMarker::Native => break file,
                    LockMarker::Legacy { pid, suffix_offset }
                        if !legacy_owner_alive(pid)
                            .map_err(|error| lock_recovery_error(&lock_path, error))? =>
                    {
                        // Keep the only recoverable owner record intact even
                        // if the process dies partway through this upgrade.
                        file.seek(SeekFrom::Start(suffix_offset))?;
                        file.write_all(LOCK_PROTOCOL)?;
                        file.sync_data()?;
                        break file;
                    }
                    LockMarker::Legacy { .. } => {}
                    LockMarker::Unknown => ambiguous = true,
                }
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        // A live legacy writer can remove its sidecar after finishing. Drop
        // this handle and reopen the current path instead of locking an
        // unlinked inode on the next attempt.
        drop(file);
        if Instant::now() >= deadline {
            if ambiguous {
                return Err(lock_recovery_error(
                    &lock_path,
                    "unrecognized or empty owner record",
                ));
            }
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
    };
    let result = operation.await;
    // Holding the handle across the await also covers unwinding/cancellation.
    drop(file);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("dsh-file-lock-{}", temp_suffix()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        (root, path)
    }

    #[tokio::test]
    async fn legacy_sidecar_does_not_block_writes_or_get_deleted() {
        let (root, path) = fixture();
        let sidecar = root.join("settings.json.lock");
        fs::write(&sidecar, "2147483647\n").await.unwrap();
        with_file_lock(&path, async { fs::write(&path, "new settings").await })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fs::read_to_string(&path).await.unwrap(), "new settings");
        assert_eq!(
            fs::read(&sidecar).await.unwrap(),
            [b"2147483647\n".as_slice(), LOCK_PROTOCOL].concat()
        );
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn same_process_writers_are_mutually_exclusive() {
        let (root, path) = fixture();
        let entered = Arc::new(AtomicU64::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let path = path.clone();
            let entered = entered.clone();
            tasks.push(tokio::spawn(async move {
                with_file_lock(&path, async {
                    assert_eq!(entered.fetch_add(1, Ordering::SeqCst), 0);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    assert_eq!(entered.fetch_sub(1, Ordering::SeqCst), 1);
                })
                .await
                .unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_and_panicked_operations_release_ownership() {
        let (root, path) = fixture();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let task_path = path.clone();
        let task = tokio::spawn(async move {
            with_file_lock(&task_path, async {
                entered_tx.send(()).unwrap();
                std::future::pending::<()>().await;
            })
            .await
            .unwrap();
        });
        entered_rx.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        with_file_lock(&path, async {}).await.unwrap();
        let task_path = path.clone();
        let task = tokio::spawn(async move {
            with_file_lock(&task_path, async { panic!("writer failed") })
                .await
                .unwrap();
        });
        assert!(task.await.unwrap_err().is_panic());
        with_file_lock(&path, async {}).await.unwrap();
        fs::remove_dir_all(root).await.unwrap();
    }

    #[test]
    fn process_lock_holder() {
        let Some(path) = std::env::var_os("DSH_ATOMIC_WRITE_LOCK_TEST") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            if std::env::var_os("DSH_ATOMIC_WRITE_BEFORE_LINK_TEST").is_some() {
                let candidate =
                    prepare_lock_candidate(&path.with_file_name("settings.json.lock")).unwrap();
                fs::write(path.with_extension("ready"), "ready")
                    .await
                    .unwrap();
                std::future::pending::<()>().await;
                drop(candidate);
            }
            if std::env::var_os("DSH_ATOMIC_WRITE_LEGACY_LOCK_TEST").is_some() {
                let sidecar = path.with_file_name("settings.json.lock");
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(sidecar)
                    .await
                    .unwrap();
                file.write_all(format!("{}\n", std::process::id()).as_bytes())
                    .await
                    .unwrap();
                drop(file);
                fs::write(path.with_extension("ready"), "ready")
                    .await
                    .unwrap();
                std::future::pending::<()>().await;
            }
            with_file_lock(&path, async {
                fs::write(path.with_extension("ready"), "ready")
                    .await
                    .unwrap();
                std::future::pending::<()>().await;
            })
            .await
            .unwrap();
        });
    }

    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[tokio::test]
    async fn active_process_lock_is_respected_and_process_exit_releases_it() {
        assert_process_lock(false).await;
    }

    #[tokio::test]
    async fn live_legacy_process_is_not_overridden_and_dead_owner_is_migrated() {
        assert_process_lock(true).await;
    }

    async fn assert_process_lock(legacy: bool) {
        let (root, path) = fixture();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "tests::process_lock_holder", "--nocapture"])
            .env("DSH_ATOMIC_WRITE_LOCK_TEST", &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if legacy {
            command.env("DSH_ATOMIC_WRITE_LEGACY_LOCK_TEST", "1");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = ChildGuard(command.spawn().unwrap());
        let child_pid = child.0.id();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !path.with_extension("ready").exists() {
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "lock holder exited early"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let blocked = with_file_lock(&path, async { panic!("entered another process's lock") })
            .await
            .unwrap_err();
        assert_eq!(blocked.kind(), std::io::ErrorKind::TimedOut);
        drop(child);
        with_file_lock(&path, async {}).await.unwrap();
        let bytes = fs::read(root.join("settings.json.lock")).await.unwrap();
        assert!(bytes.ends_with(LOCK_PROTOCOL));
        if legacy {
            assert!(bytes.starts_with(format!("{child_pid}\n").as_bytes()));
        }
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn crash_before_lock_publication_does_not_leave_a_blocking_sidecar() {
        let (root, path) = fixture();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "tests::process_lock_holder", "--nocapture"])
            .env("DSH_ATOMIC_WRITE_LOCK_TEST", &path)
            .env("DSH_ATOMIC_WRITE_BEFORE_LINK_TEST", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = ChildGuard(command.spawn().unwrap());
        tokio::time::timeout(Duration::from_secs(10), async {
            while !path.with_extension("ready").exists() {
                assert!(child.0.try_wait().unwrap().is_none());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!root.join("settings.json.lock").exists());
        drop(child);
        with_file_lock(&path, async {}).await.unwrap();
        assert_eq!(
            fs::read(root.join("settings.json.lock")).await.unwrap(),
            LOCK_PROTOCOL
        );
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn interrupted_legacy_upgrade_retains_pid_and_recovers_only_dead_owners() {
        let (root, path) = fixture();
        let sidecar = root.join("settings.json.lock");
        for length in [1, 5, LOCK_PROTOCOL.len() - 1] {
            fs::write(
                &sidecar,
                [b"2147483647\n".as_slice(), &LOCK_PROTOCOL[..length]].concat(),
            )
            .await
            .unwrap();
            with_file_lock(&path, async {}).await.unwrap();
            assert_eq!(
                fs::read(&sidecar).await.unwrap(),
                [b"2147483647\n".as_slice(), LOCK_PROTOCOL].concat()
            );
        }
        let live = [
            format!("{}\n", std::process::id()).as_bytes(),
            &LOCK_PROTOCOL[..5],
        ]
        .concat();
        fs::write(&sidecar, &live).await.unwrap();
        let error = with_file_lock(&path, async {
            panic!("live legacy ownership must be preserved")
        })
        .await
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(fs::read(&sidecar).await.unwrap(), live);
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn ambiguous_legacy_sidecar_is_preserved_with_recovery_guidance() {
        let (root, path) = fixture();
        let sidecar = root.join("settings.json.lock");
        fs::write(&sidecar, "").await.unwrap();
        let error = with_file_lock(&path, async {
            panic!("unknown ownership must not be overridden")
        })
        .await
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("Stop all application instances"));
        assert_eq!(fs::read(&sidecar).await.unwrap(), b"");
        fs::remove_dir_all(root).await.unwrap();
    }
}
