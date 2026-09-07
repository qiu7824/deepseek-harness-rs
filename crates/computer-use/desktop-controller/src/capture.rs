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
    latest: Option<Frame>,
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
                latest: None,
            })
        }
    }
    pub fn read(
        &mut self,
        mut tick: impl FnMut() -> bool,
        wait: Duration,
    ) -> Result<&Frame, String> {
        // Each response must come from a newly delivered capture frame. In
        // particular, a manual screenshot must not survive a later handoff.
        self.latest = None;
        let start = Instant::now();
        loop {
            if !tick() {
                return Err("控制操作已暂停或取消".into());
            }
            if let Ok(frame) = self.pool.TryGetNextFrame() {
                {
                    let result = self.encode(&frame);
                    let _ = frame.Close();
                    let frame = result.map_err(|e| format!("无法读取本机画面：{e}"))?;
                    self.latest = Some(frame);
                }
            }
            if self.latest.is_some() {
                return Ok(self.latest.as_ref().unwrap());
            }
            if start.elapsed() >= wait {
                return Err("尚未收到本机桌面画面，请检查桌面是否锁定或已切换到安全界面".into());
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
            let mut stage = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut stage))?;
            let stage = stage.unwrap();
            self.context.CopyResource(&stage, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&stage, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
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
            self.context.Unmap(&stage, 0);
            let image = image::RgbImage::from_raw(desc.Width, desc.Height, rgb).unwrap();
            let scale = (640000.0 / (f64::from(desc.Width) * f64::from(desc.Height)))
                .sqrt()
                .min(1.0);
            let width = (f64::from(desc.Width) * scale).floor().max(1.0) as u32;
            let height = (f64::from(desc.Height) * scale).floor().max(1.0) as u32;
            let image = if width != desc.Width || height != desc.Height {
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
