//! Caller-owned render surface and bounded screenshot capture.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows::{
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{Direct3D::D3D_DRIVER_TYPE_HARDWARE, Direct3D11::*, Dxgi::IDXGIDevice},
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
    core::{Interface, factory},
};
use windows_sys::Win32::UI::{
    Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE,
    },
    WindowsAndMessaging::*,
};
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub jpeg: Vec<u8>,
}
#[derive(Clone, Copy)]
pub struct FrameViewport {
    pub rect: [f32; 4],
    pub notified_at: i64,
}

pub fn frame_clock() -> Result<i64, String> {
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
    let mut counter = 0;
    let mut frequency = 0;
    if unsafe { QueryPerformanceCounter(&mut counter) } == 0
        || unsafe { QueryPerformanceFrequency(&mut frequency) } == 0
        || frequency <= 0
    {
        return Err("无法校验远端画面时间，请重新连接".into());
    }
    Ok((i128::from(counter) * 10_000_000 / i128::from(frequency)) as i64)
}

fn frame_matches_viewport(timestamp: i64, viewport: FrameViewport) -> bool {
    timestamp >= viewport.notified_at
}
pub struct RenderWindow {
    pub hwnd: usize,
    join: Option<std::thread::JoinHandle<()>>,
    pub escape_available: Arc<AtomicBool>,
    hotkey_requests: mpsc::SyncSender<(bool, mpsc::SyncSender<bool>)>,
}

const STOP_MODIFIERS: u32 = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
pub const STOP_SHORTCUT: &str = "Ctrl+Alt+Esc";

fn update_hotkey(
    registered: &mut bool,
    enabled: bool,
    mut change: impl FnMut(bool) -> bool,
) -> bool {
    if *registered == enabled {
        return true;
    }
    if !change(enabled) {
        return false;
    }
    *registered = enabled;
    true
}

impl RenderWindow {
    pub fn new(paused: Arc<AtomicBool>) -> Result<Self, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        let (hotkey_requests, changes) = mpsc::sync_channel::<(bool, mpsc::SyncSender<bool>)>(4);
        let available = Arc::new(AtomicBool::new(false));
        let available_thread = available.clone();
        let join = std::thread::spawn(move || unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                windows_sys::w!("STATIC"),
                windows_sys::w!("Harness desktop renderer"),
                WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN | 4,
                GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) + 64,
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                1920,
                1080,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if hwnd.is_null() {
                let _ = tx.send(Err("无法创建远程画面渲染器".to_string()));
                return;
            }
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetWindowPos(
                hwnd,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
            let mut registered = !paused.load(Ordering::SeqCst)
                && RegisterHotKey(hwnd, 1, STOP_MODIFIERS, VK_ESCAPE as u32) != 0;
            available_thread.store(registered, Ordering::SeqCst);
            if !paused.load(Ordering::SeqCst) && !registered {
                crate::sdk::set_pause_state(
                    &paused,
                    crate::sdk::PauseReason::EmergencyStopUnavailable,
                );
                DestroyWindow(hwnd);
                let _ = tx.send(Err(format!(
                    "COMPUTER_USE_EMERGENCY_STOP_UNAVAILABLE: 无法注册 {STOP_SHORTCUT} 全局急停键；请释放快捷键占用后重试"
                )));
                return;
            }
            let _ = tx.send(Ok((hwnd as usize, registered)));
            let mut message = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                if message.message == WM_APP + 1 {
                    break;
                }
                if message.message == WM_APP + 2 {
                    while let Ok((enabled, reply)) = changes.try_recv() {
                        let success = update_hotkey(&mut registered, enabled, |enabled| {
                            if enabled {
                                RegisterHotKey(hwnd, 1, STOP_MODIFIERS, VK_ESCAPE as u32) != 0
                            } else {
                                UnregisterHotKey(hwnd, 1) != 0
                            }
                        });
                        available_thread.store(registered, Ordering::SeqCst);
                        let _ = reply.send(success);
                    }
                    continue;
                }
                if message.message == WM_HOTKEY && message.wParam == 1 {
                    crate::sdk::set_pause_state(&paused, crate::sdk::PauseReason::EscapeHotkey);
                    UnregisterHotKey(hwnd, 1);
                    registered = false;
                    available_thread.store(false, Ordering::SeqCst);
                    continue;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if registered {
                UnregisterHotKey(hwnd, 1);
            }
            DestroyWindow(hwnd);
        });
        let (hwnd, _registered) = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "画面渲染器启动超时")??;
        Ok(Self {
            hwnd,
            join: Some(join),
            escape_available: available,
            hotkey_requests,
        })
    }
    pub fn control_changed(&self, agent: bool) -> Result<(), String> {
        let failure = || {
            format!(
                "COMPUTER_USE_EMERGENCY_STOP_UNAVAILABLE: 未能确认 {STOP_SHORTCUT} 全局急停键状态，智能体保持暂停"
            )
        };
        let (reply, result) = mpsc::sync_channel(1);
        self.hotkey_requests
            .try_send((agent, reply))
            .map_err(|_| failure())?;
        if unsafe { PostMessageW(self.hwnd as _, WM_APP + 2, 0, 0) } == 0 {
            return Err(failure());
        }
        if result.recv_timeout(Duration::from_secs(1)).unwrap_or(false) {
            Ok(())
        } else {
            Err(failure())
        }
    }
    pub fn refresh_size(&self) {
        unsafe {
            let mut rect = std::mem::zeroed();
            if GetClientRect(self.hwnd as _, &mut rect) != 0 {
                let size =
                    (rect.right - rect.left) as u32 | (((rect.bottom - rect.top) as u32) << 16);
                PostMessageW(
                    self.hwnd as _,
                    WM_SIZE,
                    SIZE_RESTORED as usize,
                    size as isize,
                );
            }
        }
    }
    pub fn metrics(&self) -> serde_json::Value {
        unsafe {
            let mut rect = std::mem::zeroed();
            let valid = GetClientRect(self.hwnd as _, &mut rect) != 0;
            serde_json::json!({"valid":valid,"width":rect.right-rect.left,"height":rect.bottom-rect.top,"visible":IsWindowVisible(self.hwnd as _)!=0})
        }
    }
    pub fn close(&mut self) {
        if self.hwnd == 0 {
            return;
        }
        unsafe {
            PostMessageW(self.hwnd as _, WM_APP + 1, 0, 0);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        self.hwnd = 0;
    }
}
impl Drop for RenderWindow {
    fn drop(&mut self) {
        self.close();
    }
}
pub struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging: Option<ID3D11Texture2D>,
    staging_size: (u32, u32),
    latest: Option<Frame>,
    accepted_frames: u64,
    discarded_before_viewport: u64,
    last_frame_time: Option<i64>,
    last_viewport_time: Option<i64>,
}
impl Capture {
    pub fn new(hwnd: usize) -> Result<Self, String> {
        Self::create(HWND(hwnd as _)).map_err(|error| format!("画面捕获初始化失败：{error}"))
    }
    fn create(hwnd: HWND) -> windows::core::Result<Self> {
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.unwrap();
            let context = context.unwrap();
            let dxgi: IDXGIDevice = device.cast()?;
            let rt: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            let interop: IGraphicsCaptureItemInterop =
                factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd)?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &rt,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                item.Size()?,
            )?;
            let session = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            session.StartCapture()?;
            Ok(Self {
                pool,
                session,
                device,
                context,
                staging: None,
                staging_size: (0, 0),
                latest: None,
                accepted_frames: 0,
                discarded_before_viewport: 0,
                last_frame_time: None,
                last_viewport_time: None,
            })
        }
    }
    pub fn read(
        &mut self,
        tick: impl FnMut() -> bool,
        viewport: impl Fn() -> Option<FrameViewport>,
        wait: Duration,
    ) -> Result<&Frame, String> {
        let image = self.read_pixels(tick, viewport, wait)?;
        self.latest = Some(Self::encode_snapshot(&image)?);
        Ok(self.latest.as_ref().unwrap())
    }
    pub fn read_pixels(
        &mut self,
        mut tick: impl FnMut() -> bool,
        viewport: impl Fn() -> Option<FrameViewport>,
        wait: Duration,
    ) -> Result<image::RgbImage, String> {
        let start = Instant::now();
        loop {
            if !tick() {
                return Err("控制操作已暂停或取消".into());
            }
            if start.elapsed() >= wait {
                return Err("尚未收到桌面视频帧，请检查远端连接".into());
            }
            if let Ok(mut frame) = self.pool.TryGetNextFrame() {
                for _ in 0..2 {
                    if let Ok(newer) = self.pool.TryGetNextFrame() {
                        let _ = frame.Close();
                        frame = newer
                    } else {
                        break;
                    }
                }
                if let Some(viewport) = viewport() {
                    let timestamp = frame.SystemRelativeTime().map(|time| time.Duration);
                    let timestamp = match timestamp {
                        Ok(timestamp) => timestamp,
                        Err(error) => {
                            let _ = frame.Close();
                            return Err(format!("无法读取远端视频帧时间：{error}"));
                        }
                    };
                    if !frame_matches_viewport(timestamp, viewport) {
                        self.discarded_before_viewport += 1;
                        let _ = frame.Close();
                        continue;
                    }
                    let image = self.copy_pixels(&frame, viewport.rect);
                    let _ = frame.Close();
                    let image = image.map_err(|e| format!("无法读取远端视频帧：{e}"))?;
                    self.accepted_frames += 1;
                    self.last_frame_time = Some(timestamp);
                    self.last_viewport_time = Some(viewport.notified_at);
                    return Ok(image);
                }
                let _ = frame.Close();
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({
            "acceptedFrames": self.accepted_frames,
            "discardedBeforeViewport": self.discarded_before_viewport,
            "lastFrameTimestamp100ns": self.last_frame_time,
            "lastViewportTimestamp100ns": self.last_viewport_time,
            "source": "sdk-render-window"
        })
    }
    fn copy_pixels(
        &mut self,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
        viewport: [f32; 4],
    ) -> windows::core::Result<image::RgbImage> {
        unsafe {
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let texture: ID3D11Texture2D = access.GetInterface()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Width == 0
                || desc.Height == 0
                || u64::from(desc.Width) * u64::from(desc.Height) > 16_000_000
            {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80070057u32 as i32,
                )));
            }
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            if self.staging_size != (desc.Width, desc.Height) || self.staging.is_none() {
                let mut stage = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut stage))?;
                self.staging = stage;
                self.staging_size = (desc.Width, desc.Height);
            }
            let stage = self.staging.as_ref().unwrap();
            self.context.CopyResource(stage, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(stage, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut rgb = vec![0u8; desc.Width as usize * desc.Height as usize * 3];
            for y in 0..desc.Height as usize {
                let row = std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    desc.Width as usize * 4,
                );
                for x in 0..desc.Width as usize {
                    let at = (y * desc.Width as usize + x) * 3;
                    rgb[at..at + 3].copy_from_slice(&[row[x * 4 + 2], row[x * 4 + 1], row[x * 4]]);
                }
            }
            self.context.Unmap(stage, 0);
            let image = image::RgbImage::from_raw(desc.Width, desc.Height, rgb).unwrap();
            let left = (viewport[0].floor() as u32).min(desc.Width);
            let top = (viewport[1].floor() as u32).min(desc.Height);
            let right = ((viewport[0] + viewport[2]).ceil() as u32).min(desc.Width);
            let bottom = ((viewport[1] + viewport[3]).ceil() as u32).min(desc.Height);
            if right <= left || bottom <= top {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80070057u32 as i32,
                )));
            }
            let image = if left == 0 && top == 0 && right == desc.Width && bottom == desc.Height {
                image
            } else {
                image::imageops::crop_imm(&image, left, top, right - left, bottom - top).to_image()
            };
            Ok(image)
        }
    }
    fn encode_snapshot(image: &image::RgbImage) -> Result<Frame, String> {
        let scale = (640000.0 / (f64::from(image.width()) * f64::from(image.height())))
            .sqrt()
            .min(1.0);
        let width = (f64::from(image.width()) * scale).floor().max(1.0) as u32;
        let height = (f64::from(image.height()) * scale).floor().max(1.0) as u32;
        let resized;
        let source = if width != image.width() || height != image.height() {
            resized = image::imageops::resize(
                image,
                width,
                height,
                image::imageops::FilterType::Triangle,
            );
            &resized
        } else {
            image
        };
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
            .encode(source, width, height, image::ExtendedColorType::Rgb8)
            .map_err(|e| format!("截图编码失败：{e}"))?;
        Ok(Frame {
            width,
            height,
            jpeg,
        })
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_stop_requires_modifiers_and_registration_confirmation() {
        assert_eq!(
            STOP_MODIFIERS & (MOD_CONTROL | MOD_ALT),
            MOD_CONTROL | MOD_ALT
        );
        let mut registered = false;
        assert!(!update_hotkey(&mut registered, true, |_| false));
        assert!(
            !registered,
            "failed registration must not enable agent control"
        );
        assert!(update_hotkey(&mut registered, true, |_| true));
        assert!(registered);
        assert!(update_hotkey(&mut registered, true, |_| panic!(
            "already registered"
        )));
        assert!(update_hotkey(&mut registered, false, |_| true));
        assert!(!registered);
    }

    #[test]
    fn compositor_frames_before_sdk_layout_are_not_remote_frames() {
        let viewport = FrameViewport {
            rect: [0.0, 0.0, 1920.0, 1080.0],
            notified_at: 500,
        };
        assert!(!frame_matches_viewport(499, viewport));
        assert!(frame_matches_viewport(500, viewport));
        assert!(frame_matches_viewport(510, viewport));
        let resized = FrameViewport {
            notified_at: 600,
            rect: [0.0, 0.0, 1280.0, 720.0],
        };
        assert!(!frame_matches_viewport(510, resized));
        assert!(frame_matches_viewport(601, resized));
    }
}
