//! Installed runtimes receive a read-only capability; workspace writes always
//! use the runner's fresh AppContainer SID. No cached SID grants write access.
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GENERIC_ALL, GENERIC_WRITE, GetLastError, INVALID_HANDLE_VALUE, LocalFree,
    },
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
};

pub struct RuntimeReadAccess(PSID);
impl Drop for RuntimeReadAccess {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
impl RuntimeReadAccess {
    pub fn raw(&self) -> PSID {
        self.0
    }
    fn derive(name: &str) -> Result<Self, String> {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let (mut groups, mut sids) = (null_mut(), null_mut());
        let (mut group_count, mut count) = (0, 0);
        let ok = unsafe {
            DeriveCapabilitySidsFromName(
                name.as_ptr(),
                &mut groups,
                &mut group_count,
                &mut sids,
                &mut count,
            )
        };
        let error = unsafe { GetLastError() };
        let chosen = if ok != 0 && count == 1 && !sids.is_null() {
            unsafe { *sids }
        } else {
            null_mut()
        };
        unsafe {
            if !groups.is_null() {
                for i in 0..group_count {
                    LocalFree(*groups.add(i as usize));
                }
                LocalFree(groups.cast());
            }
            if !sids.is_null() {
                for i in 0..count {
                    let sid = *sids.add(i as usize);
                    if sid != chosen {
                        LocalFree(sid);
                    }
                }
                LocalFree(sids.cast());
            }
        }
        if chosen.is_null() {
            Err(format!(
                "derive read-only runtime capability: Windows error {error}"
            ))
        } else {
            Ok(Self(chosen))
        }
    }
    pub fn acquire(
        root: &Path,
        cache: &Path,
        grant: impl FnOnce(&Path, PSID) -> Result<(), String>,
    ) -> Result<Self, String> {
        let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
        if root.parent().is_none()
            || std::env::var_os("USERPROFILE")
                .and_then(|value| fs::canonicalize(value).ok())
                .is_some_and(|profile| profile == root)
        {
            return Err("the user profile or drive root cannot be a cached runtime".into());
        }
        if !root.join("python.exe").is_file() || !root.join("Lib").is_dir() || !cache.is_absolute()
        {
            return Err("invalid installed Python runtime or permission-state directory".into());
        }
        fs::create_dir_all(cache).map_err(|e| e.to_string())?;
        let cache = fs::canonicalize(cache).map_err(|e| e.to_string())?;
        let (identity, _, _) = root_state(&root, null_mut())?;
        let key = format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "runtime-read-v1\n{}\n{}\n{}",
                    cache.display(),
                    root.display(),
                    identity
                )
                .to_lowercase()
                .as_bytes()
            )
        );
        let access = Self::derive(&format!("DeepSeekHarness.RuntimeReadOnly.{key}"))?;
        let lease = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(cache.join(format!("{key}.lock")))
            .map_err(|e| e.to_string())?;
        fs2::FileExt::lock_exclusive(&lease).map_err(|e| e.to_string())?;
        let ready = cache.join(format!("{key}.ready"));
        if !ready.exists()
            && fs::read_dir(&cache)
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|suffix| suffix == "ready")
                })
                .take(256)
                .count()
                >= 256
        {
            return Err("read-only runtime permission state limit reached".into());
        }
        let (current, present, writable) = root_state(&root, access.raw())?;
        if current != identity || writable {
            return Err("runtime identity or read-only capability changed unexpectedly".into());
        }
        let marker = fs::symlink_metadata(&ready)
            .ok()
            .filter(|meta| meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 256)
            .and_then(|_| {
                let mut text = String::new();
                std::fs::File::open(&ready)
                    .ok()?
                    .take(256)
                    .read_to_string(&mut text)
                    .ok()?;
                Some(text)
            });
        if !present || marker.as_deref() != Some(identity.as_str()) {
            grant(&root, access.raw())?;
            let (after, present, writable) = root_state(&root, access.raw())?;
            if after != identity || !present || writable {
                return Err("read-only runtime permission verification failed".into());
            }
            let temporary = ready.with_extension(format!("{}.tmp", std::process::id()));
            fs::write(&temporary, &identity).map_err(|e| e.to_string())?;
            fs::rename(&temporary, &ready).map_err(|e| e.to_string())?;
        }
        Ok(access)
    }
}

fn root_state(path: &Path, sid: PSID) -> Result<(String, bool, bool), String> {
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let handle = CreateFileW(
            name.as_ptr(),
            READ_CONTROL | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!("inspect runtime: Windows error {}", GetLastError()));
        }
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(handle, &mut info) == 0 {
            let error = GetLastError();
            CloseHandle(handle);
            return Err(format!("runtime file identity: Windows error {error}"));
        }
        let identity = format!(
            "{}:{}:{}",
            info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
        );
        let mut descriptor = null_mut();
        let mut acl = null_mut();
        let status = GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        );
        CloseHandle(handle);
        if status != 0 {
            return Err(format!("runtime ACL: Windows error {status}"));
        }
        let mut readable = false;
        let mut writable = false;
        if !sid.is_null() && !acl.is_null() {
            for i in 0..(*acl).AceCount as u32 {
                let mut ace = null_mut();
                if GetAce(acl, i, &mut ace) == 0 {
                    continue;
                }
                let header = &*ace.cast::<ACE_HEADER>();
                if header.AceType != 0 {
                    continue;
                }
                let entry = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                if EqualSid((&entry.SidStart as *const u32).cast_mut().cast(), sid) == 0 {
                    continue;
                }
                // Only write-bearing bits: FILE_GENERIC_WRITE also contains read
                // control/synchronize bits and must not be used as this mask.
                writable |= entry.Mask
                    & (FILE_WRITE_DATA
                        | FILE_APPEND_DATA
                        | FILE_WRITE_EA
                        | FILE_WRITE_ATTRIBUTES
                        | FILE_DELETE_CHILD
                        | DELETE
                        | WRITE_DAC
                        | WRITE_OWNER
                        | GENERIC_WRITE
                        | GENERIC_ALL)
                    != 0;
                let flags = u32::from(header.AceFlags);
                let rights = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
                readable |= entry.Mask & rights == rights
                    && flags & INHERIT_ONLY_ACE == 0
                    && flags & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)
                        == OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
            }
        }
        if !descriptor.is_null() {
            LocalFree(descriptor);
        }
        Ok((identity, readable, writable))
    }
}
