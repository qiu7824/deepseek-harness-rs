//! Bounded screenshots from a native monitor or explicitly selected window.
use std::time::{Duration, Instant};
use windows::{
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE, Direct3D11::*, Dxgi::IDXGIDevice, Gdi::HMONITOR,
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
    core::{Interface, factory},
};
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub jpeg: Vec<u8>,
}
pub struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    runtime_device: IDirect3DDevice,
    item: GraphicsCaptureItem,
    size: windows::Graphics::SizeInt32,
    latest: Option<Frame>,
    minimum_frame_time: i64,
}
impl Capture {
    pub fn new(hwnd: Option<usize>, monitor: usize) -> Result<Self, String> {
        Self::create(hwnd, monitor).map_err(|error| format!("画面捕获初始化失败：{error}"))
    }
    fn create(hwnd: Option<usize>, monitor: usize) -> windows::core::Result<Self> {
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
            let item: GraphicsCaptureItem = if let Some(hwnd) = hwnd {
                interop.CreateForWindow(HWND(hwnd as _))?
            } else {
                interop.CreateForMonitor(HMONITOR(monitor as _))?
            };
            let size = item.Size()?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &rt,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;
            let session = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            session.StartCapture()?;
            Ok(Self {
                pool,
                session,
                device,
                context,
                runtime_device: rt,
                item,
                size,
                latest: None,
                minimum_frame_time: 0,
            })
        }
    }
    pub fn invalidate(&mut self) -> Result<(), String> {
        self.latest = None;
        let mut counter = 0;
        let mut frequency = 0;
        unsafe {
            use windows_sys::Win32::System::Performance::{
                QueryPerformanceCounter, QueryPerformanceFrequency,
            };
            if QueryPerformanceCounter(&mut counter) == 0
                || QueryPerformanceFrequency(&mut frequency) == 0
                || frequency <= 0
            {
                return Err("无法校验画面时间，请重新连接".into());
            }
        }
        self.minimum_frame_time = (i128::from(counter) * 10_000_000 / i128::from(frequency)) as i64;
        Ok(())
    }
    pub fn read(
        &mut self,
        mut tick: impl FnMut() -> bool,
        wait: Duration,
    ) -> Result<&Frame, String> {
        // Pure observations can reuse a still-valid static frame. Every input
        // and control handoff invalidates it using the compositor's QPC clock.
        let start = Instant::now();
        let mut recreated = false;
        loop {
            if !tick() {
                return Err("控制操作已暂停或取消".into());
            }
            let mut newest: Option<windows::Graphics::Capture::Direct3D11CaptureFrame> = None;
            for _ in 0..8 {
                let Ok(frame) = self.pool.TryGetNextFrame() else {
                    break;
                };
                let size = frame
                    .ContentSize()
                    .map_err(|e| format!("无法读取画面尺寸：{e}"))?;
                if size.Width <= 0
                    || size.Height <= 0
                    || i64::from(size.Width) * i64::from(size.Height) > 16_000_000
                {
                    let _ = frame.Close();
                    return Err("本机画面尺寸无效".into());
                }
                if size != self.size {
                    let _ = frame.Close();
                    if let Some(old) = newest.take() {
                        let _ = old.Close();
                    }
                    self.latest = None;
                    self.pool
                        .Recreate(
                            &self.runtime_device,
                            DirectXPixelFormat::B8G8R8A8UIntNormalized,
                            2,
                            size,
                        )
                        .map_err(|e| format!("无法更新画面尺寸：{e}"))?;
                    self.size = size;
                } else if frame
                    .SystemRelativeTime()
                    .map_err(|e| format!("无法读取画面时间：{e}"))?
                    .Duration
                    >= self.minimum_frame_time
                {
                    if let Some(previous) = newest.replace(frame) {
                        let _ = previous.Close();
                    }
                } else {
                    let _ = frame.Close();
                }
            }
            if let Some(frame) = newest {
                let result = self.encode(&frame);
                let _ = frame.Close();
                self.latest = Some(result.map_err(|e| format!("无法读取本机画面：{e}"))?);
            }
            if self.latest.is_some() {
                return Ok(self.latest.as_ref().unwrap());
            }
            // An input can leave a window unchanged. Recreate once to request
            // a new static frame without returning a pre-input observation.
            if !recreated && start.elapsed() >= Duration::from_millis(150) {
                self.session
                    .Close()
                    .map_err(|e| format!("无法刷新捕获会话：{e}"))?;
                let _ = self.pool.Close();
                self.pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                    &self.runtime_device,
                    DirectXPixelFormat::B8G8R8A8UIntNormalized,
                    2,
                    self.size,
                )
                .map_err(|e| format!("无法刷新本机画面：{e}"))?;
                self.session = self
                    .pool
                    .CreateCaptureSession(&self.item)
                    .map_err(|e| format!("无法恢复捕获会话：{e}"))?;
                let _ = self.session.SetIsCursorCaptureEnabled(false);
                self.session
                    .StartCapture()
                    .map_err(|e| format!("无法启动捕获会话：{e}"))?;
                // A newly started session has no queued pre-input frames. Its
                // initial frame may carry the old compositor time when static.
                self.minimum_frame_time = 0;
                recreated = true;
            }
            if start.elapsed() >= wait {
                return Err("COMPUTER_USE_FRAME_TIMEOUT: 尚未收到操作后的画面，请重新 capture；此错误不代表操作未执行".into());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    fn encode(
        &self,
        frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
    ) -> windows::core::Result<Frame> {
        unsafe {
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let texture: ID3D11Texture2D = access.GetInterface()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let content = frame.ContentSize()?;
            if desc.Width == 0
                || desc.Height == 0
                || u64::from(desc.Width) * u64::from(desc.Height) > 16_000_000
                || content.Width <= 0
                || content.Height <= 0
                || content.Width as u32 > desc.Width
                || content.Height as u32 > desc.Height
            {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80070057u32 as i32,
                )));
            }
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut stage = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut stage))?;
            let stage = stage.unwrap();
            self.context.CopyResource(&stage, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&stage, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let content_width = content.Width as u32;
            let content_height = content.Height as u32;
            let mut rgb = vec![0u8; content_width as usize * content_height as usize * 3];
            for y in 0..content_height as usize {
                let row = std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    content_width as usize * 4,
                );
                for x in 0..content_width as usize {
                    let at = (y * content_width as usize + x) * 3;
                    rgb[at..at + 3].copy_from_slice(&[row[x * 4 + 2], row[x * 4 + 1], row[x * 4]]);
                }
            }
            self.context.Unmap(&stage, 0);
            let image = image::RgbImage::from_raw(content_width, content_height, rgb).unwrap();
            let scale = (640000.0 / (f64::from(content_width) * f64::from(content_height)))
                .sqrt()
                .min(1.0);
            let width = (f64::from(content_width) * scale).floor().max(1.0) as u32;
            let height = (f64::from(content_height) * scale).floor().max(1.0) as u32;
            let image = if width != content_width || height != content_height {
                image::imageops::resize(
                    &image,
                    width,
                    height,
                    image::imageops::FilterType::Triangle,
                )
            } else {
                image
            };
            let mut jpeg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
                .encode(&image, width, height, image::ExtendedColorType::Rgb8)
                .map_err(|_| {
                    windows::core::Error::from_hresult(windows::core::HRESULT(0x80004005u32 as i32))
                })?;
            Ok(Frame {
                width,
                height,
                jpeg,
            })
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
