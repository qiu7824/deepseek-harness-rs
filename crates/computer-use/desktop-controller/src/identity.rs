//! Window handles are reusable; bind observations to a process lifetime.
use sha2::{Digest, Sha256};
use windows_sys::Win32::{Foundation::*, System::Threading::*, UI::WindowsAndMessaging::*};

pub fn executable_revision(path: &str) -> Result<String, String> {
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).map_err(|_| "COMPUTER_USE_APP_IDENTITY_UNAVAILABLE")?;
    if file
        .metadata()
        .map_err(|_| "COMPUTER_USE_APP_IDENTITY_UNAVAILABLE")?
        .len()
        > 512 * 1024 * 1024
    {
        return Err("COMPUTER_USE_APP_IDENTITY_TOO_LARGE".into());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let length = file
            .read(&mut buffer)
            .map_err(|_| "COMPUTER_USE_APP_IDENTITY_UNAVAILABLE")?;
        if length == 0 {
            break;
        }
        digest.update(&buffer[..length]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowIdentity {
    pub window: usize,
    pub process: u32,
    pub created: u64,
    pub executable: String,
}
impl WindowIdentity {
    pub fn permission_identity(&self) -> Result<serde_json::Value, String> {
        self.validate()?;
        Ok(
            serde_json::json!({"applicationId":self.executable.to_ascii_lowercase(),"applicationRevision":executable_revision(&self.executable)?,"targetRevision":self.window_ref(),"label":self.executable}),
        )
    }
    pub fn read(window: usize) -> Result<Self, String> {
        unsafe {
            if IsWindow(window as HWND) == 0 {
                return Err("COMPUTER_USE_STALE_WINDOW".into());
            }
            let mut process = 0;
            GetWindowThreadProcessId(window as HWND, &mut process);
            if process == 0 {
                return Err("COMPUTER_USE_STALE_WINDOW".into());
            }
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process);
            if handle.is_null() {
                return Err("COMPUTER_USE_APP_IDENTITY_UNAVAILABLE".into());
            }
            let (mut created, mut exit, mut kernel, mut user) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            let times = GetProcessTimes(handle, &mut created, &mut exit, &mut kernel, &mut user);
            let mut buffer = vec![0u16; 32768];
            let mut size = buffer.len() as u32;
            let path = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
            CloseHandle(handle);
            if times == 0 || path == 0 {
                return Err("COMPUTER_USE_APP_IDENTITY_UNAVAILABLE".into());
            }
            Ok(Self {
                window,
                process,
                created: ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
                executable: String::from_utf16_lossy(&buffer[..size as usize]),
            })
        }
    }
    pub fn app_ref(&self) -> String {
        format!("app:{}:{}", self.process, self.created)
    }
    pub fn window_ref(&self) -> String {
        format!("{}:{}", self.app_ref(), self.window)
    }
    pub fn validate(&self) -> Result<(), String> {
        let current = Self::read(self.window)?;
        if current != *self {
            return Err("COMPUTER_USE_STALE_WINDOW".into());
        }
        Ok(())
    }
}
