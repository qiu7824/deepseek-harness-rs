//! Atomic, no-overwrite publication of a desktop readiness file.
//! The native boundary stays std-only so the production implementation can be
//! compiled and exercised directly with rustc on Windows.

use std::fs;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(not(windows))]
pub(crate) fn publish_no_replace(temp: &Path, path: &Path) -> std::io::Result<()> {
    fs::hard_link(temp, path)
}

#[cfg(windows)]
fn windows_file_api_path(path: &Path) -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "readiness path has no file name",
        )
    })?;
    if name.encode_wide().any(|unit| unit == 0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "readiness file name contains NUL",
        ));
    }
    // A readiness leaf is one file, never an ADS. In particular, joining
    // "a:stream" would treat it as a drive prefix and discard the parent.
    if name.encode_wide().any(|unit| unit == b':' as u16) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "readiness file name cannot select an alternate data stream",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "readiness path has no parent",
        )
    })?;
    // Windows canonicalize supplies the Win32 verbatim disk/UNC spelling.
    // Only the parent exists for a fresh destination; keep the leaf as OsStr
    // so Unicode and unpaired UTF-16 units survive without a lossy conversion.
    let parent = fs::canonicalize(parent)?;
    if !parent.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "readiness parent is not a directory",
        ));
    }
    Ok(parent.join(name))
}

#[cfg(windows)]
pub(crate) fn publish_no_replace(temp: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    // Unlike std::fs::rename on Windows, flags zero does not replace a target.
    // A same-volume move also supports FAT/exFAT, where hard links do not exist.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, target: *const u16, flags: u32) -> i32;
    }
    // Rust's file APIs already convert long paths, but this direct Win32 call
    // needs the same spelling even when the executable has no longPathAware
    // manifest. Canonicalize each existing parent, never the absent target.
    let from = windows_file_api_path(temp)?;
    let to = windows_file_api_path(path)?;
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // Match the 51 UTF-16 units in the production UUID sibling name.
    const TEMP_LEAF: &str = ".dsh-ready-00000000-0000-0000-0000-000000000000.tmp";
    const REPORT: &str =
        r#"{"version":1,"pid":42,"url":"http://127.0.0.1:41321","note":"中文 空格"}"#;

    struct FixtureDirectory(PathBuf);

    impl FixtureDirectory {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "dsh-ready-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for FixtureDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn publish_fixture(path: &Path, report: &[u8]) -> std::io::Result<()> {
        let temp = path.parent().unwrap().join(TEMP_LEAF);
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(report)?;
            file.sync_all()?;
            drop(file);
            publish_no_replace(&temp, path)
        })();
        // Match the JSON publisher's cleanup on either success or failure.
        let _ = fs::remove_file(&temp);
        result
    }

    #[test]
    fn readiness_publication_is_complete_and_never_overwrites_existing_file() {
        let root = FixtureDirectory::new();
        let path = root.0.join("中文 ready file.json");
        let report = REPORT.as_bytes();
        publish_fixture(&path, report).unwrap();
        assert_eq!(fs::read(&path).unwrap(), report);
        assert_eq!(
            publish_fixture(&path, br#"{"pid":7}"#).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&path).unwrap(), report);
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
    }

    #[cfg(windows)]
    fn assert_windows_long_path_publication(parent: &Path) {
        use std::os::windows::ffi::OsStrExt;
        use std::path::{Component, Prefix};
        // A verbatim TEMP input would bypass the original MAX_PATH failure.
        assert!(matches!(
            parent.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::UNC(_, _))
        ));
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(existing: *const u16, target: *const u16, flags: u32) -> i32;
        }
        fs::create_dir_all(parent).unwrap();
        let temp = parent.join(TEMP_LEAF);
        let path = parent.join("ready.json");
        let report = REPORT.as_bytes();
        fs::write(&temp, report).unwrap();
        let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        assert!(from.len() > 260);
        // Reproduce the old publication call using real files, in the test
        // executable built without longPathAware opt-in. This must fail before
        // exercising the fixed path; do not count a skipped fixture as proof.
        let moved = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) };
        let error = std::io::Error::last_os_error();
        assert_eq!(
            moved, 0,
            "unprefixed control succeeded; run this fixture without longPathAware opt-in"
        );
        eprintln!(
            "old readiness MoveFileExW failed as expected: {error}; source UTF-16 length={}, target UTF-16 length={}",
            from.len(),
            to.len()
        );
        assert!(temp.is_file());
        assert!(!path.exists());
        fs::remove_file(&temp).unwrap();

        publish_fixture(&path, report).unwrap();
        assert_eq!(fs::read(&path).unwrap(), report);
        let before = fs::read(&path).unwrap();
        assert_eq!(
            publish_fixture(&path, br#"{"pid":7}"#).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(parent).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn windows_readiness_publication_supports_long_unicode_directories() {
        use std::os::windows::ffi::OsStrExt;
        let root = FixtureDirectory::new();
        let mut parent = root.0.join("中文 有空格");
        while parent.as_os_str().encode_wide().count() <= 300 {
            parent = parent.join("中文 空格 readiness directory");
        }
        assert!(parent.as_os_str().encode_wide().count() > 260);
        assert_windows_long_path_publication(&parent);
    }

    #[cfg(windows)]
    #[test]
    fn windows_readiness_handles_a_long_temp_sibling_of_a_short_target() {
        use std::os::windows::ffi::OsStrExt;
        let root = FixtureDirectory::new();
        let prefix = root.0.join("中文 有空格");
        let prefix_length = prefix.as_os_str().encode_wide().count();
        assert!(
            prefix_length < 214,
            "fixture needs a shorter system TEMP path"
        );
        let parent = prefix.join("段".repeat(215 - prefix_length - 1));
        assert_eq!(parent.as_os_str().encode_wide().count(), 215);
        assert!(parent.join("ready.json").as_os_str().encode_wide().count() < 260);
        assert_windows_long_path_publication(&parent);
    }

    #[cfg(windows)]
    #[test]
    fn windows_readiness_file_paths_preserve_unpaired_utf16_and_reject_invalid_leaves() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        let root = FixtureDirectory::new();
        assert_eq!(
            windows_file_api_path(Path::new(r"C:\")).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        let nul = root
            .0
            .join(OsString::from_wide(&[b'r' as u16, 0, b'x' as u16]));
        assert_eq!(
            windows_file_api_path(&nul).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        let stream = root.0.join("ready.json:stream");
        assert_eq!(
            windows_file_api_path(&stream).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        let mut drive_leaf = root.0.as_os_str().to_os_string();
        drive_leaf.push(r"\a:stream");
        assert_eq!(
            windows_file_api_path(&PathBuf::from(drive_leaf))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        let file_parent = root.0.join("parent-file");
        fs::write(&file_parent, b"file").unwrap();
        assert_eq!(
            windows_file_api_path(&file_parent.join("ready.json"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotADirectory
        );

        let temp = root.0.join(OsString::from_wide(&[
            0x4e2d,
            0x6587,
            0x20,
            b't' as u16,
            0xd800,
            b'm' as u16,
        ]));
        let path = root.0.join(OsString::from_wide(&[
            0x4e2d,
            0x6587,
            0x20,
            b'r' as u16,
            0xdfff,
            b'y' as u16,
        ]));
        fs::write(&temp, b"complete report").unwrap();
        publish_no_replace(&temp, &path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"complete report");
        assert!(!temp.exists());
        fs::write(&temp, b"replacement").unwrap();
        assert_eq!(
            publish_no_replace(&temp, &path).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&path).unwrap(), b"complete report");
        assert_eq!(fs::read(&temp).unwrap(), b"replacement");
    }
}
