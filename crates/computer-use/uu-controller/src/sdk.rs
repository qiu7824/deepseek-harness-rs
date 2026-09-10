//! Export-discovered controller ABI and bounded event delivery.

use serde_json::{Value, json};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;

use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::*;

fn module_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("DSH_UU_INSTALL_DIR").unwrap_or_default())
        .join("streamer.dll")
}

fn sdk_path(path: &std::path::Path) -> Result<Vec<u16>, String> {
    // This SDK's path parser corrupts its heap on Win32 verbatim prefixes.
    // Preserve the absolute target using the equivalent ordinary Win32 form.
    let path = path.to_str().ok_or("UU 路径编码不可用")?;
    let ordinary = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        path.to_owned()
    };
    let wide: Vec<u16> = ordinary.encode_utf16().collect();
    if wide.contains(&0) || wide.len() >= 240 {
        return Err("UU 控制引擎不支持该路径格式或长度，请使用较短的安装和缓存路径".into());
    }
    Ok(wide.into_iter().chain(Some(0)).collect())
}

const MAX_BYTES: usize = 65_536;
const WAIT: Duration = Duration::from_secs(15);

#[repr(u8)]
#[derive(Clone, Copy)]
pub(crate) enum PauseReason {
    Agent = 0,
    StartHuman = 1,
    GuiInput = 2,
    GuiTakeover = 3,
    ResumePending = 4,
    EscapeHotkey = 5,
    ReaderShutdown = 6,
    ReleaseFailed = 7,
    EmergencyStopUnavailable = 8,
}
static PAUSE_REASON: AtomicU8 = AtomicU8::new(PauseReason::Agent as u8);

pub(crate) fn set_pause_state(paused: &AtomicBool, reason: PauseReason) {
    PAUSE_REASON.store(reason as u8, Ordering::SeqCst);
    paused.store(!matches!(reason, PauseReason::Agent), Ordering::SeqCst);
}

fn pause_reason_name(reason: u8) -> &'static str {
    match reason {
        1 => "start-human",
        2 => "gui-input",
        3 => "gui-takeover",
        4 => "resume-pending",
        5 => "escape-hotkey",
        6 => "reader-shutdown",
        7 => "release-failed",
        8 => "emergency-stop-unavailable",
        _ => "agent",
    }
}

fn pause_diagnostics() -> Value {
    json!({"pauseReason":pause_reason_name(PAUSE_REASON.load(Ordering::SeqCst))})
}

fn paused_error_for(reason: u8) -> String {
    if reason == PauseReason::ReaderShutdown as u8 {
        "COMPUTER_USE_DESKTOP_DISCONNECTED".into()
    } else {
        format!(
            "COMPUTER_USE_MANUAL_CONTROL; controlDiagnostics={}",
            json!({"pauseReason":pause_reason_name(reason)})
        )
    }
}

fn paused_error() -> String {
    paused_error_for(PAUSE_REASON.load(Ordering::SeqCst))
}

fn finish_agent_resume(paused: &AtomicBool, reason: &AtomicU8) -> bool {
    if reason
        .compare_exchange(
            PauseReason::ResumePending as u8,
            PauseReason::Agent as u8,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return false;
    }
    paused.store(false, Ordering::SeqCst);
    if reason.load(Ordering::SeqCst) != PauseReason::Agent as u8 {
        paused.store(true, Ordering::SeqCst);
        return false;
    }
    true
}

static EVENTS: Mutex<Option<SyncSender<Event>>> = Mutex::new(None);
static DROPPED_EVENTS: AtomicUsize = AtomicUsize::new(0);
static CONTROL_SENDER: AtomicUsize = AtomicUsize::new(0);
static FRAME_CHANGES: AtomicUsize = AtomicUsize::new(0);
#[derive(Clone, Copy)]
struct RenderViewport {
    session: u32,
    track: i32,
    format: i32,
    rotation: i32,
    source: i32,
    ready: crate::render::FrameViewport,
}
static FRAME_VIEWPORT: Mutex<Option<RenderViewport>> = Mutex::new(None);
extern "system" fn on_frame_change(
    session: u32,
    format: i32,
    rotation: i32,
    source: i32,
    track: i32,
    rect: *const f32,
) {
    let _ = std::panic::catch_unwind(|| {
        FRAME_CHANGES.fetch_add(1, Ordering::Relaxed);
        if !rect.is_null() {
            let values = unsafe { std::slice::from_raw_parts(rect, 6) };
            if values.iter().all(|v| v.is_finite())
                && values[0] >= 0.0
                && values[1] >= 0.0
                && values[2] >= 1.0
                && values[3] >= 1.0
            {
                if let Ok(mut viewport) = FRAME_VIEWPORT.lock() {
                    let rect = [values[0], values[1], values[2], values[3]];
                    let unchanged = viewport.as_ref().is_some_and(|old| {
                        old.session == session
                            && old.track == track
                            && old.format == format
                            && old.rotation == rotation
                            && old.source == source
                            && old.ready.rect == rect
                    });
                    // Repeated notifications do not move the boundary ahead
                    // of a static frame already presented by the compositor.
                    if !unchanged {
                        *viewport =
                            crate::render::frame_clock()
                                .ok()
                                .map(|notified_at| RenderViewport {
                                    session,
                                    track,
                                    format,
                                    rotation,
                                    source,
                                    ready: crate::render::FrameViewport { rect, notified_at },
                                });
                    }
                }
            }
            send_event(Event::Layout(
                session,
                json!({"formatCode":format,"rotationCode":rotation,"sourceId":source,"track":track,"rect":values}),
            ));
        }
    });
}
static INVALID_MEDIA: AtomicUsize = AtomicUsize::new(0);

type Exchange = unsafe extern "system" fn(*const usize) -> *const usize;
type Init = unsafe extern "system" fn(*const u8, *const u8, *const u8) -> i32;
type Release = unsafe extern "system" fn() -> i32;
type CreateConnection = unsafe extern "system" fn(*const LoginRoom) -> i32;
type ExitRoom = unsafe extern "system" fn(i32) -> i32;
type Connect = unsafe extern "system" fn(i32, *const ConnectParams) -> i32;
type DecodeCapability = unsafe extern "system" fn(*mut OutBytes) -> i32;
type Version = unsafe extern "system" fn(*mut *const u8, *mut u32) -> i32;

#[repr(C)]
struct ProxyConfig {
    proxy_type: i32,
    pad0: u32,
    host: *const u8,
    host_len: i32,
    port: u16,
    pad1: u16,
    username: *const u8,
    username_len: i32,
    pad2: u32,
    password: *const u8,
    password_len: i32,
    pad3: u32,
}

#[repr(C)]
struct LoginRoom {
    token: *const u8,
    token_len: i32,
    pad0: u32,
    signaling: *const *const u8,
    signaling_lengths: *const i32,
    signaling_count: i32,
    ws_connect_timeout_ms: i32,
    streamer_retry_delta_ms: i32,
    pad1: u32,
    report_token: *const u8,
    report_token_len: i32,
    pad2: u32,
    report_url: *const u8,
    report_url_len: i32,
    pad3: u32,
    report_server_address: *const u8,
    report_server_address_len: i32,
    reserved: i32,
    proxy: ProxyConfig,
}

#[repr(C)]
struct ConnectParams {
    options: *const u8,
    options_len: i32,
    pad0: u32,
    source_id: *const u8,
    source_id_len: i32,
    pad1: u32,
}

#[repr(C)]
struct OutBytes {
    data: *const u8,
    len: u32,
    pad: u32,
}

const _: () = {
    assert!(size_of::<usize>() == 8);
    assert!(size_of::<ProxyConfig>() == 0x38);
    assert!(size_of::<LoginRoom>() == 0x98);
    assert!(offset_of!(LoginRoom, token_len) == 0x08);
    assert!(offset_of!(LoginRoom, signaling) == 0x10);
    assert!(offset_of!(LoginRoom, signaling_lengths) == 0x18);
    assert!(offset_of!(LoginRoom, signaling_count) == 0x20);
    assert!(offset_of!(LoginRoom, ws_connect_timeout_ms) == 0x24);
    assert!(offset_of!(LoginRoom, streamer_retry_delta_ms) == 0x28);
    assert!(offset_of!(LoginRoom, report_token) == 0x30);
    assert!(offset_of!(LoginRoom, report_token_len) == 0x38);
    assert!(offset_of!(LoginRoom, report_url) == 0x40);
    assert!(offset_of!(LoginRoom, report_url_len) == 0x48);
    assert!(offset_of!(LoginRoom, report_server_address) == 0x50);
    assert!(offset_of!(LoginRoom, report_server_address_len) == 0x58);
    assert!(offset_of!(LoginRoom, reserved) == 0x5c);
    assert!(offset_of!(LoginRoom, proxy) == 0x60);
    assert!(offset_of!(ProxyConfig, host) == 0x08);
    assert!(offset_of!(ProxyConfig, host_len) == 0x10);
    assert!(offset_of!(ProxyConfig, port) == 0x14);
    assert!(offset_of!(ProxyConfig, username) == 0x18);
    assert!(offset_of!(ProxyConfig, username_len) == 0x20);
    assert!(offset_of!(ProxyConfig, password) == 0x28);
    assert!(offset_of!(ProxyConfig, password_len) == 0x30);
    assert!(size_of::<ConnectParams>() == 0x20);
    assert!(offset_of!(ConnectParams, source_id) == 0x10);
    assert!(offset_of!(ConnectParams, source_id_len) == 0x18);
    assert!(size_of::<OutBytes>() == 0x10);
    assert!(offset_of!(OutBytes, len) == 0x08);
};

enum Event {
    Room(u32, i32),
    Code(u32, i32),
    Media(u32, Value),
    Data(u32, Value),
    Echo(u32, Vec<u8>),
    Layout(u32, Value),
}

fn send_event(event: Event) {
    if let Ok(guard) = EVENTS.try_lock() {
        if let Some(sender) = guard.as_ref() {
            if sender.try_send(event).is_err() {
                DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
            }
        }
    } else {
        DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
    }
}

extern "system" fn on_room(session: u32, state: i32) {
    let _ = std::panic::catch_unwind(|| send_event(Event::Room(session, state)));
}

extern "system" fn on_code(session: u32, code: i32) {
    let _ = std::panic::catch_unwind(|| send_event(Event::Code(session, code)));
}

fn receive_data(kind: &str, session: u32, data: *const u8, len: u32) {
    if !data.is_null() && len > 0 && len <= 65536 {
        let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
        if kind == "control" {
            if let Some(reply) = crate::wire::echo_reply(bytes) {
                send_event(Event::Echo(session, reply));
            }
        }
        let value = crate::wire::metadata(kind, bytes);
        send_event(Event::Data(session, value));
    }
}
extern "system" fn on_control(session: u32, data: *const u8, len: u32) {
    let _ = std::panic::catch_unwind(|| receive_data("control", session, data, len));
}
extern "system" fn on_text(session: u32, data: *const u8, len: u32) {
    let _ = std::panic::catch_unwind(|| receive_data("text", session, data, len));
}
extern "system" fn on_binary(session: u32, data: *const u8, len: u32) {
    let _ = std::panic::catch_unwind(|| receive_data("binary", session, data, len));
}
extern "system" fn on_qos(session: u32, data: *const u8, len: u32) {
    let _ = std::panic::catch_unwind(|| {
        if data.is_null() || len > 65536 {
            return;
        }
        if let Ok(value) = serde_json::from_slice::<Value>(unsafe {
            std::slice::from_raw_parts(data, len as usize)
        }) {
            fn collect(
                v: &Value,
                path: &str,
                out: &mut serde_json::Map<String, Value>,
                depth: usize,
            ) {
                if depth > 5 || out.len() > 70 {
                    return;
                }
                match v {
                    Value::Object(items) => {
                        for (k, v) in items {
                            collect(v, &format!("{path}.{k}"), out, depth + 1)
                        }
                    }
                    Value::Array(items) => {
                        for (i, v) in items.iter().take(6).enumerate() {
                            collect(v, &format!("{path}.{i}"), out, depth + 1)
                        }
                    }
                    Value::Number(_) | Value::Bool(_) => {
                        out.insert(path.to_string(), v.clone());
                    }
                    _ => {}
                }
            }
            let mut stats = serde_json::Map::new();
            collect(&value, "qos", &mut stats, 0);
            stats.insert(
                "mediaStreamCount".into(),
                json!(value["media_streams"].as_array().map(Vec::len).unwrap_or(0)),
            );
            send_event(Event::Data(session, Value::Object(stats)));
        }
    });
}
fn media_scalar(value: Option<&Value>, max_text: usize) -> Option<Value> {
    match value? {
        Value::String(text) if text.len() <= max_text => Some(Value::String(text.clone())),
        Value::Number(number) if number.as_i64().is_some() || number.as_u64().is_some() => {
            Some(Value::Number(number.clone()))
        }
        _ => None,
    }
}

extern "system" fn on_media(session: u32, data: *const u8, len: u32) {
    let result = std::panic::catch_unwind(|| {
        if data.is_null() || len == 0 || len as usize > MAX_BYTES {
            return false;
        }
        // The SDK owns this pointer for the duration of the callback.
        let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return false;
        };
        let Some(kind) = media_scalar(value.get("type"), 128) else {
            return false;
        };
        let Some(track_id) = media_scalar(value.get("track_id"), 1024) else {
            return false;
        };
        let Some(index) = value.get("track_index").and_then(Value::as_i64) else {
            return false;
        };
        if i32::try_from(index).is_err() {
            return false;
        }
        let type_recognized = matches!(kind.as_str(), Some("video" | "audio"));
        // Only documented track metadata is returned; arbitrary callback text is not logged.
        // Integer/unknown types are preserved as observations, never treated as video.
        send_event(Event::Media(
            session,
            json!({
                "type": kind, "track_id": track_id, "track_index": index,
                "type_recognized": type_recognized
            }),
        ));
        true
    });
    if !matches!(result, Ok(true)) {
        INVALID_MEDIA.fetch_add(1, Ordering::Relaxed);
    }
}

struct CallbackRegistration;

impl CallbackRegistration {
    fn install(sender: SyncSender<Event>) -> Result<Self, String> {
        let mut events = EVENTS.lock().map_err(|_| "Callback registry is poisoned")?;
        if events.is_some() {
            return Err("A controller callback registry is already active".into());
        }
        *events = Some(sender);
        DROPPED_EVENTS.store(0, Ordering::Relaxed);
        INVALID_MEDIA.store(0, Ordering::Relaxed);
        FRAME_CHANGES.store(0, Ordering::Relaxed);
        *FRAME_VIEWPORT
            .lock()
            .map_err(|_| "Viewport registry is poisoned")? = None;
        Ok(Self)
    }
}

impl Drop for CallbackRegistration {
    fn drop(&mut self) {
        // Release completes SDK workers before this registry is removed.
        let mut events = EVENTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *events = None;
    }
}

struct Sdk {
    module: HMODULE,
    functions: [usize; 21],
    init_attempted: bool,
    session: Option<i32>,
    log_directory: Option<std::path::PathBuf>,
}

impl Sdk {
    fn load() -> Result<Self, String> {
        let module = module_path();
        let path = module.as_path();
        let bytes = std::fs::read(path).map_err(|e| format!("Read SDK: {e}"))?;
        let get_u32 = |offset: usize| -> Result<u32, String> {
            let end = offset.checked_add(4).ok_or("Invalid PE offset")?;
            let chunk: [u8; 4] = bytes
                .get(offset..end)
                .ok_or("Truncated PE")?
                .try_into()
                .map_err(|_| "Truncated PE integer")?;
            Ok(u32::from_le_bytes(chunk))
        };
        if bytes.get(..2) != Some(b"MZ") {
            return Err("Invalid SDK PE signature".into());
        }
        let pe = get_u32(0x3c)? as usize;
        if bytes.get(pe..pe + 4) != Some(b"PE\0\0")
            || bytes.get(pe + 4..pe + 6) != Some(&[0x64, 0x86])
        {
            return Err("UU 控制引擎必须为受支持的 x64 版本".into());
        }
        let image_size = get_u32(pe + 24 + 56)? as usize;
        if !(0x10000..=0x10000000).contains(&image_size) {
            return Err("Unexpected SDK image size".into());
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let module = unsafe {
            LoadLibraryExW(
                wide.as_ptr(),
                ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        };
        if module.is_null() {
            return Err(format!(
                "LoadLibraryExW: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut sdk = Self {
            module,
            functions: [0; 21],
            init_attempted: false,
            session: None,
            log_directory: None,
        };
        let base = module as usize;
        let image_end = base.checked_add(image_size).ok_or("SDK address overflow")?;
        let export =
            unsafe { GetProcAddress(module, c"ExchangeControllerInterface".as_ptr().cast()) }
                .ok_or("Missing controller interface")?;
        let exchange: Exchange = unsafe { std::mem::transmute(export) };
        let mut callbacks = [0usize; 18];
        callbacks[0] = on_media as *const () as usize;
        callbacks[2] = on_frame_change as *const () as usize;
        callbacks[3] = on_control as *const () as usize;
        callbacks[4] = on_text as *const () as usize;
        callbacks[6] = on_binary as *const () as usize;
        callbacks[9] = on_qos as *const () as usize;
        callbacks[7] = on_room as *const () as usize;
        callbacks[12] = on_code as *const () as usize;
        let functions = unsafe { exchange(callbacks.as_ptr()) };
        let address = functions as usize;
        if address < base
            || address
                .checked_add(21 * 8)
                .is_none_or(|end| end > image_end)
        {
            return Err("Controller function table ABI mismatch".into());
        }
        let table = unsafe { std::slice::from_raw_parts(functions, 21) };
        if table
            .iter()
            .any(|&entry| entry < base || entry >= image_end)
        {
            return Err("Controller function outside SDK image".into());
        }
        sdk.functions.copy_from_slice(table);
        CONTROL_SENDER.store(table[10], Ordering::SeqCst);
        Ok(sdk)
    }

    fn version(&self) -> Result<String, String> {
        let version: Version = unsafe { std::mem::transmute(self.functions[17]) };
        let mut data = ptr::null();
        let mut len = 0;
        let status = unsafe { version(&mut data, &mut len) };
        if status != 0 || data.is_null() || len == 0 || len > 64 {
            return Err(format!("SDK version query failed (status {status})"));
        }
        let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
        let text = std::str::from_utf8(bytes).map_err(|_| "SDK version is not UTF-8")?;
        Ok(text.to_owned())
    }

    fn init(&mut self) -> Result<i32, String> {
        let root = std::env::current_dir().map_err(|_| "无法确定控制进程缓存目录")?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "本机时间无效")?
            .as_nanos();
        let logs = root.join(format!("engine-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&logs).map_err(|_| "无法创建控制引擎日志目录")?;
        self.log_directory = Some(logs.clone());
        let config = module_path().with_file_name("streamer-config.json");
        let log_path = sdk_path(&logs)?;
        let config_path = sdk_path(&config)?;
        let suffix: Vec<u16> = format!("harness-{}", std::process::id())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let init: Init = unsafe { std::mem::transmute(self.functions[0]) };
        self.init_attempted = true;
        Ok(unsafe {
            init(
                log_path.as_ptr().cast(),
                config_path.as_ptr().cast(),
                suffix.as_ptr().cast(),
            )
        })
    }

    fn decoder_caps(&self) -> Result<(i32, Vec<Decoder>), String> {
        let getter: DecodeCapability = unsafe { std::mem::transmute(self.functions[15]) };
        let mut out = OutBytes {
            data: ptr::null(),
            len: 0,
            pad: 0,
        };
        let status = unsafe { getter(&mut out) };
        if status != 0 || out.data.is_null() || out.len == 0 || out.len > 1_048_576 {
            return Err(format!(
                "Decoder capability unavailable (status {status}, length {})",
                out.len
            ));
        }
        // Copy/parse before Release; the SDK owns this memory, so it is never freed here.
        let bytes = unsafe { std::slice::from_raw_parts(out.data, out.len as usize) };
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| "Decoder capability is not valid JSON")?;
        let h264 = value
            .get("H264")
            .and_then(Value::as_array)
            .ok_or("Decoder capability lacks the required H264 array")?;
        let h265 = value.get("H265").and_then(Value::as_array);
        let mut caps = Vec::new();
        for (codec, entries) in [("CodecType_H264", Some(h264)), ("CodecType_H265", h265)] {
            if let Some(entries) = entries {
                if entries.len() > 256 {
                    return Err("Decoder capability array exceeds bound".into());
                }
                for entry in entries {
                    let width = entry.get("width").and_then(Value::as_i64).unwrap_or(0);
                    let height = entry.get("height").and_then(Value::as_i64).unwrap_or(0);
                    if width <= 0
                        || height <= 0
                        || width > i32::MAX as i64
                        || height > i32::MAX as i64
                    {
                        continue;
                    }
                    let chroma = match entry.get("chroma_sampling").and_then(Value::as_i64) {
                        Some(1) => "ChromaFormat_420",
                        Some(2) => "ChromaFormat_422",
                        Some(3) => "ChromaFormat_444",
                        Some(4) => "ChromaFormat_400",
                        _ => "ChromaFormat_UNKNOWN",
                    };
                    caps.push(Decoder {
                        codec,
                        width: width as i32,
                        height: height as i32,
                        chroma,
                    });
                }
            }
        }
        Ok((status, caps))
    }

    fn shutdown(&mut self) -> Value {
        CONTROL_SENDER.store(0, Ordering::SeqCst);
        let exit_status = self.session.take().map(|session| {
            let exit: ExitRoom = unsafe { std::mem::transmute(self.functions[3]) };
            unsafe { exit(session) }
        });
        let release_status = if self.init_attempted {
            self.init_attempted = false;
            let release: Release = unsafe { std::mem::transmute(self.functions[1]) };
            Some(unsafe { release() })
        } else {
            None
        };
        // A failed Release may leave SDK workers alive; unloading their code would
        // be unsafe. In that exceptional case retain the reference until process exit.
        let unload_deferred = release_status.is_some_and(|status| status != 0);
        let free_status = if !self.module.is_null() && !unload_deferred {
            let module = std::mem::replace(&mut self.module, ptr::null_mut());
            Some(unsafe { FreeLibrary(module) } != 0)
        } else {
            None
        };
        if unload_deferred {
            self.module = ptr::null_mut();
        }
        if !unload_deferred {
            if let Some(logs) = self.log_directory.take() {
                // Remove only flat log files inside the directory this SDK
                // instance created. Never traverse vendor-created links.
                if std::fs::symlink_metadata(&logs)
                    .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                {
                    if let Ok(entries) = std::fs::read_dir(&logs) {
                        for entry in entries.flatten().take(128) {
                            if entry.file_type().is_ok_and(|t| t.is_file()) {
                                let _ = std::fs::remove_file(entry.path());
                            }
                        }
                    }
                    let _ = std::fs::remove_dir(logs);
                }
            }
        }
        json!({"exit_room_status": exit_status, "release_status": release_status,
            "library_freed": free_status, "unload_deferred_after_release_failure": unload_deferred})
    }
}

impl Drop for Sdk {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub fn check_engine() -> Result<Value, String> {
    eprintln!("DSH_ENGINE_CHECK=load");
    let mut sdk = Sdk::load()?;
    let version = sdk.version()?;
    eprintln!("DSH_ENGINE_CHECK=init");
    let init_status = sdk.init()?;
    eprintln!("DSH_ENGINE_CHECK=release");
    let teardown = sdk.shutdown();
    Ok(json!({"version":version,"initStatus":init_status,"teardown":teardown}))
}

struct Decoder {
    codec: &'static str,
    width: i32,
    height: i32,
    chroma: &'static str,
}

// Protocol schemas define field numbers, scalar types and enum values.
struct Schema {
    connect: Value,
    defines: Value,
}

impl Schema {
    fn load() -> Result<Self, String> {
        Ok(Self {
            connect: serde_json::from_str(include_str!("schemas/connect.json"))
                .map_err(|_| "Invalid extracted connect descriptor")?,
            defines: serde_json::from_str(include_str!("schemas/defines.json"))
                .map_err(|_| "Invalid extracted defines descriptor")?,
        })
    }

    fn message(&self, name: &str) -> Result<&Value, String> {
        self.connect["messageType"]
            .as_array()
            .and_then(|items| items.iter().find(|m| m["name"] == name))
            .ok_or_else(|| format!("Descriptor message missing: {name}"))
    }

    fn number(&self, message: &str, name: &str, kind: &str) -> Result<u32, String> {
        let field = self.message(message)?["field"]
            .as_array()
            .and_then(|fields| fields.iter().find(|f| f["name"] == name))
            .ok_or_else(|| format!("Descriptor field missing: {message}.{name}"))?;
        if field["type"] != kind {
            return Err(format!("Descriptor type mismatch: {message}.{name}"));
        }
        let number = field["number"]
            .as_u64()
            .ok_or("Invalid descriptor field number")?;
        if number == 0 || number >= (1 << 29) {
            return Err("Descriptor field number out of range".into());
        }
        Ok(number as u32)
    }

    fn enum_value(&self, enum_name: &str, name: &str) -> Result<i32, String> {
        let enums = if enum_name == "Type" {
            &self.message("ConnectOptions")?["enumType"]
        } else {
            &self.defines["enumType"]
        };
        let value = enums
            .as_array()
            .and_then(|items| items.iter().find(|e| e["name"] == enum_name))
            .and_then(|e| e["value"].as_array())
            .and_then(|items| items.iter().find(|v| v["name"] == name))
            .and_then(|v| v["number"].as_i64())
            .ok_or_else(|| format!("Descriptor enum missing: {enum_name}.{name}"))?;
        i32::try_from(value).map_err(|_| "Descriptor enum exceeds int32".into())
    }

    fn scalar(
        &self,
        out: &mut Vec<u8>,
        message: &str,
        name: &str,
        kind: &str,
        value: i32,
    ) -> Result<(), String> {
        let number = self.number(message, name, kind)?;
        varint(out, u64::from(number) << 3);
        // protobuf int32 values are sign-extended; -1 must take ten bytes.
        varint(out, value as i64 as u64);
        Ok(())
    }

    fn enumeration(
        &self,
        out: &mut Vec<u8>,
        message: &str,
        name: &str,
        enum_name: &str,
        value: &str,
    ) -> Result<(), String> {
        self.scalar(
            out,
            message,
            name,
            "TYPE_ENUM",
            self.enum_value(enum_name, value)?,
        )
    }

    fn bytes(
        &self,
        out: &mut Vec<u8>,
        message: &str,
        name: &str,
        kind: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let number = self.number(message, name, kind)?;
        varint(out, (u64::from(number) << 3) | 2);
        varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
        Ok(())
    }

    fn connect_options(&self, controller: &str, caps: &[Decoder]) -> Result<Vec<u8>, String> {
        let mut capture = Vec::new();
        self.enumeration(&mut capture, "CaptureParams", "fps", "FPS", "FPS_30")?;
        self.scalar(
            &mut capture,
            "CaptureParams",
            "cursor_capture",
            "TYPE_BOOL",
            1,
        )?;
        self.enumeration(
            &mut capture,
            "CaptureParams",
            "video_quality",
            "VideoQuality",
            "VideoQuality_HD",
        )?;
        self.enumeration(
            &mut capture,
            "CaptureParams",
            "auto_frame_quality",
            "VideoQuality",
            "VideoQuality_HD",
        )?;
        self.enumeration(
            &mut capture,
            "CaptureParams",
            "choose_resolution_type",
            "ChooseResolutionType",
            "ChooseType_FOLLOW_REMOTE",
        )?;
        self.enumeration(
            &mut capture,
            "CaptureParams",
            "chroma_format",
            "ChromaFormat",
            "ChromaFormat_420",
        )?;
        self.scalar(&mut capture, "CaptureParams", "enable_hdr", "TYPE_BOOL", 0)?;
        self.scalar(&mut capture, "CaptureParams", "fpsCount", "TYPE_INT32", 30)?;
        let mut out = Vec::new();
        self.enumeration(
            &mut out,
            "ConnectOptions",
            "capture_type",
            "Type",
            "CT_DESKTOP",
        )?;
        self.scalar(&mut out, "ConnectOptions", "type_value", "TYPE_INT32", -1)?;
        self.bytes(
            &mut out,
            "ConnectOptions",
            "capture_params",
            "TYPE_MESSAGE",
            &capture,
        )?;
        for cap in caps {
            let mut encoded = Vec::new();
            self.scalar(&mut encoded, "DecoderCap", "fps", "TYPE_INT32", 30)?;
            self.enumeration(
                &mut encoded,
                "DecoderCap",
                "codec_type",
                "CodecType",
                cap.codec,
            )?;
            self.scalar(
                &mut encoded,
                "DecoderCap",
                "resolution_width",
                "TYPE_INT32",
                cap.width,
            )?;
            self.scalar(
                &mut encoded,
                "DecoderCap",
                "resolution_height",
                "TYPE_INT32",
                cap.height,
            )?;
            self.enumeration(
                &mut encoded,
                "DecoderCap",
                "chroma_format",
                "ChromaFormat",
                cap.chroma,
            )?;
            self.bytes(
                &mut out,
                "ConnectOptions",
                "decoder_cap_list",
                "TYPE_MESSAGE",
                &encoded,
            )?;
        }
        self.scalar(
            &mut out,
            "ConnectOptions",
            "force_virtual_display",
            "TYPE_BOOL",
            0,
        )?;
        let mut mode = Vec::new();
        self.scalar(&mut mode, "VirtualDisplayMode", "width", "TYPE_INT32", 1920)?;
        self.scalar(
            &mut mode,
            "VirtualDisplayMode",
            "height",
            "TYPE_INT32",
            1080,
        )?;
        self.scalar(&mut mode, "VirtualDisplayMode", "VSync", "TYPE_INT32", 60)?;
        self.bytes(
            &mut out,
            "ConnectOptions",
            "virtual_display_modes",
            "TYPE_MESSAGE",
            &mode,
        )?;
        self.enumeration(
            &mut out,
            "ConnectOptions",
            "client_type",
            "ClientType",
            "Client_WINDOWS",
        )?;
        self.bytes(
            &mut out,
            "ConnectOptions",
            "device_id",
            "TYPE_STRING",
            controller.as_bytes(),
        )?;
        self.enumeration(
            &mut out,
            "ConnectOptions",
            "control_connect_type",
            "ControlConnectType",
            "ControlConnectType_Normal",
        )?;
        // Only capabilities implemented by this controller are advertised.
        Ok(out)
    }
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

fn text_field<'a>(value: &'a Value, field: &str, required: bool) -> Result<&'a str, String> {
    let text = match value.get(field) {
        Some(Value::String(text)) => text.as_str(),
        None | Some(Value::Null) if !required => "",
        _ => {
            return Err(format!(
                "Room response field {field} is missing or is not text"
            ));
        }
    };
    if text.len() > MAX_BYTES || (required && text.is_empty()) || text.as_bytes().contains(&0) {
        return Err(format!(
            "Room response field {field} has invalid length or NUL"
        ));
    }
    Ok(text)
}

fn timeout_field(value: &Value, field: &str) -> Result<i32, String> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .filter(|&v| (0..=i32::MAX as i64).contains(&v))
        .map(|v| v as i32)
        .ok_or_else(|| format!("Room response requires nonnegative int32 {field}"))
}

#[derive(Default)]
struct Observation {
    room_states: Vec<i32>,
    codes: Vec<i32>,
    tracks: Vec<Value>,
    messages: Vec<Value>,
    login_completed: bool,
    terminal_room_state: bool,
    ignored_sessions: usize,
    layout: Option<Value>,
}

impl Observation {
    fn record(&mut self, event: Event, session: u32) {
        match event {
            Event::Layout(id, value) if id == session => self.layout = Some(value),
            Event::Room(id, state) if id == session => {
                if self.room_states.len() < 256 {
                    self.room_states.push(state);
                }
                if state == 2 {
                    self.login_completed = true;
                }
                if matches!(state, 0 | 3 | 4) {
                    self.terminal_room_state = true;
                }
            }
            Event::Code(id, code) if id == session => {
                if self.codes.len() < 256 {
                    self.codes.push(code);
                }
            }
            Event::Media(id, track) if id == session => {
                if self.tracks.len() < 128 && !self.tracks.contains(&track) {
                    self.tracks.push(track);
                }
            }
            Event::Echo(id, reply) if id == session => {
                let ptr = CONTROL_SENDER.load(Ordering::SeqCst);
                if ptr != 0 {
                    let send: unsafe extern "system" fn(i32, *const u8, u32) -> i32 =
                        unsafe { std::mem::transmute(ptr) };
                    unsafe {
                        send(session as i32, reply.as_ptr(), reply.len() as u32);
                    }
                }
            }
            Event::Data(id, value) if id == session => {
                if self.messages.len() < 64 && !self.messages.contains(&value) {
                    self.messages.push(value);
                }
            }
            _ => self.ignored_sessions += 1,
        }
    }

    fn wait(
        &mut self,
        rx: &Receiver<Event>,
        session: u32,
        for_media: bool,
        cancel: &std::sync::atomic::AtomicBool,
    ) {
        let deadline = Instant::now() + WAIT;
        loop {
            if cancel.load(Ordering::SeqCst)
                || self.terminal_room_state
                || (!for_media && self.login_completed)
            {
                break;
            }
            // Stop once a video track exists; audio-only arrival must not mask missing video.
            if for_media && self.tracks.iter().any(|t| t["type"] == "video") {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining.min(Duration::from_millis(200))) {
                Ok(event) => self.record(event, session),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            }
        }
        for event in rx.try_iter().take(256) {
            self.record(event, session);
        }
    }
}

fn release_sent<T: Ord>(
    held: &mut std::collections::BTreeSet<T>,
    input: &T,
    sent: Result<(), String>,
) -> Result<(), String> {
    sent?;
    held.remove(input);
    Ok(())
}

fn release_due(
    held: Option<Instant>,
    retry: Option<Instant>,
    now: Instant,
    emergency: bool,
) -> bool {
    match retry {
        Some(deadline) => now >= deadline,
        None => held.is_some_and(|deadline| emergency || now >= deadline),
    }
}

pub struct Engine {
    capture: Option<crate::render::Capture>,
    sdk: Sdk,
    _callbacks: CallbackRegistration,
    window: crate::render::RenderWindow,
    rx: Receiver<Event>,
    observed: Observation,
    session: i32,
    _room: Value,
    _servers: Vec<*const u8>,
    _lengths: Vec<i32>,
    _options: Vec<u8>,
    _login: Box<LoginRoom>,
    _connect: Box<ConnectParams>,
    paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
    name: String,
    dimensions: Option<(u32, u32)>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    render_track: Option<i32>,
    human_dimensions: Option<(u32, u32)>,
    held_keys: std::collections::BTreeSet<u32>,
    held_input_deadline: Option<Instant>,
    release_retry_deadline: Option<Instant>,
    held_buttons: std::collections::BTreeSet<String>,
    input_sequence: AtomicU64,
    input_generation: std::sync::Arc<AtomicU64>,
    active_input_generation: u64,
    video_encoder: Option<crate::video::Encoder>,
    video_started: Instant,
    control_id: String,
}
impl Engine {
    pub fn open(
        room: Value,
        controller: &str,
        name: String,
        paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
        input_generation: std::sync::Arc<AtomicU64>,
    ) -> Result<Self, String> {
        let token = text_field(&room, "token", true)?;
        let report_token = text_field(&room, "report_token", false)?;
        let report_url = text_field(&room, "report_url", false)?;
        let report_server = text_field(&room, "report_server_address", false)?;
        let list = room["signaling_list"]
            .as_array()
            .filter(|items| !items.is_empty() && items.len() <= 64)
            .ok_or("UU 缺少有效的信令服务")?;
        let mut servers = Vec::new();
        let mut lengths = Vec::new();
        for value in list {
            let text = value
                .as_str()
                .filter(|v| !v.is_empty() && v.len() <= 65536 && !v.as_bytes().contains(&0))
                .ok_or("UU 信令地址无效")?;
            servers.push(text.as_ptr());
            lengths.push(text.len() as i32);
        }
        let login = Box::new(LoginRoom {
            token: token.as_ptr(),
            token_len: token.len() as i32,
            pad0: 0,
            signaling: servers.as_ptr(),
            signaling_lengths: lengths.as_ptr(),
            signaling_count: servers.len() as i32,
            ws_connect_timeout_ms: timeout_field(&room, "ws_connect_timeout_ms")?,
            streamer_retry_delta_ms: timeout_field(&room, "streamer_retry_delta_ms")?,
            pad1: 0,
            report_token: report_token.as_ptr(),
            report_token_len: report_token.len() as i32,
            pad2: 0,
            report_url: report_url.as_ptr(),
            report_url_len: report_url.len() as i32,
            pad3: 0,
            report_server_address: report_server.as_ptr(),
            report_server_address_len: report_server.len() as i32,
            reserved: 0,
            proxy: ProxyConfig {
                proxy_type: 0,
                pad0: 0,
                host: ptr::null(),
                host_len: 0,
                port: 0,
                pad1: 0,
                username: ptr::null(),
                username_len: 0,
                pad2: 0,
                password: ptr::null(),
                password_len: 0,
                pad3: 0,
            },
        });
        let (tx, rx) = mpsc::sync_channel(256);
        let callbacks = CallbackRegistration::install(tx)?;
        let window = crate::render::RenderWindow::new(paused.clone())?;
        let mut sdk = Sdk::load()?;
        sdk.version()?;
        if sdk.init()? != 0 {
            return Err("UU 控制引擎初始化失败".into());
        }
        let (_, caps) = sdk.decoder_caps()?;
        if caps.is_empty() {
            return Err("本机没有可用的视频解码器".into());
        }
        let options = Schema::load()?.connect_options(controller, &caps)?;
        let create: CreateConnection = unsafe { std::mem::transmute(sdk.functions[2]) };
        let session = unsafe { create(&*login) };
        if session <= 0 {
            return Err("UU 连接会话创建失败".into());
        }
        sdk.session = Some(session);
        let mut observed = Observation::default();
        observed.wait(&rx, session as u32, false, &cancelled);
        if !observed.login_completed || observed.terminal_room_state {
            return Err("UU 信令连接未完成，请重新连接设备".into());
        }
        let connect = Box::new(ConnectParams {
            options: options.as_ptr(),
            options_len: options.len() as i32,
            pad0: 0,
            source_id: b"".as_ptr(),
            source_id_len: 0,
            pad1: 0,
        });
        let start: Connect = unsafe { std::mem::transmute(sdk.functions[4]) };
        if unsafe { start(session, &*connect) } != 0 {
            return Err("UU 媒体连接启动失败".into());
        }
        observed.wait(&rx, session as u32, true, &cancelled);
        if observed.terminal_room_state {
            return Err("UU 媒体连接已断开".into());
        }
        let end = Instant::now() + Duration::from_millis(600);
        while Instant::now() < end {
            if let Ok(event) = rx.recv_timeout(Duration::from_millis(50)) {
                observed.record(event, session as u32)
            }
        }
        if !observed.tracks.iter().any(|track| track["type"] == "video") {
            return Err("UU 尚未收到视频轨道，请检查远端桌面连接后重试".into());
        }
        let mut engine = Self {
            capture: None,
            sdk,
            _callbacks: callbacks,
            window,
            rx,
            observed,
            session,
            _room: room,
            _servers: servers,
            _lengths: lengths,
            _options: options,
            _login: login,
            _connect: connect,
            paused,
            name,
            dimensions: None,
            cancelled,
            render_track: None,
            human_dimensions: None,
            held_keys: Default::default(),
            held_buttons: Default::default(),
            held_input_deadline: None,
            release_retry_deadline: None,
            input_sequence: AtomicU64::new(1),
            input_generation,
            active_input_generation: 0,
            video_encoder: None,
            video_started: Instant::now(),
            control_id: format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| "本机时间无效")?
                    .as_nanos()
            ),
        };
        let track = engine
            .observed
            .messages
            .iter()
            .find_map(|m| {
                m["screens"].as_array().and_then(|rows| {
                    rows.iter()
                        .find(|r| r["id"] == m["current"])
                        .and_then(|r| r["track"].as_i64())
                })
            })
            .unwrap_or(0) as i32;
        engine.send(11, &crate::wire::request_track(track as u32))?;
        engine.send(11, &crate::wire::request_capture())?;
        engine.send(11, &crate::wire::start_desktop())?;
        #[repr(C)]
        struct Render {
            window: usize,
            track: i32,
            mode: i32,
        }
        let params = Render {
            window: engine.window.hwnd,
            track,
            // Mode 1 fits the video to the native window. Mode 0 consumes
            // viewport properties installed by the vendor's Qt client.
            mode: 1,
        };
        let render: unsafe extern "system" fn(i32, *const Render) -> i32 =
            unsafe { std::mem::transmute(engine.sdk.functions[6]) };
        engine.render_track = Some(track);
        if unsafe { render(session, &params) } != 0 {
            return Err("UU 画面渲染启动失败".into());
        }
        engine.window.refresh_size();
        engine.capture = Some(crate::render::Capture::new(engine.window.hwnd)?);
        Ok(engine)
    }
    fn send(&self, index: usize, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > 65536 {
            return Err("控制命令超过大小限制".into());
        }
        let send: unsafe extern "system" fn(i32, *const u8, u32) -> i32 =
            unsafe { std::mem::transmute(self.sdk.functions[index]) };
        if unsafe { send(self.session, bytes.as_ptr(), bytes.len() as u32) } != 0 {
            return Err("UU 控制通道发送失败".into());
        }
        Ok(())
    }
    fn input(&self, mut value: Value) -> Result<(), String> {
        if value["action"]
            .as_str()
            .is_some_and(|action| action.starts_with("mouse_"))
        {
            if let Some(screen) = self
                .observed
                .messages
                .iter()
                .rev()
                .find_map(|message| message.get("current").and_then(Value::as_u64))
            {
                value["screen_id"] = json!(screen);
            }
        }
        if value["action"]
            .as_str()
            .is_some_and(|action| action.starts_with("kbd_"))
        {
            value["kseq"] = json!(self.input_sequence.fetch_add(1, Ordering::Relaxed));
        }
        self.send(
            10,
            &serde_json::to_vec(&value).map_err(|_| "无法编码控制命令")?,
        )
    }
    fn finish_release(&mut self, result: &Result<(), String>) {
        if self.held_keys.is_empty() && self.held_buttons.is_empty() {
            self.held_input_deadline = None;
            self.release_retry_deadline = None;
        } else if result.is_err() {
            // Video keep-alives must never postpone a failed release.
            self.release_retry_deadline = Some(Instant::now() + Duration::from_millis(250));
        }
    }
    fn release_key(&mut self, key: u32) -> Result<(), String> {
        if !self.held_keys.contains(&key) {
            return Ok(());
        }
        let sent = crate::input::keyboard(key, true).and_then(|packet| self.input(packet));
        let result = release_sent(&mut self.held_keys, &key, sent);
        self.finish_release(&result);
        result
    }
    fn release_button(&mut self, button: &str) -> Result<(), String> {
        if !self.held_buttons.contains(button) {
            return Ok(());
        }
        let sent = crate::input::mouse_button(button, false).and_then(|packet| self.input(packet));
        let result = release_sent(&mut self.held_buttons, &button.to_string(), sent);
        self.finish_release(&result);
        result
    }
    fn release_inputs(&mut self) -> Result<(), String> {
        self.held_input_deadline = None;
        let mut error = None;
        for key in self.held_keys.iter().copied().collect::<Vec<_>>() {
            if let Err(failure) = self.release_key(key) {
                error.get_or_insert(failure);
            }
        }
        for button in self.held_buttons.iter().cloned().collect::<Vec<_>>() {
            if let Err(failure) = self.release_button(&button) {
                error.get_or_insert(failure);
            }
        }
        let result = error.map_or(Ok(()), Err);
        self.finish_release(&result);
        result
    }
    fn press_key(&mut self, key: u32) -> Result<(), String> {
        let packet = crate::input::keyboard(key, false)?;
        if self.release_retry_deadline.is_some() {
            self.release_inputs()?;
        }
        if self.held_keys.len() >= 16 && !self.held_keys.contains(&key) {
            return Err("同时按住的按键过多".into());
        }
        self.held_keys.insert(key);
        self.held_input_deadline = Some(Instant::now() + Duration::from_secs(5));
        let result = self.input(packet);
        if result.is_err() {
            let _ = self.release_inputs();
        }
        result
    }
    fn press_button(&mut self, button: &str) -> Result<(), String> {
        let packet = crate::input::mouse_button(button, true)?;
        if self.release_retry_deadline.is_some() {
            self.release_inputs()?;
        }
        self.held_buttons.insert(button.to_string());
        self.held_input_deadline = Some(Instant::now() + Duration::from_secs(5));
        let result = self.input(packet);
        if result.is_err() {
            let _ = self.release_inputs();
        }
        result
    }
    pub fn control_mode(&mut self, manual: bool) -> Result<(), String> {
        set_pause_state(
            &self.paused,
            if manual {
                PauseReason::GuiTakeover
            } else {
                PauseReason::ResumePending
            },
        );
        self.dimensions = None;
        let released_hotkey = self.window.control_changed(false);
        if let Err(error) = self.release_inputs() {
            set_pause_state(&self.paused, PauseReason::ReleaseFailed);
            return Err(error);
        }
        if let Err(error) = released_hotkey.and_then(|_| {
            if manual {
                Ok(())
            } else {
                self.window.control_changed(true)
            }
        }) {
            set_pause_state(&self.paused, PauseReason::EmergencyStopUnavailable);
            return Err(error);
        }
        if !manual && !finish_agent_resume(&self.paused, &PAUSE_REASON) {
            return Err(paused_error());
        }
        Ok(())
    }
    fn mark_human_control(&self) {
        if !self.paused.load(Ordering::SeqCst) {
            set_pause_state(&self.paused, PauseReason::GuiInput);
        }
        if self.window.escape_available.load(Ordering::SeqCst) {
            let _ = self.window.control_changed(false);
        }
    }
    pub fn poll(&mut self) {
        if release_due(
            self.held_input_deadline,
            self.release_retry_deadline,
            Instant::now(),
            PAUSE_REASON.load(Ordering::SeqCst) == PauseReason::EscapeHotkey as u8,
        ) {
            let _ = self.release_inputs();
        }
        for event in self.rx.try_iter().take(128) {
            self.observed.record(event, self.session as u32)
        }
    }
    fn dimensions_for(&self, human: bool) -> Option<(u32, u32)> {
        if human {
            self.human_dimensions
        } else {
            self.dimensions
        }
    }
    pub fn state(&self) -> Value {
        self.state_for(true)
    }
    pub(crate) fn state_for(&self, human: bool) -> Value {
        let dimensions = self.dimensions_for(human);
        let (width, height) = dimensions.unwrap_or((0, 0));
        json!({"title":self.name,"connected":self.observed.login_completed&&!self.observed.terminal_room_state,"interactive":dimensions.is_some()&&!self.observed.terminal_room_state,"phase":if self.observed.terminal_room_state{"disconnected"}else if dimensions.is_some(){"ready"}else{"waiting-for-frame"},"viewport":{"width":width,"height":height},"escapeAvailable":self.window.escape_available.load(Ordering::SeqCst),"emergencyStopShortcut":crate::render::STOP_SHORTCUT,"controlId":self.control_id,"controlDiagnostics":pause_diagnostics()})
    }
    fn allowed(&self, human: bool) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("COMPUTER_USE_ABORTED".into());
        }
        if !human && self.paused.load(Ordering::SeqCst) {
            return Err(paused_error());
        }
        if !human && !self.window.escape_available.load(Ordering::SeqCst) {
            set_pause_state(&self.paused, PauseReason::EmergencyStopUnavailable);
            return Err(format!(
                "COMPUTER_USE_EMERGENCY_STOP_UNAVAILABLE: {} 全局急停键不可用，智能体保持暂停",
                crate::render::STOP_SHORTCUT
            ));
        }
        if self.observed.terminal_room_state {
            return Err("UU 连接已断开，请重新连接设备".into());
        }
        Ok(())
    }
    pub fn capture(&mut self, human: bool) -> Result<Value, String> {
        self.poll();
        self.allowed(human)?;
        let rx = &self.rx;
        let observed = &mut self.observed;
        let session = self.session;
        let paused = self.paused.clone();
        let cancelled = self.cancelled.clone();
        let track = self.render_track;
        let input_generation = self.input_generation.clone();
        let generation = self.active_input_generation;
        let image = self
            .capture
            .as_mut()
            .ok_or("画面捕获不可用")?
            .read(
                || {
                    for event in rx.try_iter().take(128) {
                        observed.record(event, session as u32)
                    }
                    !cancelled.load(Ordering::SeqCst)
                        && !observed.terminal_room_state
                        && input_generation.load(Ordering::SeqCst) == generation
                        && (human || !paused.load(Ordering::SeqCst))
                },
                || {
                    FRAME_VIEWPORT.lock().ok().and_then(|v| {
                        v.filter(|viewport| {
                            viewport.session == session as u32 && Some(viewport.track) == track
                        })
                        .map(|viewport| viewport.ready)
                    })
                },
                Duration::from_secs(3),
            )
            .map_err(|message| {
                if cancelled.load(Ordering::SeqCst) {
                    "COMPUTER_USE_ABORTED".to_string()
                } else if input_generation.load(Ordering::SeqCst) != generation {
                    "COMPUTER_USE_CAPTURE_INTERRUPTED".to_string()
                } else if !human && paused.load(Ordering::SeqCst) {
                    paused_error()
                } else {
                    message
                }
            });
        let image = match image {
            Ok(image) => image,
            Err(error) => {
                if error != "COMPUTER_USE_CAPTURE_INTERRUPTED" {
                    if human {
                        self.human_dimensions = None
                    } else {
                        self.dimensions = None
                    }
                }
                return Err(error);
            }
        };
        if human {
            self.human_dimensions = Some((image.width, image.height))
        } else {
            self.dimensions = Some((image.width, image.height))
        }
        use base64::Engine;
        let screenshot = json!({"mediaType":"image/jpeg","base64":base64::engine::general_purpose::STANDARD.encode(&image.jpeg),"width":image.width,"height":image.height});
        Ok(json!({"state":self.state_for(human),"screenshot":screenshot}))
    }
    fn video_frame(&mut self, keyframe: bool, diagnostics: bool) -> Result<Value, String> {
        self.allowed(true)?;
        if !self.held_keys.is_empty() || !self.held_buttons.is_empty() {
            self.held_input_deadline = Some(Instant::now() + Duration::from_secs(5));
        }
        let session = self.session;
        let track = self.render_track;
        let cancelled = self.cancelled.clone();
        let input_generation = self.input_generation.clone();
        let generation = self.active_input_generation;
        let rx = &self.rx;
        let observed = &mut self.observed;
        let capture_started = Instant::now();
        let result = self.capture.as_mut().ok_or("视频捕获不可用")?.read_pixels(
            || {
                for event in rx.try_iter().take(128) {
                    observed.record(event, session as u32)
                }
                !cancelled.load(Ordering::SeqCst)
                    && !observed.terminal_room_state
                    && input_generation.load(Ordering::SeqCst) == generation
            },
            || {
                FRAME_VIEWPORT.lock().ok().and_then(|v| {
                    v.filter(|viewport| {
                        viewport.session == session as u32 && Some(viewport.track) == track
                    })
                    .map(|viewport| viewport.ready)
                })
            },
            Duration::from_millis(250),
        );
        let image = match result {
            Ok(image) => image,
            Err(e) => {
                if input_generation.load(Ordering::SeqCst) != generation {
                    return Err("COMPUTER_USE_CAPTURE_INTERRUPTED".into());
                }
                if e.starts_with("尚未收到") {
                    return Err("COMPUTER_USE_FRAME_PENDING".into());
                }
                return Err(e);
            }
        };
        let width = image.width() / 2 * 2;
        let height = image.height() / 2 * 2;
        let capture_us = capture_started.elapsed().as_micros() as u64;
        let encode_started = Instant::now();
        if self.video_encoder.as_ref().is_none_or(|encoder| {
            encoder.width != width || encoder.height != height || (keyframe && encoder.has_output)
        }) {
            self.video_encoder = Some(crate::video::Encoder::new(width, height)?);
        }
        let timestamp = self.video_started.elapsed().as_micros() as u64;
        let packet = match self
            .video_encoder
            .as_mut()
            .unwrap()
            .encode(&image, timestamp)
        {
            Ok(packet) => packet,
            Err(error) if self.video_encoder.as_ref().unwrap().hardware => {
                let mut encoder = crate::video::Encoder::software(width, height)?;
                encoder.hardware_failure = Some(error);
                let packet = encoder.encode(&image, timestamp)?;
                self.video_encoder = Some(encoder);
                packet
            }
            Err(error) => return Err(error),
        };
        let encoder = self.video_encoder.as_ref().unwrap();
        let encoder_hardware = encoder.hardware;
        let hardware_failure = encoder.hardware_failure.clone();
        self.human_dimensions = Some((width, height));
        use base64::Engine as _;
        let video=packet.map(|packet|json!({"data":base64::engine::general_purpose::STANDARD.encode(packet.bytes),"key":packet.key,"timestamp":packet.timestamp_us,"codec":encoder.codec,"width":width,"height":height}));
        let mut value = json!({"state":self.state(),"video":video});
        if diagnostics {
            value["timings"] = json!({"captureMicros":capture_us,"encodeMicros":encode_started.elapsed().as_micros()as u64,"hardware":encoder_hardware,"hardwareFallback":hardware_failure});
        }
        Ok(value)
    }
    pub fn act(
        &mut self,
        args: &Value,
        human: bool,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
        generation: u64,
    ) -> Result<Value, String> {
        self.cancelled = cancelled;
        self.active_input_generation = generation;
        self.poll();
        self.allowed(human)?;
        let action = args["action"].as_str().ok_or("缺少控制命令")?;
        if args.get("controlId").is_some()
            && args["controlId"].as_str() != Some(self.control_id.as_str())
        {
            return Err("COMPUTER_USE_STALE_CONTROL".into());
        }
        if action == "status" {
            if args["diagnostics"] == true {
                let stats: unsafe extern "system" fn(i32) -> i32 =
                    unsafe { std::mem::transmute(self.sdk.functions[14]) };
                unsafe {
                    stats(self.session);
                }
                let deadline = Instant::now() + Duration::from_millis(300);
                while Instant::now() < deadline {
                    if let Ok(event) = self.rx.recv_timeout(Duration::from_millis(25)) {
                        self.observed.record(event, self.session as u32)
                    }
                }
                return Ok(
                    json!({"state":self.state(),"diagnostics":{"window":self.window.metrics(),"layout":self.observed.layout,"frameNotifications":FRAME_CHANGES.load(Ordering::Relaxed),"capture":self.capture.as_ref().map(|capture|capture.diagnostics()),"tracks":self.observed.tracks,"connectionCodes":self.observed.codes,"stats":self.observed.messages.iter().filter(|m|m.get("mediaStreamCount").is_some()).collect::<Vec<_>>()}}),
                );
            }
            return Ok(json!({"state":self.state_for(human)}));
        }
        if action == "capture" {
            return self.capture(human);
        }
        if action == "video_frame" {
            if !human {
                return Err("COMPUTER_USE_HUMAN_REQUIRED".into());
            }
            return self.video_frame(args["keyFrame"] == true, args["diagnostics"] == true);
        }

        if matches!(
            action,
            "mouse_move" | "mouse_down" | "mouse_up" | "key_down" | "key_up" | "release_inputs"
        ) {
            if !human {
                return Err("COMPUTER_USE_HUMAN_REQUIRED".into());
            }
            if args["controlId"].as_str() != Some(self.control_id.as_str()) {
                return Err("COMPUTER_USE_STALE_CONTROL".into());
            }
            if action == "release_inputs" {
                self.release_inputs()?;
                return Ok(json!({"state":self.state()}));
            }
            self.mark_human_control();
            self.held_input_deadline = Some(Instant::now() + Duration::from_secs(5));
            if action == "key_up" {
                let code = crate::input::key_code(args["key"].as_str().ok_or("缺少按键")?)?;
                self.release_key(code)?;
                return Ok(json!({"state":self.state()}));
            }
            if action == "mouse_up" {
                let button = args["button"].as_str().unwrap_or("left");
                self.release_button(button)?;
                return Ok(json!({"state":self.state()}));
            }
            if self.release_retry_deadline.is_some() {
                self.release_inputs()?;
            }
            let (width, height) = self.dimensions_for(true).ok_or("请先等待桌面画面")?;
            if action == "key_down" {
                let code = crate::input::key_code(args["key"].as_str().ok_or("缺少按键")?)?;
                self.press_key(code)?;
            } else {
                self.input(crate::input::mouse_move(
                    args["x"].as_f64().ok_or("缺少横坐标")?,
                    args["y"].as_f64().ok_or("缺少纵坐标")?,
                    width,
                    height,
                )?)?;
                if action == "mouse_down" {
                    let button = args["button"].as_str().unwrap_or("left");
                    self.press_button(button)?;
                }
            }
            return Ok(json!({"state":self.state()}));
        }
        if human {
            self.mark_human_control();
        }
        if self.release_retry_deadline.is_some() {
            self.release_inputs()?;
        }
        let (width, height) = self
            .dimensions_for(human)
            .ok_or("尚无实际画面，不能发送输入；请先刷新画面")?;
        let coord = |key: &str| {
            args[key]
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("无效坐标 {key}"))
        };
        match action {
            "click" | "double_click" => {
                self.input(crate::input::mouse_move(
                    coord("x")?,
                    coord("y")?,
                    width,
                    height,
                )?)?;
                let button = args["button"].as_str().unwrap_or("left");
                for _ in 0..if action == "double_click" { 2 } else { 1 } {
                    self.allowed(human)?;
                    let pressed = self.press_button(button);
                    let released = self.release_button(button);
                    pressed?;
                    released?;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            "scroll" => {
                self.input(crate::input::wheel(
                    args["deltaX"].as_f64().unwrap_or(0.0),
                    args["deltaY"].as_f64().unwrap_or(0.0),
                )?)?;
            }
            "key" | "keypress" => {
                let keys = args["keys"]
                    .as_array()
                    .filter(|v| !v.is_empty() && v.len() <= 5)
                    .ok_or("keys 需要 1 至 5 个按键")?;
                let keys = keys
                    .iter()
                    .map(|v| crate::input::key_code(v.as_str().ok_or("按键名称无效")?))
                    .collect::<Result<Vec<_>, String>>()?;
                let mut held = Vec::new();
                let result = (|| {
                    for key in keys {
                        self.allowed(human)?;
                        held.push(key);
                        self.press_key(key)?;
                    }
                    Ok::<(), String>(())
                })();
                let mut release_error = None;
                for key in held.into_iter().rev() {
                    if let Err(error) = self.release_key(key) {
                        release_error.get_or_insert(error);
                    }
                }
                result?;
                if let Some(error) = release_error {
                    return Err(error);
                }
            }
            "type" | "input" => {
                let text = args["text"]
                    .as_str()
                    .filter(|v| !v.is_empty() && v.len() <= 32768)
                    .ok_or("输入内容需要 1 至 32768 字节")?;
                self.input(json!({"action":"text_input","content":text}))?;
            }
            "drag" => {
                let x = coord("x")?;
                let y = coord("y")?;
                let end_x = coord("endX")?;
                let end_y = coord("endY")?;
                crate::input::mouse_move(end_x, end_y, width, height)?;
                self.input(crate::input::mouse_move(x, y, width, height)?)?;
                let pressed = self.press_button("left");
                let result = (|| {
                    pressed?;
                    for step in 1..=12 {
                        self.allowed(human)?;
                        let t = f64::from(step) / 12.0;
                        self.input(crate::input::mouse_move(
                            x + (end_x - x) * t,
                            y + (end_y - y) * t,
                            width,
                            height,
                        )?)?;
                        std::thread::sleep(Duration::from_millis(16));
                    }
                    Ok::<(), String>(())
                })();
                let released = self.release_button("left");
                result?;
                released?;
            }
            _ => return Err("不支持的 UU 控制命令".into()),
        }
        if args["includeScreenshot"] == false {
            return Ok(json!({"state":self.state_for(human)}));
        }
        std::thread::sleep(Duration::from_millis(120));
        self.capture(human)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.release_inputs();
        self.capture.take();
        if let Some(track) = self.render_track.take() {
            let stop: unsafe extern "system" fn(i32, i32) -> i32 =
                unsafe { std::mem::transmute(self.sdk.functions[8]) };
            unsafe {
                stop(self.session, track);
            }
        }
        // The SDK subclasses the rendering HWND. Destroy it while the DLL
        // containing its window procedure is still loaded.
        self.window.close();
        self.sdk.shutdown();
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;

    #[test]
    fn completing_hotkey_registration_does_not_override_a_new_takeover() {
        let paused = AtomicBool::new(true);
        for reason in [
            PauseReason::EscapeHotkey,
            PauseReason::GuiInput,
            PauseReason::EmergencyStopUnavailable,
        ] {
            let reason = AtomicU8::new(reason as u8);
            assert!(!finish_agent_resume(&paused, &reason));
            assert!(paused.load(Ordering::SeqCst));
        }
        let reason = AtomicU8::new(PauseReason::ResumePending as u8);
        assert!(finish_agent_resume(&paused, &reason));
        assert!(!paused.load(Ordering::SeqCst));
        assert_eq!(reason.load(Ordering::SeqCst), PauseReason::Agent as u8);
    }

    #[test]
    fn shutdown_is_not_reported_as_human_takeover() {
        assert_eq!(
            paused_error_for(PauseReason::ReaderShutdown as u8),
            "COMPUTER_USE_DESKTOP_DISCONNECTED"
        );
        for reason in [
            PauseReason::StartHuman,
            PauseReason::GuiInput,
            PauseReason::GuiTakeover,
            PauseReason::ResumePending,
            PauseReason::EscapeHotkey,
            PauseReason::ReleaseFailed,
        ] {
            let message = paused_error_for(reason as u8);
            let (code, data) = message.split_once("; controlDiagnostics=").unwrap();
            assert_eq!(code, "COMPUTER_USE_MANUAL_CONTROL");
            let data: Value = serde_json::from_str(data).unwrap();
            assert_eq!(data, json!({"pauseReason":pause_reason_name(reason as u8)}));
        }
    }

    #[test]
    fn failed_key_and_button_releases_remain_available_for_retry() {
        let mut keys = std::collections::BTreeSet::from([17_u32, 65]);
        let mut buttons =
            std::collections::BTreeSet::from(["left".to_string(), "right".to_string()]);
        assert!(release_sent(&mut keys, &17, Err("send failed".into())).is_err());
        assert!(release_sent(&mut buttons, &"left".into(), Err("send failed".into())).is_err());
        release_sent(&mut keys, &65, Ok(())).unwrap();
        release_sent(&mut buttons, &"right".into(), Ok(())).unwrap();
        assert_eq!(keys.iter().copied().collect::<Vec<_>>(), vec![17]);
        assert_eq!(buttons.iter().cloned().collect::<Vec<_>>(), vec!["left"]);
        release_sent(&mut buttons, &"left".into(), Ok(())).unwrap();
        release_sent(&mut keys, &17, Ok(())).unwrap();
        assert!(keys.is_empty());
        assert!(buttons.is_empty());
    }

    #[test]
    fn video_keepalive_cannot_postpone_a_failed_release() {
        let now = Instant::now();
        let retry = Some(now + Duration::from_millis(250));
        let refreshed_hold = Some(now + Duration::from_secs(5));
        assert!(!release_due(
            refreshed_hold,
            retry,
            now + Duration::from_millis(249),
            false
        ));
        assert!(release_due(
            refreshed_hold,
            retry,
            now + Duration::from_millis(250),
            false
        ));
        assert!(!release_due(
            None,
            None,
            now + Duration::from_secs(60),
            false
        ));
    }

    #[test]
    fn emergency_stop_releases_held_input_without_waiting_for_the_hold_timeout() {
        let now = Instant::now();
        let held = Some(now + Duration::from_secs(5));
        assert!(
            !release_due(held, None, now, false),
            "manual drag can remain held"
        );
        assert!(release_due(held, None, now, true));
        let retry = Some(now + Duration::from_millis(250));
        assert!(!release_due(held, retry, now, true));
        assert!(release_due(
            held,
            retry,
            now + Duration::from_millis(250),
            true
        ));
        assert!(!release_due(None, None, now, true));
    }
}
