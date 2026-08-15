//! Windows security-descriptor helpers for atomic local-file replacement.
//! Rust port of `packages/fs/fs-local/src/win32.ts`.
//!
//! # Deviations
//!
//! - The TS loads Win32 FFI (`GetFileSecurityW`/`SetFileSecurityW`/
//!   `ReplaceFileW`) through koffi. The Rust port simplifies both
//!   boundaries until the sandbox-windows-acl milestone: `copy_file_dacl`
//!   is a no-op (a new temp file inherits the staging directory's DACL,
//!   which is the same behavior for the CREATE path; the protected-DACL
//!   REPLACE path is the deferred part), and `replace_file` removes the
//!   target before renaming (content-atomic, ACL inheritance from the
//!   parent directory). Both seam injections in `fsio` remain so tests pin
//!   the same choreography.

use std::path::Path;

/// Copy an existing file's DACL onto another file (simplified no-op; see the
/// module deviations).
pub async fn copy_file_dacl_win32(source: &Path, destination: &Path) -> Result<(), String> {
    let _ = (source, destination);
    Ok(())
}

/// Replace a Windows file while preserving the replaced file's ACL (the
/// simplified remove-then-rename; see the module deviations).
pub async fn replace_file_win32(replaced: &Path, replacement: &Path) -> Result<(), String> {
    // The target may have vanished during staging; treat absence as a plain
    // rename (mirrors the TS ENOENT fallback).
    if replaced.exists() {
        std::fs::remove_file(replaced).map_err(|error| error.to_string())?;
    }
    std::fs::rename(replacement, replaced).map_err(|error| error.to_string())
}
