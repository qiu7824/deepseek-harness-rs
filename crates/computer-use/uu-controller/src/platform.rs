//! Process-scoped Windows graphics and media initialization.
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
#[link(name = "mfplat")]
unsafe extern "system" {
    fn MFStartup(version: u32, flags: u32) -> i32;
    fn MFShutdown() -> i32;
}
pub struct MediaPlatform;
impl MediaPlatform {
    pub fn new() -> Result<Self, String> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|_| "Windows 图形组件初始化失败")?;
        if unsafe { MFStartup(0x00020070, 0) } < 0 {
            unsafe { RoUninitialize() };
            return Err("Windows 媒体组件初始化失败，请检查系统媒体功能".into());
        }
        Ok(Self)
    }
}
impl Drop for MediaPlatform {
    fn drop(&mut self) {
        unsafe {
            MFShutdown();
            RoUninitialize();
        }
    }
}
