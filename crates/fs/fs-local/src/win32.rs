//! Windows security-descriptor helpers for atomic local-file replacement.
//! Rust port of `packages/fs/fs-local/src/win32.ts`.
//!
//! # Deviations
//!
//! - Replacement uses `ReplaceFileW` to preserve the original file on failure
//!   and inherit its security descriptor at publication.

use std::path::Path;

/// Replacement merges the original security descriptor at publication.
/// The temporary file retains its private staging directory permissions.
pub async fn copy_file_dacl_win32(source: &Path, destination: &Path) -> Result<(), String> {
    let _ = (source, destination);
    Ok(())
}

/// Replace existing content without deleting the original before publication.
pub async fn replace_file_win32(replaced: &Path, replacement: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn ReplaceFileW(
                replaced: *const u16,
                replacement: *const u16,
                backup: *const u16,
                flags: u32,
                exclude: *mut std::ffi::c_void,
                reserved: *mut std::ffi::c_void,
            ) -> i32;
        }
        let replaced: Vec<u16> = replaced.as_os_str().encode_wide().chain(Some(0)).collect();
        let replacement: Vec<u16> = replacement
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let ok = unsafe {
            ReplaceFileW(
                replaced.as_ptr(),
                replacement.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(replacement, replaced).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn failed_replacement_does_not_delete_the_existing_file() {
        let root = std::env::temp_dir().join(format!("safe-replace-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let original = root.join("original.docx");
        std::fs::write(&original, b"keep this file").unwrap();
        assert!(
            replace_file_win32(&original, &root.join("missing.docx"))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&original).unwrap(), b"keep this file");
        std::fs::remove_dir_all(root).unwrap();
    }
}
