//! Window handles are reusable; bind observations to a process lifetime.
use windows_sys::Win32::{Foundation::*, System::Threading::*, UI::WindowsAndMessaging::*};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowIdentity {
    pub window: usize,
    pub process: u32,
    pub created: u64,
    pub executable: String,
}
impl WindowIdentity {
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
