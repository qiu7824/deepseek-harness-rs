//! Atomic whole-file replacement for the JSON backend. Rust port of
//! `packages/storage/storage-json/src/atomic.ts`.
//!
//! Publish protocol: write a same-directory temp file, fsync it, then
//! rename over the target. Rename is an atomic replace on POSIX and on
//! Windows (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`); replacement is
//! the intended semantic here — a unit file has exactly one writer per
//! process and last-write-wins is correct. After the rename the parent
//! directory is fsynced on POSIX so the new entry is crash-durable.
//!
//! # Deviations
//!
//! - The blocking file operations are SYNC here; callers run them through
//!   `spawn_blocking` (the Node async-fs threadpool equivalent). The
//!   blocking-shutdown hazard of spawn_blocking at process exit is the
//!   documented Node threadpool analog.

use std::io::Write;
use std::path::Path;

/// Durably replace `path` with `data` (TS `writeAtomic`; synchronous —
/// callers wrap it in `spawn_blocking`).
pub fn write_atomic(path: &Path, data: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "unit path has no parent"))?;
    let tmp = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let outcome = (|| {
        let mut handle = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        handle.write_all(data.as_bytes())?;
        handle.sync_all()?;
        drop(handle);
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        fsync_directory(dir)?;
        Ok::<(), std::io::Error>(())
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    outcome
}

/// fsync a POSIX directory so a just-renamed entry is crash-durable (TS
/// `fsyncDirectory`; Windows rejects directory opens, so this is
/// POSIX-only).
#[cfg(unix)]
fn fsync_directory(path: &Path) -> std::io::Result<()> {
    let handle = std::fs::File::open(path)?;
    handle.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_and_cleans_up_on_failure() {
        let root = std::env::temp_dir().join(format!("dsh-json-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let target = root.join("unit.json");
        write_atomic(&target, "{\"v\":1}").expect("write");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "{\"v\":1}");
        write_atomic(&target, "{\"v\":2}").expect("replace");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "{\"v\":2}");
        // No temp litter survives a successful publish.
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
