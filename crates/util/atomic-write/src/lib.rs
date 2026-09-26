//! Atomic file replacement and writer coordination. Rust port of
//! `@deepseek-ai/dsh-atomic-write`.
//!
//! `write_file_atomic` writes a random-suffix sibling with exclusive create
//! and the caller's permission bits, then renames it over the target, so
//! readers observe either the old or the new complete content.
//! `with_file_lock` serializes cross-process writers through a
//! create-exclusive `<file>.lock` sibling. The lock records its holder's PID
//! and hostname; a lock whose holder ran on this host and no longer exists is
//! taken over, so a crash while holding it cannot block later writers.
//!
//! # Deviations
//!
//! - Permission bits apply on Unix only (Rust std cannot chmod on Windows
//!   without extra crates); the `mode`/`dir_mode` arguments are validated and
//!   otherwise no-ops on Windows.

use std::path::{Path, PathBuf};
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

fn is_lock_contention(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        return true;
    }
    #[cfg(windows)]
    {
        // Create-new races around another writer's lock release can surface
        // as access/sharing/lock denial rather than AlreadyExists on Windows.
        // The existing deadline keeps permanent permission failures loud.
        matches!(error.raw_os_error(), Some(5 | 32 | 33))
    }
    #[cfg(not(windows))]
    false
}

const LOCK_RETRY_INITIAL_MS: u64 = 20;
const LOCK_RETRY_MAX_MS: u64 = 200;
const LOCK_TIMEOUT_MS: u64 = 2_000;

/// The process a lock record names. A record without a hostname predates
/// hostnames and was written on this host.
#[derive(Debug, PartialEq)]
struct LockHolder {
    pid: u32,
    hostname: Option<String>,
    nonce: Option<String>,
}

fn local_hostname() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer outlives the call and its length is passed.
        if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0 {
            let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
            return String::from_utf8_lossy(&buffer[..end]).into_owned();
        }
        String::new()
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").unwrap_or_default()
    }
}

/// The record a new lock carries: holder PID, its host, and a nonce that
/// keeps each record unique.
fn lock_record() -> String {
    let nonce = format!("{:016x}", {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or_default();
        time ^ (u64::from(std::process::id()) << 32)
            ^ TEMP_COUNTER.fetch_add(1, Ordering::Relaxed).rotate_left(17)
    });
    format!(
        "{}\n",
        serde_json::json!({"pid": std::process::id(), "hostname": local_hostname(), "nonce": nonce})
    )
}

/// The holder a lock record names, or None for a record this protocol did
/// not write completely.
fn parse_lock_holder(record: &str) -> Option<LockHolder> {
    // Earlier releases recorded only the PID.
    if let Some(pid) = record.strip_suffix('\n')
        && !pid.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
    {
        return Some(LockHolder {
            pid: pid.parse().ok()?,
            hostname: None,
            nonce: None,
        });
    }
    // An unparsable record is being written or was cut short; neither proves
    // that its holder stopped.
    let value: serde_json::Value = serde_json::from_str(record).ok()?;
    Some(LockHolder {
        pid: u32::try_from(value.get("pid")?.as_u64()?).ok()?,
        hostname: Some(value.get("hostname")?.as_str()?.to_string()),
        nonce: value
            .get("nonce")
            .and_then(|nonce| nonce.as_str())
            .map(str::to_string),
    })
}

/// Whether no process with this PID exists. Errors that do not prove absence
/// (e.g. access denied for another user's process) read as alive.
fn process_exited(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only probes for existence and permission.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return false;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: plain handle query; the handle is closed below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
        }
        let mut code = 0u32;
        let queried = unsafe { GetExitCodeProcess(handle, &mut code) } != 0;
        unsafe { CloseHandle(handle) };
        queried && code != STILL_ACTIVE
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// Whether the holder is proven gone: it ran on this host and its process
/// no longer exists.
fn holder_exited(holder: &LockHolder) -> bool {
    if holder
        .hostname
        .as_deref()
        .is_some_and(|host| host != local_hostname())
    {
        return false;
    }
    process_exited(holder.pid)
}

/// Remove the lock when its recorded holder exited. Contenders that read the
/// same record serialize on a claim file named after it, and the claimant
/// removes the lock only while it still holds that record, so a removal never
/// deletes a lock another contender acquired after the dead holder's.
async fn take_over_exited_lock(lock_path: &Path) -> std::io::Result<bool> {
    let Ok(record) = fs::read_to_string(lock_path).await else {
        // A lock that vanished, or that Windows is still deleting, proves
        // nothing about a holder.
        return Ok(false);
    };
    let Some(holder) = parse_lock_holder(&record) else {
        return Ok(false);
    };
    if !holder_exited(&holder) {
        return Ok(false);
    }
    let tag: String = holder
        .nonce
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(32)
        .collect();
    let claim = sibling(lock_path, &format!(".takeover-{}-{tag}", holder.pid));
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&claim)
        .await
    {
        Ok(mut file) => {
            let _ = file
                .write_all(format!("{}\n", std::process::id()).as_bytes())
                .await;
        }
        // Another contender owns the claim for this record.
        Err(error) if is_lock_contention(&error) => return Ok(false),
        Err(error) => return Err(error),
    }
    let removed = async {
        if fs::read_to_string(lock_path).await.ok().as_deref() != Some(record.as_str()) {
            return false;
        }
        fs::remove_file(lock_path).await.is_ok()
    }
    .await;
    // A claim left behind names a record that is no longer the lock, so it
    // blocks no later takeover.
    let _ = fs::remove_file(&claim).await;
    Ok(removed)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    path.with_file_name(format!(
        "{}{suffix}",
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
    ))
}

/// Removes the lock file when the holding operation finishes or is dropped,
/// so a cancelled writer never leaves its lock behind.
struct LockRelease(PathBuf);

impl Drop for LockRelease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Hold the cross-process writer lock for `filename` around one operation
/// (TS `withFileLock`). A lock whose recorded holder ran on this host and no
/// longer exists is removed and acquisition retries at once; any other lock,
/// including one with an incomplete record or another host's record, is
/// waited for until the deadline. A holder whose PID a live process reused
/// keeps its lock until an operator removes it.
pub async fn with_file_lock<T>(
    filename: &Path,
    operation: impl std::future::Future<Output = T>,
) -> Result<T, std::io::Error> {
    let lock_path = sibling(filename, ".lock");
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
                let _ = file.write_all(lock_record().as_bytes()).await;
                drop(file);
                break;
            }
            Err(error) if is_lock_contention(&error) => {
                if take_over_exited_lock(&lock_path).await? {
                    continue;
                }
            }
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
    let release = LockRelease(lock_path);
    let result = operation.await;
    drop(release);
    Ok(result)
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-atomic-write-{name}-{}-{}",
            std::process::id(),
            temp_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn exited_pid() -> u32 {
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", "exit"])
                .spawn()
                .unwrap()
        } else {
            std::process::Command::new("true").spawn().unwrap()
        };
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn records_parse_in_current_and_legacy_shapes() {
        let current = lock_record();
        let holder = parse_lock_holder(&current).unwrap();
        assert_eq!(holder.pid, std::process::id());
        assert_eq!(holder.hostname.as_deref(), Some(local_hostname().as_str()));
        assert!(holder.nonce.is_some());
        assert_eq!(
            parse_lock_holder("4242\n"),
            Some(LockHolder {
                pid: 4242,
                hostname: None,
                nonce: None
            })
        );
        assert_eq!(parse_lock_holder(""), None);
        assert_eq!(parse_lock_holder("{\"pid\":12"), None);
        assert_eq!(parse_lock_holder("{\"pid\":12}"), None);
    }

    #[tokio::test]
    async fn a_lock_left_by_an_exited_process_is_taken_over() {
        let dir = temp_dir("exited");
        let target = dir.join("settings.json");
        for record in [
            format!("{}\n", exited_pid()),
            format!(
                "{}\n",
                serde_json::json!({"pid": exited_pid(), "hostname": local_hostname(), "nonce": "abc"})
            ),
        ] {
            std::fs::write(dir.join("settings.json.lock"), &record).unwrap();
            let started = Instant::now();
            let value = with_file_lock(&target, async { 7 }).await.unwrap();
            assert_eq!(value, 7);
            assert!(started.elapsed() < Duration::from_millis(LOCK_TIMEOUT_MS));
            assert!(!dir.join("settings.json.lock").exists());
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_live_or_foreign_holder_keeps_its_lock() {
        let dir = temp_dir("live");
        let target = dir.join("settings.json");
        let lock = dir.join("settings.json.lock");
        for record in [
            lock_record(),
            format!(
                "{}\n",
                serde_json::json!({"pid": exited_pid(), "hostname": "another-host", "nonce": "n"})
            ),
            "{\"pid\":".to_string(),
        ] {
            std::fs::write(&lock, &record).unwrap();
            let error = with_file_lock(&target, async {}).await.unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert_eq!(std::fs::read_to_string(&lock).unwrap(), record);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_cancelled_operation_releases_the_lock() {
        let dir = temp_dir("cancel");
        let target = dir.join("settings.json");
        let pending = with_file_lock(&target, std::future::pending::<()>());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), pending)
                .await
                .is_err()
        );
        assert!(!dir.join("settings.json.lock").exists());
        with_file_lock(&target, async {}).await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
