//! Persistent stdio for a desktop's original Host process. This module is
//! deliberately std-only so its platform boundary can be tested on its own.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub fn validate_log_path(path: &Path, has_ready_file: bool) -> io::Result<()> {
    if !has_ready_file {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--stdio-log requires --ready-file",
        ));
    }
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--stdio-log needs an absolute file path",
        ));
    }
    if !path.parent().is_some_and(Path::is_dir) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "--stdio-log parent directory must already exist",
        ));
    }
    Ok(())
}

/// Keep the returned File alive for the entire Host process lifetime. Redirect
/// the process's own handles; never introduce a wrapper or change its PID.
pub fn redirect(path: &Path) -> io::Result<File> {
    validate_log_path(path, true)?;
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    io::stdout().flush()?;
    io::stderr().flush()?;
    redirect_handles(&file)?;
    // One bounded marker after redirection actually succeeds, including when
    // ordinary Host boot is silent. No user content or configuration values.
    file.write_all(b"dsh: desktop Host starting\n")?;
    Ok(file)
}

#[cfg(unix)]
fn redirect_handles(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn dup2(old: i32, new: i32) -> i32;
    }
    for destination in [1, 2] {
        if unsafe { dup2(file.as_raw_fd(), destination) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn redirect_handles(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> *mut core::ffi::c_void;
        fn SetStdHandle(kind: u32, handle: *mut core::ffi::c_void) -> i32;
    }
    const OUTPUT: u32 = -11i32 as u32;
    const ERROR: u32 = -12i32 as u32;
    let old_output = unsafe { GetStdHandle(OUTPUT) };
    if unsafe { SetStdHandle(OUTPUT, file.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { SetStdHandle(ERROR, file.as_raw_handle()) } == 0 {
        let error = io::Error::last_os_error();
        let _ = unsafe { SetStdHandle(OUTPUT, old_output) };
        return Err(error);
    }
    // Rust 1.97.1's windows stdio gets GetStdHandle afresh on every write;
    // neither std::io::stdout nor stderr caches the old anonymous pipe handle.
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn redirect_handles(_: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "desktop stdio is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "dsh-web-stdio-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn stdio_log_requires_a_ready_file_absolute_path_and_existing_parent() {
        let root = root();
        std::fs::create_dir(&root).unwrap();
        assert!(validate_log_path(&root.join("host.log"), true).is_ok());
        assert!(validate_log_path(&root.join("host.log"), false).is_err());
        assert!(validate_log_path(Path::new("relative.log"), true).is_err());
        assert!(validate_log_path(&root.join("missing/host.log"), true).is_err());
        assert!(redirect(&root).is_err());
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn stdio_fixture_entry() {
        let Ok(log) = std::env::var("DSH_WEB_STDIO_FIXTURE_LOG") else {
            return;
        };
        let marker = PathBuf::from(std::env::var("DSH_WEB_STDIO_FIXTURE_MARKER").unwrap());
        let release = marker.with_extension("release");
        let _owned_log = redirect(Path::new(&log)).unwrap();
        println!("stdout before parent closes pipes");
        eprintln!("stderr before parent closes pipes");
        std::fs::write(&marker, b"ready").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !release.exists() {
            assert!(
                Instant::now() < deadline,
                "parent did not release stdio fixture"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        println!("stdout after parent closes pipes");
        eprintln!("stderr after parent closes pipes");
        io::stdout().flush().unwrap();
        io::stderr().flush().unwrap();
        // The child test harness writes its result after this test returns.
        // Retain the redirected Windows handles until this process exits.
        std::mem::forget(_owned_log);
    }

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn both_stdio_streams_append_and_survive_the_original_parent_pipe_closing() {
        let root = root();
        std::fs::create_dir(&root).unwrap();
        let log = root.join("host.log");
        let marker = root.join("ready");
        std::fs::write(&log, b"existing log preserved\n").unwrap();
        // Strip the crate prefix: works both under cargo --lib and a standalone
        // rustc --test web_stdio.rs invocation with no crate dependencies.
        let scope = module_path!().split_once("::").unwrap().1;
        let fixture = format!("{scope}::stdio_fixture_entry");
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &fixture, "--nocapture"])
                .env("DSH_WEB_STDIO_FIXTURE_LOG", &log)
                .env("DSH_WEB_STDIO_FIXTURE_MARKER", &marker)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "stdio fixture exited before ready"
            );
            assert!(
                Instant::now() < deadline,
                "stdio fixture never became ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(child.0.stdout.take());
        drop(child.0.stderr.take());
        std::fs::write(marker.with_extension("release"), b"continue").unwrap();
        let status = child.0.wait().unwrap();
        assert!(status.success(), "{status}");
        let text = std::fs::read_to_string(&log).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        for expected in [
            "existing log preserved",
            "dsh: desktop Host starting",
            "stdout before parent closes pipes",
            "stderr before parent closes pipes",
            "stdout after parent closes pipes",
            "stderr after parent closes pipes",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
