//! Low-latency H.264 access units for the browser's native VideoDecoder.
use std::{
    mem::ManuallyDrop,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Media::MediaFoundation::*,
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
            Variant::VARIANT,
        },
    },
    core::{Interface, Result as WinResult},
};

pub struct Packet {
    pub bytes: Vec<u8>,
    pub key: bool,
    pub timestamp_us: u64,
}
pub struct Encoder {
    transform: IMFTransform,
    output: IMFMediaBuffer,
    header: Vec<u8>,
    nal_length: usize,
    pub width: u32,
    pub height: u32,
    pub codec: String,
    pub hardware: bool,
    pub hardware_failure: Option<String>,
    pub has_output: bool,
    events: Option<IMFMediaEventGenerator>,
    activation: Option<IMFActivate>,
    input_credits: u32,
    output_credits: u32,
    in_flight: u32,
    empty_output_wait: Duration,
    provides_samples: bool,
    nv12: Vec<u8>,
}
fn failure(error: windows::core::Error) -> String {
    format!("视频编码失败：{error}")
}
fn units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = if data[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if data[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            0
        };
        if n > 0 {
            starts.push((i, i + n));
            i += n
        } else {
            i += 1
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, (_, a))| &data[*a..starts.get(n + 1).map(|v| v.0).unwrap_or(data.len())])
        .filter(|n| !n.is_empty())
        .collect()
}
fn header_annex_b(data: &[u8]) -> Result<(Vec<u8>, usize), String> {
    if !units(data).is_empty() {
        return Ok((data.to_vec(), 4));
    }
    if data.len() < 7 || data[0] != 1 {
        return Err("H.264 序列头格式无效".into());
    }
    let length = (data[4] & 3) as usize + 1;
    let mut at = 6;
    let mut result = Vec::new();
    for count in [Some(data[5] & 31), None] {
        let count = if let Some(n) = count {
            n
        } else {
            let n = *data.get(at).ok_or("H.264 序列头被截断")?;
            at += 1;
            n
        };
        for _ in 0..count {
            let n = data.get(at..at + 2).ok_or("H.264 序列头被截断")?;
            let n = u16::from_be_bytes([n[0], n[1]]) as usize;
            at += 2;
            let bytes = data.get(at..at + n).ok_or("H.264 序列头被截断")?;
            result.extend([0, 0, 0, 1]);
            result.extend(bytes);
            at += n;
        }
    }
    Ok((result, length))
}
fn annex_b(data: &[u8], length: usize) -> Result<Vec<u8>, String> {
    if data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1]) {
        return Ok(data.to_vec());
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let mut n = 0usize;
        for b in data.get(at..at + length).ok_or("H.264 帧头不完整")? {
            n = n
                .checked_mul(256)
                .and_then(|n| n.checked_add(*b as usize))
                .ok_or("H.264 帧过大")?;
        }
        at += length;
        let bytes = data.get(at..at + n).ok_or("H.264 帧数据不完整")?;
        out.extend([0, 0, 0, 1]);
        out.extend(bytes);
        at += n;
    }
    Ok(out)
}
unsafe fn media_type(
    width: u32,
    height: u32,
    subtype: &windows::core::GUID,
) -> WinResult<IMFMediaType> {
    unsafe {
        let ty = MFCreateMediaType()?;
        ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        ty.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        ty.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
        ty.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
        ty.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        ty.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        ty.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        Ok(ty)
    }
}
fn hardware_encoders() -> WinResult<Vec<IMFActivate>> {
    unsafe {
        let output = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        let mut items = std::ptr::null_mut();
        let mut count = 0;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&output),
            &mut items,
            &mut count,
        )?;
        let mut result = Vec::new();
        if !items.is_null() {
            for item in std::slice::from_raw_parts_mut(items, count as usize) {
                if let Some(activation) = item.take() {
                    result.push(activation)
                }
            }
            CoTaskMemFree(Some(items.cast()));
        }
        Ok(result)
    }
}
impl Encoder {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let mut failure = None;
        for activation in hardware_encoders().unwrap_or_default().into_iter().take(8) {
            match unsafe { activation.ActivateObject::<IMFTransform>() }
                .map_err(super::video::failure)
                .and_then(|transform| {
                    Self::create(width, height, transform, Some(activation.clone()))
                }) {
                Ok(encoder) => return Ok(encoder),
                Err(error) => {
                    failure = Some(error);
                    let _ = unsafe { activation.ShutdownObject() };
                }
            }
        }
        let mut encoder = Self::software(width, height)?;
        encoder.hardware_failure = failure;
        Ok(encoder)
    }
    pub fn software(width: u32, height: u32) -> Result<Self, String> {
        let transform = unsafe { CoCreateInstance(&CMSH264EncoderMFT, None, CLSCTX_INPROC_SERVER) }
            .map_err(failure)?;
        Self::create(width, height, transform, None)
    }
    fn create(
        width: u32,
        height: u32,
        transform: IMFTransform,
        activation: Option<IMFActivate>,
    ) -> Result<Self, String> {
        if width < 2
            || height < 2
            || width % 2 != 0
            || height % 2 != 0
            || u64::from(width) * u64::from(height) > 8_294_400
        {
            return Err("视频尺寸不受支持".into());
        }
        let result = (|| -> WinResult<Self> {
            unsafe {
                let hardware = activation.is_some();
                let events = if hardware {
                    transform
                        .GetAttributes()?
                        .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                    Some(transform.cast::<IMFMediaEventGenerator>()?)
                } else {
                    None
                };
                let codec_api: ICodecAPI = transform.cast()?;
                codec_api.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true))?;
                let _ = codec_api.SetValue(&CODECAPI_AVEncCommonRealTime, &VARIANT::from(true));
                let _ = codec_api.SetValue(&CODECAPI_AVEncMPVGOPSize, &VARIANT::from(30u32));
                let _ = codec_api
                    .SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &VARIANT::from(0u32));
                let _ = codec_api.SetValue(&CODECAPI_AVEncNumWorkerThreads, &VARIANT::from(4u32));
                let output_type = media_type(width, height, &MFVideoFormat_H264)?;
                output_type.SetUINT32(
                    &MF_MT_AVG_BITRATE,
                    (width * height * 6).clamp(2_000_000, 30_000_000),
                )?;
                output_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
                transform.SetOutputType(0, &output_type, 0)?;
                let input = media_type(width, height, &MFVideoFormat_NV12)?;
                input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
                input.SetUINT32(&MF_MT_SAMPLE_SIZE, width * height * 3 / 2)?;
                transform.SetInputType(0, &input, 0)?;
                let info = transform.GetOutputStreamInfo(0)?;
                let output =
                    MFCreateMemoryBuffer(info.cbSize.max(1024 * 1024).min(8 * 1024 * 1024))?;
                transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
                Ok(Self {
                    transform,
                    output,
                    header: Vec::new(),
                    nal_length: 4,
                    width,
                    height,
                    codec: String::new(),
                    hardware,
                    hardware_failure: None,
                    has_output: false,
                    events,
                    activation,
                    input_credits: 0,
                    output_credits: 0,
                    in_flight: 0,
                    empty_output_wait: Duration::ZERO,
                    provides_samples: info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                        != 0,
                    nv12: vec![0; width as usize * height as usize * 3 / 2],
                })
            }
        })();
        let mut encoder = result.map_err(failure)?;
        encoder.refresh_header()?;
        Ok(encoder)
    }
    fn refresh_header(&mut self) -> Result<(), String> {
        unsafe {
            let ty = self.transform.GetOutputCurrentType(0).map_err(failure)?;
            if let Ok(size) = ty.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) {
                if size > 65536 {
                    return Err("H.264 序列头过大".into());
                }
                if size > 0 {
                    let mut header = vec![0; size as usize];
                    ty.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut header, None)
                        .map_err(failure)?;
                    (self.header, self.nal_length) = header_annex_b(&header)?;
                    self.update_codec();
                }
            }
            Ok(())
        }
    }
    fn update_codec(&mut self) {
        if let Some(sps) = units(&self.header)
            .into_iter()
            .find(|n| n[0] & 31 == 7 && n.len() >= 4)
        {
            self.codec = format!("avc1.{:02X}{:02X}{:02X}", sps[1], sps[2], sps[3]);
        }
    }
    fn poll_events(&mut self) -> WinResult<()> {
        unsafe {
            if let Some(events) = &self.events {
                for _ in 0..64 {
                    let event = match events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                        Ok(event) => event,
                        Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                        Err(error) => return Err(error),
                    };
                    event.GetStatus()?.ok()?;
                    match event.GetType()? {
                        kind if kind == METransformNeedInput.0 as u32 => {
                            self.input_credits = self.input_credits.saturating_add(1).min(16)
                        }
                        kind if kind == METransformHaveOutput.0 as u32 => {
                            self.output_credits = self.output_credits.saturating_add(1).min(16)
                        }
                        _ => {}
                    }
                }
            }
            Ok(())
        }
    }
    pub fn encode(
        &mut self,
        image: &image::RgbImage,
        timestamp_us: u64,
    ) -> Result<Option<Packet>, String> {
        if image.width() < self.width || image.height() < self.height {
            return Err("视频尺寸已经变化".into());
        }
        self.poll_events().map_err(failure)?;
        if self.hardware
            && self.output_credits == 0
            && self.empty_output_wait > Duration::from_secs(2)
        {
            return Err("硬件视频编码响应超时".into());
        }
        let result = (|| -> WinResult<Option<(Vec<u8>, u64)>> {
            unsafe {
                if !self.hardware || (self.input_credits > 0 && self.in_flight < 2) {
                    rgb_to_nv12(image, self.width, self.height, &mut self.nv12);
                    let buffer = MFCreateMemoryBuffer(self.nv12.len() as u32)?;
                    let mut ptr = std::ptr::null_mut();
                    buffer.Lock(&mut ptr, None, None)?;
                    std::ptr::copy_nonoverlapping(self.nv12.as_ptr(), ptr, self.nv12.len());
                    buffer.Unlock()?;
                    buffer.SetCurrentLength(self.nv12.len() as u32)?;
                    let sample = MFCreateSample()?;
                    sample.AddBuffer(&buffer)?;
                    sample.SetSampleTime(timestamp_us as i64 * 10)?;
                    sample.SetSampleDuration(10_000_000 / 30)?;
                    self.transform.ProcessInput(0, &sample, 0)?;
                    if self.hardware {
                        self.input_credits -= 1;
                        self.in_flight += 1;
                    }
                }
                if self.hardware {
                    let wait_started = Instant::now();
                    let deadline = wait_started + Duration::from_millis(5);
                    loop {
                        self.poll_events()?;
                        if self.output_credits > 0 {
                            self.output_credits -= 1;
                            break;
                        }
                        if Instant::now() >= deadline {
                            self.empty_output_wait +=
                                wait_started.elapsed().min(Duration::from_millis(10));
                            return Ok(None);
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                self.output.SetCurrentLength(0)?;
                let out = if self.provides_samples {
                    None
                } else {
                    let out = MFCreateSample()?;
                    out.AddBuffer(&self.output)?;
                    Some(out)
                };
                let mut data = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(out),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0;
                let result = self.transform.ProcessOutput(0, &mut data, &mut status);
                let out = ManuallyDrop::take(&mut data[0].pSample);
                drop(ManuallyDrop::take(&mut data[0].pEvents));
                if result
                    .as_ref()
                    .is_err_and(|e| e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT)
                {
                    return Ok(None);
                }
                result?;
                self.in_flight = self.in_flight.saturating_sub(1);
                let out = out.ok_or_else(|| {
                    windows::core::Error::from_hresult(windows::core::HRESULT(0x80004005u32 as i32))
                })?;
                let pts = out
                    .GetSampleTime()
                    .unwrap_or(timestamp_us as i64 * 10)
                    .max(0) as u64
                    / 10;
                let buffer = out.ConvertToContiguousBuffer()?;
                let mut ptr = std::ptr::null_mut();
                let mut length = 0;
                buffer.Lock(&mut ptr, None, Some(&mut length))?;
                if length > 8 * 1024 * 1024 {
                    buffer.Unlock()?;
                    return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                        0x80070057u32 as i32,
                    )));
                }
                let bytes = std::slice::from_raw_parts(ptr, length as usize).to_vec();
                buffer.Unlock()?;
                Ok(Some((bytes, pts)))
            }
        })();
        let Some((bytes, pts)) = result.map_err(failure)? else {
            return Ok(None);
        };
        let mut bytes = annex_b(&bytes, self.nal_length)?;
        let nals = units(&bytes);
        let key = nals.iter().any(|n| n[0] & 31 == 5);
        if nals.iter().any(|n| n[0] & 31 == 7) {
            self.header.clear();
            for nal in nals.iter().filter(|n| matches!(n[0] & 31, 7 | 8)) {
                self.header.extend([0, 0, 0, 1]);
                self.header.extend_from_slice(nal);
            }
            self.update_codec();
        } else if key {
            if self.header.is_empty() {
                self.refresh_header()?;
            }
            let mut prefix = self.header.clone();
            prefix.append(&mut bytes);
            bytes = prefix;
        }
        if self.codec.is_empty() {
            return Err("H.264 缺少 SPS".into());
        }
        self.has_output = true;
        self.empty_output_wait = Duration::ZERO;
        Ok(Some(Packet {
            bytes,
            key,
            timestamp_us: pts,
        }))
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            if let Some(activation) = &self.activation {
                let _ = activation.ShutdownObject();
            } else if let Ok(shutdown) = self.transform.cast::<IMFShutdown>() {
                let _ = shutdown.Shutdown();
            }
        }
    }
}
fn rgb_to_nv12(image: &image::RgbImage, width: u32, height: u32, out: &mut [u8]) {
    let width = width as usize;
    let height = height as usize;
    let stride = image.width() as usize;
    let rgb = image.as_raw();
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let (mut rsum, mut gsum, mut bsum) = (0i32, 0i32, 0i32);
            for dy in 0..2 {
                for dx in 0..2 {
                    let at = ((y + dy) * stride + x + dx) * 3;
                    let (r, g, b) = (rgb[at] as i32, rgb[at + 1] as i32, rgb[at + 2] as i32);
                    out[(y + dy) * width + x + dx] =
                        (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
                    rsum += r;
                    gsum += g;
                    bsum += b;
                }
            }
            let (r, g, b) = (rsum / 4, gsum / 4, bsum / 4);
            let uv = width * height + (y / 2) * width + x;
            out[uv] = (((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            out[uv + 1] = (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodes_a_real_keyframe() {
        let _platform = crate::platform::MediaPlatform::new().unwrap();
        let mut encoder = Encoder::new(320, 180).unwrap();
        let image = image::RgbImage::from_fn(320, 180, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let mut found = false;
        println!(
            "encoder hardware={} fallback={:?}",
            encoder.hardware, encoder.hardware_failure
        );
        for i in 0..60 {
            std::thread::sleep(Duration::from_millis(5));
            if let Some(packet) = encoder.encode(&image, i * 33333).unwrap() {
                assert!(packet.key);
                assert!(units(&packet.bytes).iter().any(|n| n[0] & 31 == 7));
                found = true;
                break;
            }
        }
        assert!(found);
    }
}
