//! One active process tree per account: untrusted siblings cannot race helper startup.
use anyhow::{Result, ensure};
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};
pub const READ_SLOTS: usize = 2;
pub const WRITE_SLOTS: usize = 4;
pub const SIZE: usize = READ_SLOTS + WRITE_SLOTS;

pub fn initialize_token_owner(root: &Path) -> Result<()> {
    use sha2::{Digest, Sha256};
    let parent = root.parent().ok_or_else(||anyhow::anyhow!("state directory has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let identity = format!("{:x}",Sha256::digest(root.to_string_lossy().to_lowercase().as_bytes()));
    let lock = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(parent.join(format!(".dsh-token-{}.lock",&identity[..20])))?;
    lock.lock_exclusive()?;
    validate_owner(root,true)?;
    let path = root.join("dsh-native-pool.json");
    let mut owner: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    if owner["protected"] != true {
        protect_owner(root)?;
        owner["protected"] = serde_json::json!(true);
        std::fs::write(path,serde_json::to_vec(&owner)?)?;
    }
    Ok(())
}
pub fn home(root: &Path, workspace: &Path, index: usize) -> PathBuf {
    if index < READ_SLOTS {
        return root.join("readonly").join(index.to_string());
    }
    use sha2::{Digest, Sha256};
    let workspace = codex_windows_sandbox::canonicalize_path(workspace)
        .to_string_lossy()
        .to_lowercase();
    let hash = format!("{:x}", Sha256::digest(workspace.as_bytes()));
    root.join("projects")
        .join(&hash[..24])
        .join((index - READ_SLOTS).to_string())
}

pub fn validate_owner(root: &Path, setup: bool) -> Result<bool> {
    let path = root.join("dsh-native-pool.json");
    let principal = codex_windows_sandbox::current_user_sid_string()?;
    if !path.exists() {
        if !setup {
            return Ok(false);
        }
        if root.exists() {
            ensure!(
                std::fs::read_dir(root)?.next().is_none(),
                "STATE_OWNERSHIP_MISMATCH: pool directory is not empty"
            );
        }
        std::fs::create_dir_all(root)?;
        use std::io::Write;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&serde_json::to_vec(
            &serde_json::json!({"product":"deepseek-harness-rs","version":1,"ownerSid":principal}),
        )?)?;
        file.sync_all()?;
    }
    let owner: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        owner["product"] == "deepseek-harness-rs"
            && owner["version"] == 1
            && owner["ownerSid"] == principal,
        "STATE_OWNERSHIP_MISMATCH: native pool belongs to another owner"
    );
    Ok(true)
}

pub fn protect_owner(root: &Path) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SE_FILE_OBJECT,
                SetNamedSecurityInfoW,
            },
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
            PROTECTED_DACL_SECURITY_INFORMATION,
        },
    };
    let principal = codex_windows_sandbox::current_user_sid_string()?;
    let descriptor_text: Vec<_> =
        format!("D:P(A;OICI;GA;;;SY)(A;OICI;GA;;;BA)(A;OICI;GA;;;{principal})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
    let mut descriptor = std::ptr::null_mut();
    ensure!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                descriptor_text.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } != 0,
        "cannot create pool protection"
    );
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = std::ptr::null_mut();
    let ok =
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) };
    let path: Vec<_> = root
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let status = if ok != 0 && present != 0 {
        unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null_mut(),
            )
        }
    } else {
        87
    };
    unsafe {
        LocalFree(descriptor.cast());
    }
    ensure!(status == 0, "cannot protect pool state: {status}");
    Ok(())
}

fn runner_alive(home: &Path) -> Result<bool> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError},
        System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };
    let path = home.join("active-runner.json");
    if !path.exists() {
        return Ok(false);
    }
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        record["phase"] == "running",
        "NATIVE_SLOT_QUARANTINED: interrupted helper startup needs explicit repair"
    );
    let pid = record["pid"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| anyhow::anyhow!("invalid native runner identity"))?;
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle == 0 {
        ensure!(
            unsafe { GetLastError() } == ERROR_INVALID_PARAMETER,
            "cannot verify previous native runner lifetime"
        );
        return Ok(false);
    }
    let mut created = unsafe { std::mem::zeroed() };
    let mut exited = unsafe { std::mem::zeroed() };
    let mut kernel = unsafe { std::mem::zeroed() };
    let mut user = unsafe { std::mem::zeroed() };
    let mut code = 0;
    let ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    let status = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe {
        CloseHandle(handle);
    }
    ensure!(
        ok != 0 && status != 0,
        "cannot verify native process record"
    );
    let lifetime = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    Ok(record["created"].as_u64() == Some(lifetime) && code == 259)
}

pub fn acquire(root: &Path, workspace: &Path, index: usize) -> Result<Option<File>> {
    let slot = home(root, workspace, index);
    acquire_directory(&slot)
}

fn acquire_directory(slot: &Path) -> Result<Option<File>> {
    std::fs::create_dir_all(&slot)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(slot.join("execution.lock"))?;
    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    if runner_alive(&slot)? {
        return Ok(None);
    }
    Ok(Some(file))
}

pub fn acquire_other_projects(root: &Path, workspace: &Path) -> Result<Vec<File>> {
    let current = home(root, workspace, READ_SLOTS)
        .parent()
        .unwrap()
        .to_path_buf();
    let mut leases = Vec::new();
    let projects = root.join("projects");
    if projects.exists() {
        for entry in std::fs::read_dir(projects)? {
            let project = entry?.path();
            if project == current || !project.is_dir() {
                continue;
            }
            for index in 0..WRITE_SLOTS {
                let slot = project.join(index.to_string());
                if slot.exists() {
                    leases.push(acquire_directory(&slot)?.ok_or_else(|| {
                        anyhow::anyhow!("NATIVE_SLOT_BUSY: stop all native commands before setup")
                    })?);
                }
            }
        }
    }
    Ok(leases)
}

pub fn settle(slot: &Path) {
    for _ in 0..100 {
        match runner_alive(slot) {
            Ok(false) => {
                let _ = std::fs::remove_file(slot.join("active-runner.json"));
                return;
            }
            Err(_) => return,
            Ok(true) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}
