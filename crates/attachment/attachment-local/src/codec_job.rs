//! Owned codec process lifetime. The worker waits for its stdin gate before
//! accessing image data; the parent assigns the Job before opening that gate.
#[cfg(windows)]
mod platform {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::*;

    pub struct ProcessJob(HANDLE);
    unsafe impl Send for ProcessJob {}
    impl ProcessJob {
        pub fn attach(child: &tokio::process::Child) -> std::io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            limits.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            // A codec never needs descendants. Reject them in the kernel.
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
            if unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let process = child
                .raw_handle()
                .ok_or_else(|| std::io::Error::other("codec process exited before assignment"))?;
            if unsafe { AssignProcessToJobObject(handle, process as HANDLE) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }
    }
    impl Drop for ProcessJob {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
#[cfg(unix)]
mod platform {
    pub struct ProcessJob(u32);
    impl ProcessJob {
        pub fn attach(child: &tokio::process::Child) -> std::io::Result<Self> {
            child
                .id()
                .map(Self)
                .ok_or_else(|| std::io::Error::other("codec exited before assignment"))
        }
    }
    impl Drop for ProcessJob {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-(self.0 as i32), libc::SIGKILL);
            }
        }
    }
}
pub use platform::ProcessJob;
