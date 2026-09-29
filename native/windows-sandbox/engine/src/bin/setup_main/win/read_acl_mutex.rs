use anyhow::Result;
use codex_windows_sandbox::to_wide;
use std::ffi::OsStr;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::System::Threading::ReleaseMutex;
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_ABANDONED};

const READ_ACL_MUTEX_NAME: &str = "Local\\DshSandboxReadAcl";

pub(super) struct ReadAclMutexGuard {
    handle: HANDLE,
}

impl Drop for ReadAclMutexGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            CloseHandle(self.handle);
        }
    }
}

pub(super) fn acquire_read_acl_mutex() -> Result<Option<ReadAclMutexGuard>> {
    let name = to_wide(OsStr::new(READ_ACL_MUTEX_NAME));
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if handle == 0 {
        return Err(anyhow::anyhow!("CreateMutexW failed: {}", unsafe {
            GetLastError()
        }));
    }
    let status = unsafe { WaitForSingleObject(handle,60_000) };
    if status != WAIT_OBJECT_0 && status != WAIT_ABANDONED {
        unsafe {
            CloseHandle(handle);
        }
        anyhow::bail!("PRIVATE_ACL_BUSY: ACL updates did not acquire the shared lease: {status}");
    }
    Ok(Some(ReadAclMutexGuard { handle }))
}
