//! Process-scoped Windows graphics and DPI initialization.
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
pub struct MediaPlatform;
impl MediaPlatform {
    pub fn new() -> Result<Self, String> {
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|_| "Windows 图形组件初始化失败")?;
        Ok(Self)
    }
}
impl Drop for MediaPlatform {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}
