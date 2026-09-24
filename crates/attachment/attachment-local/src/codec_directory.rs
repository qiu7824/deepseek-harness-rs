//! Private job directories with a cross-process owner lease. Recovery only
//! removes recognized, unlocked codec jobs and never walks arbitrary contents.
use super::{AttachmentError, Path, PathBuf, VERSION, check_path, error, io_error};
use fs2::FileExt;
use std::io::{Read, Seek, Write};

pub(super) struct Directory {
    pub path: PathBuf,
    owner: Option<std::fs::File>,
}
impl Directory {
    pub fn create(parent: &Path, nonce: &str) -> Result<Self, AttachmentError> {
        check_path(parent)?;
        std::fs::create_dir_all(parent).map_err(io_error)?;
        recover(parent)?;
        let path = parent.join(nonce);
        create_private(&path)?;
        let mut directory = Self { path, owner: None };
        let mut owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path.join("owner.json"))
            .map_err(io_error)?;
        owner.try_lock_exclusive().map_err(io_error)?;
        owner
            .write_all(
                serde_json::json!({"version":VERSION,"nonce":nonce})
                    .to_string()
                    .as_bytes(),
            )
            .map_err(io_error)?;
        owner.sync_all().map_err(io_error)?;
        directory.owner = Some(owner);
        Ok(directory)
    }
}
fn remove_leaves(path: &Path) -> bool {
    for leaf in ["input", "output", "request.json", "reply.json"] {
        let _ = std::fs::remove_file(path.join(leaf));
    }
    ["input", "output", "request.json", "reply.json"]
        .iter()
        .all(|leaf| !path.join(leaf).exists())
}
impl Drop for Directory {
    fn drop(&mut self) {
        let clean = remove_leaves(&self.path);
        self.owner.take();
        if clean {
            let _ = std::fs::remove_file(self.path.join("owner.json"));
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}
pub(super) fn recover(parent: &Path) -> Result<(), AttachmentError> {
    check_path(parent)?;
    // Bound startup work; remaining stale jobs are considered on later requests.
    for entry in std::fs::read_dir(parent)
        .map_err(io_error)?
        .take(64)
        .flatten()
    {
        let path = entry.path();
        let Some(nonce) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if uuid::Uuid::parse_str(nonce).is_err() || check_path(&path).is_err() {
            continue;
        }
        let owner_path = path.join("owner.json");
        if check_path(&owner_path).is_err() {
            continue;
        }
        let Ok(mut owner) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&owner_path)
        else {
            continue;
        };
        if owner.try_lock_exclusive().is_err() {
            continue;
        }
        let mut data = String::new();
        if (&mut owner).take(4097).read_to_string(&mut data).is_err() || data.len() > 4096 {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if value["version"].as_u64() != Some(VERSION as u64)
            || value["nonce"].as_str() != Some(nonce)
        {
            continue;
        }
        // Do not remove a directory containing anything outside our protocol.
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        if entries.flatten().any(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some("owner.json" | "input" | "output" | "request.json" | "reply.json")
            )
        }) {
            continue;
        }
        if !remove_leaves(&path) {
            continue;
        }
        // Invalidate the marker while locked before unlinking it; a second
        // sweeper cannot claim the same generation after this handle closes.
        if owner.set_len(0).is_err() || owner.rewind().is_err() {
            continue;
        }
        drop(owner);
        let _ = std::fs::remove_file(&owner_path);
        let _ = std::fs::remove_dir(&path);
    }
    Ok(())
}

#[cfg(unix)]
fn create_private(path: &Path) -> Result<(), AttachmentError> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(io_error)
}
#[cfg(windows)]
fn create_private(path: &Path) -> Result<(), AttachmentError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    // Protected DACL: only the owner and LocalSystem; descendants inherit it.
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)\0"
        .encode_utf16()
        .collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) };
    let failure = if result == 0 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    unsafe {
        LocalFree(descriptor);
    }
    if let Some(failure) = failure {
        return Err(io_error(failure));
    }
    check_path(path).map_err(|_| {
        error(
            "ATTACHMENT_UNSAFE_PATH",
            "Image job directory changed during creation.",
        )
    })
}
