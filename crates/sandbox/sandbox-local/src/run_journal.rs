// Durable ownership records contain only sandbox identity and granted paths.
// They intentionally omit commands, arguments, outputs, and credentials.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

const MAX_RECORD_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Redirect {
    link: PathBuf,
    target: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    version: u32,
    pid: u32,
    profile: String,
    sid: String,
    paths: Vec<PathBuf>,
    redirect: Option<Redirect>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    entry: Entry,
    sha256: String,
}

pub(super) struct Journal(PathBuf);

impl Journal {
    pub(super) fn begin(
        directory: &Path,
        profile: &AppContainerProfile,
        paths: Vec<PathBuf>,
        redirect: Option<&ManagedTempRedirect>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(directory)
            .map_err(|e| format!("create sandbox recovery directory: {e}"))?;
        let directory = std::fs::canonicalize(directory).map_err(|e| e.to_string())?;
        let profile_name = String::from_utf16_lossy(&profile.name[..profile.name.len() - 1]);
        let entry = Entry {
            version: 1,
            pid: std::process::id(),
            profile: profile_name.clone(),
            sid: sid_string(profile.sid.0)?,
            paths: paths
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            redirect: redirect.map(|redirect| Redirect {
                link: redirect.link.clone(),
                target: redirect.target.clone(),
            }),
        };
        let checksum = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&entry).map_err(|e| e.to_string())?)
        );
        let bytes = serde_json::to_vec(&Record {
            entry,
            sha256: checksum,
        })
        .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err("sandbox recovery record exceeds its budget".into());
        }
        let pending = directory.join(format!("{profile_name}.pending"));
        let path = directory.join(format!("{profile_name}.json"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        // Publish without overwriting any existing ownership record.
        std::fs::hard_link(&pending, &path)
            .map_err(|e| format!("publish sandbox recovery record: {e}"))?;
        std::fs::remove_file(pending).map_err(|e| e.to_string())?;
        Ok(Self(path))
    }

    pub(super) fn complete(self) -> Result<(), String> {
        std::fs::remove_file(self.0).map_err(|e| format!("retire sandbox recovery record: {e}"))
    }
}

fn load(path: &Path) -> Result<Entry, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    use std::os::windows::fs::MetadataExt;
    if !meta.is_file() || meta.file_attributes() & 0x400 != 0 || meta.len() > MAX_RECORD_BYTES {
        return Err("invalid sandbox recovery file".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err("sandbox recovery file exceeds its budget".into());
    }
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|_| "incomplete sandbox recovery record".to_string())?;
    if format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&record.entry).map_err(|e| e.to_string())?)
    ) != record.sha256
    {
        return Err("sandbox recovery record checksum mismatch".into());
    }
    let entry = record.entry;
    let prefix = format!("DeepSeekHarnessSandbox.{}.", entry.pid);
    if entry.version != 1
        || entry.pid == 0
        || !entry.profile.starts_with(&prefix)
        || entry.profile[prefix.len()..].parse::<u128>().is_err()
        || entry.paths.is_empty()
        || entry.paths.len() > 1024
        || entry.paths.iter().any(|path| !path.is_absolute())
        || path.file_stem().and_then(OsStr::to_str) != Some(entry.profile.as_str())
    {
        return Err("sandbox recovery ownership record is invalid".into());
    }
    Ok(entry)
}

fn process_alive(pid: u32) -> Result<bool, String> {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return if unsafe { GetLastError() } == 87 {
            Ok(false)
        } else {
            Err(last_error("inspect sandbox runner"))
        };
    }
    let process = Handle(raw);
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
        return Err(last_error("inspect sandbox runner exit"));
    }
    // PID reuse is conservative: any live process with that id delays cleanup.
    Ok(code == 259)
}

pub(super) fn revoke_if_owned(path: &Path, sid: PSID) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::{Authorization::*, *};
    use windows_sys::Win32::Storage::FileSystem::*;
    if !path.is_dir() {
        return Ok(());
    }
    let mut raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            READ_CONTROL | WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    let mut writable = true;
    if raw == INVALID_HANDLE_VALUE && unsafe { GetLastError() } == 5 {
        writable = false;
        raw = unsafe {
            CreateFileW(
                wide(path.as_os_str()).as_ptr(),
                READ_CONTROL,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                null_mut(),
            )
        };
    }
    if raw == INVALID_HANDLE_VALUE {
        return Err(last_error("open sandbox recovery target"));
    }
    let handle = Handle(raw);
    let (mut descriptor, mut acl) = (null_mut(), null_mut());
    let status = unsafe {
        GetSecurityInfo(
            handle.0,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(format!("read sandbox recovery ACL: {status}"));
    }
    let mut inheritance = None;
    if !acl.is_null() {
        for index in 0..unsafe { (*acl).AceCount } as u32 {
            let mut ace = null_mut();
            if unsafe { GetAce(acl, index, &mut ace) } == 0 {
                continue;
            }
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if header.AceType != 0 || u32::from(header.AceFlags) & INHERITED_ACE != 0 {
                continue;
            }
            let grant = ace.cast::<ACCESS_ALLOWED_ACE>();
            let identity = unsafe { (&(*grant).SidStart as *const u32).cast_mut().cast() };
            if unsafe { EqualSid(identity, sid) } != 0 {
                inheritance = Some(
                    inheritance.unwrap_or(0)
                        | (u32::from(header.AceFlags)
                            & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)),
                );
            }
        }
    }
    unsafe {
        LocalFree(descriptor);
    }
    // Derive propagation from the actual owned ACE, never from journal input.
    if let Some(inheritance) = inheritance {
        if !writable {
            return Err(format!(
                "cannot revoke owned sandbox ACL on {}: access denied",
                path.display()
            ));
        }
        update_access(handle.0, sid, false, 0, inheritance)?;
    }
    Ok(())
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn clean(entry: &Entry) -> Result<(), String> {
    #[link(name = "userenv")]
    unsafe extern "system" {
        fn DeriveAppContainerSidFromAppContainerName(name: *const u16, sid: *mut PSID) -> i32;
    }
    let name = wide(&entry.profile);
    let mut sid = null_mut();
    let result = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
    if result < 0 {
        return Err("derive sandbox recovery identity failed".into());
    }
    let sid = Sid(sid);
    if sid_string(sid.0)? != entry.sid {
        return Err("sandbox recovery SID does not match its owned profile".into());
    }
    for path in &entry.paths {
        revoke_if_owned(path, sid.0)?;
    }
    if let Some(redirect) = &entry.redirect {
        let expected = Path::new(&entry.profile.to_ascii_lowercase()).join("AC/Temp");
        if !redirect.link.is_absolute()
            || !redirect.target.is_absolute()
            || !normalized(&redirect.link).ends_with(&format!("\\{}", normalized(&expected)))
        {
            return Err("sandbox temporary redirect identity changed; data preserved".into());
        }
        match std::fs::symlink_metadata(&redirect.link) {
            Ok(meta) => {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 == 0
                    || std::fs::read_link(&redirect.link)
                        .map(|path| normalized(&path) != normalized(&redirect.target))
                        .unwrap_or(true)
                {
                    return Err("sandbox temporary redirect changed; data preserved".into());
                }
                // Remove the verified junction itself, never its target tree.
                std::fs::remove_dir(&redirect.link)
                    .map_err(|e| format!("release sandbox temporary link: {e}"))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let result = unsafe { DeleteAppContainerProfile(name.as_ptr()) };
    if result < 0 && ![0x80070002, 0x80070003, 0x80070490].contains(&(result as u32)) {
        return Err(format!(
            "remove stopped sandbox profile: 0x{:08x}",
            result as u32
        ));
    }
    Ok(())
}

pub(super) fn recover(directory: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    let Ok(files) = std::fs::read_dir(directory) else {
        return warnings;
    };
    for file in files
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.starts_with("DeepSeekHarnessSandbox.") && name.ends_with(".json")
            })
        })
        .take(128)
    {
        let result = (|| {
            let entry = load(&file.path())?;
            if process_alive(entry.pid)? {
                return Ok(());
            }
            clean(&entry)?;
            std::fs::remove_file(file.path()).map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            warnings.push(error);
        }
    }
    warnings.truncate(4);
    warnings
}
