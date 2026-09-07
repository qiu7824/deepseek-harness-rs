//! Local desktop capture and input. No vendor transport or account is required.
use crate::{
    capture::Capture,
    input::{self, Bounds},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::GetModuleHandleW, StationsAndDesktops::*},
    UI::WindowsAndMessaging::*,
};

static HUMAN_CONTROL: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
fn human_input() {
    if let Ok(guard) = HUMAN_CONTROL.try_lock() {
        if let Some(flag) = guard.as_ref() {
            flag.store(true, Ordering::SeqCst)
        }
    }
}
unsafe extern "system" fn keyboard_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == HC_ACTION as i32 {
        let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if event.dwExtraInfo != input::INPUT_MARKER {
            human_input();
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
unsafe extern "system" fn mouse_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == HC_ACTION as i32 {
        let event = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        if event.dwExtraInfo != input::INPUT_MARKER {
            human_input();
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
struct Intervention {
    thread: u32,
    join: Option<std::thread::JoinHandle<()>>,
    available: bool,
}
impl Intervention {
    fn new(paused: Arc<AtomicBool>) -> Result<Self, String> {
        *HUMAN_CONTROL.lock().map_err(|_| "接管状态不可用")? = Some(paused);
        let (tx, rx) = mpsc::sync_channel(1);
        let join = std::thread::spawn(move || unsafe {
            let mut msg = std::mem::zeroed();
            PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
            let module = GetModuleHandleW(std::ptr::null());
            let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
            let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
            let available = !keyboard.is_null() && !mouse.is_null();
            let _ = tx.send((
                windows_sys::Win32::System::Threading::GetCurrentThreadId(),
                available,
            ));
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !keyboard.is_null() {
                UnhookWindowsHookEx(keyboard);
            }
            if !mouse.is_null() {
                UnhookWindowsHookEx(mouse);
            }
        });
        let (thread, available) = rx
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| "人工接管监听启动超时")?;
        Ok(Self {
            thread,
            join: Some(join),
            available,
        })
    }
}
impl Drop for Intervention {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread, WM_QUIT, 0, 0);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        if let Ok(mut guard) = HUMAN_CONTROL.lock() {
            *guard = None;
        }
    }
}

fn desktop_available() -> bool {
    unsafe {
        let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if desktop.is_null() {
            return false;
        }
        let mut name = [0u16; 64];
        let mut needed = 0;
        let ok = GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            (name.len() * 2) as u32,
            &mut needed,
        ) != 0;
        CloseDesktop(desktop);
        ok && String::from_utf16_lossy(
            &name[..name.iter().position(|n| *n == 0).unwrap_or(name.len())],
        )
        .eq_ignore_ascii_case("default")
    }
}
fn bounds(hwnd: Option<usize>, monitor: usize) -> Result<Bounds, String> {
    unsafe {
        let mut rect = RECT::default();
        if let Some(hwnd) = hwnd {
            if IsWindow(hwnd as _) == 0 || IsIconic(hwnd as _) != 0 {
                return Err("目标窗口已关闭或最小化".into());
            }
            if DwmGetWindowAttribute(
                hwnd as _,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                (&mut rect as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            ) < 0
                && GetWindowRect(hwnd as _, &mut rect) == 0
            {
                return Err("无法取得窗口范围".into());
            }
        } else {
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..std::mem::zeroed()
            };
            if GetMonitorInfoW(monitor as _, &mut info) == 0 {
                return Err("显示器布局已变化，请重新连接".into());
            }
            rect = info.rcMonitor;
        }
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return Err("桌面范围无效".into());
        }
        Ok(Bounds {
            left: rect.left,
            top: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        })
    }
}
pub struct Engine {
    capture: Capture,
    target: Option<usize>,
    monitor: usize,
    paused: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    observed: Option<(u32, u32, Bounds)>,
    intervention: Intervention,
}
impl Engine {
    pub fn open(
        args: &Value,
        paused: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        if !desktop_available() {
            return Err("桌面已锁定或处于安全界面，请人工处理后重试".into());
        }
        let target = match args.get("windowId") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|v| *v > 0 && *v <= isize::MAX as u64)
                    .ok_or("窗口 ID 无效")? as usize,
            ),
        };
        let monitor =
            unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) } as usize;
        bounds(target, monitor)?;
        let capture = Capture::new(target, monitor)?;
        let intervention = Intervention::new(paused.clone())?;
        if !intervention.available && !paused.load(Ordering::SeqCst) {
            return Err("人工接管监听不可用，无法启动智能体桌面控制".into());
        }
        Ok(Self {
            capture,
            target,
            monitor,
            paused,
            cancelled,
            observed: None,
            intervention,
        })
    }
    pub fn poll(&mut self) {}
    pub fn state(&self) -> Value {
        let (w, h) = self.observed.map(|(w, h, _)| (w, h)).unwrap_or((0, 0));
        json!({"title":"本机桌面","connected":true,"interactive":self.observed.is_some(),"phase":if self.observed.is_some(){"ready"}else{"waiting-for-frame"},"viewport":{"width":w,"height":h},"windowId":self.target,"manualInterventionAvailable":self.intervention.available,"escapeAvailable":self.intervention.available})
    }
    pub fn control_mode(&mut self, manual: bool) {
        self.paused
            .store(manual || !self.intervention.available, Ordering::SeqCst);
        self.observed = None;
    }
    fn allowed(&self, human: bool) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("COMPUTER_USE_ABORTED".into());
        }
        if !human && self.paused.load(Ordering::SeqCst) {
            return Err("COMPUTER_USE_MANUAL_CONTROL".into());
        }
        if !desktop_available() {
            return Err("桌面已锁定或处于安全界面，请人工处理后重试".into());
        }
        Ok(())
    }
    fn snapshot(&mut self, human: bool) -> Result<Value, String> {
        self.allowed(human)?;
        let rect = bounds(self.target, self.monitor)?;
        let paused = self.paused.clone();
        let cancelled = self.cancelled.clone();
        let result = self.capture.read(
            || {
                !cancelled.load(Ordering::SeqCst)
                    && (human || !paused.load(Ordering::SeqCst))
                    && desktop_available()
            },
            Duration::from_secs(3),
        );
        let frame = match result {
            Ok(frame) => frame,
            Err(error) => {
                self.observed = None;
                if cancelled.load(Ordering::SeqCst) {
                    return Err("COMPUTER_USE_ABORTED".into());
                }
                if !human && paused.load(Ordering::SeqCst) {
                    return Err("COMPUTER_USE_MANUAL_CONTROL".into());
                }
                return Err(error);
            }
        };
        if bounds(self.target, self.monitor)? != rect {
            self.observed = None;
            return Err("窗口位置已变化，请重新截图".into());
        }
        self.observed = Some((frame.width, frame.height, rect));
        use base64::Engine as _;
        let screenshot = json!({"mediaType":"image/jpeg","width":frame.width,"height":frame.height,"base64":base64::engine::general_purpose::STANDARD.encode(&frame.jpeg)});
        self.allowed(human)?;
        Ok(json!({"state":self.state(),"screenshot":screenshot}))
    }
    pub fn act(
        &mut self,
        args: &Value,
        human: bool,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Value, String> {
        self.cancelled = cancelled;
        self.allowed(human)?;
        let action = args["action"].as_str().ok_or("缺少操作")?;
        if action == "status" {
            return Ok(json!({"state":self.state()}));
        }
        if action == "capture" {
            return self.snapshot(human);
        }
        let (w, h, rect) = self.observed.ok_or("请先观察桌面画面，再发送输入")?;
        if bounds(self.target, self.monitor)? != rect {
            self.observed = None;
            return Err("桌面布局已变化，请重新截图".into());
        }
        if human {
            self.paused.store(true, Ordering::SeqCst)
        }
        if let Some(target) = self.target {
            if unsafe { GetForegroundWindow() } as usize != target {
                return Err("目标窗口不在前台，请先将窗口置于前台并重新观察".into());
            }
        }
        let coordinate = |x: &str, y: &str| {
            input::position(
                args[x].as_f64().ok_or("缺少横坐标")?,
                args[y].as_f64().ok_or("缺少纵坐标")?,
                w,
                h,
                rect,
            )
        };
        match action {
            "click" | "double_click" => {
                let (x, y) = coordinate("x", "y")?;
                let button = args["button"].as_str().unwrap_or("left");
                if !matches!(button, "left" | "right" | "middle") {
                    return Err("不支持的鼠标按键".into());
                }
                input::move_to(x, y)?;
                for _ in 0..if action == "double_click" { 2 } else { 1 } {
                    self.allowed(human)?;
                    let down = input::button(button, true);
                    let up = input::button(button, false);
                    down?;
                    up?;
                    std::thread::sleep(Duration::from_millis(40));
                }
            }
            "type" | "input" => {
                let text = args["text"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 32768)
                    .ok_or("输入文本长度无效")?;
                for unit in text.encode_utf16() {
                    self.allowed(human)?;
                    let down = input::unicode(unit, false);
                    let up = input::unicode(unit, true);
                    down?;
                    up?;
                }
            }
            "key" | "keypress" => {
                let keys = args["keys"]
                    .as_array()
                    .filter(|v| !v.is_empty() && v.len() <= 5)
                    .ok_or("需要 1 至 5 个按键")?;
                let keys = keys
                    .iter()
                    .map(|k| input::key_code(k.as_str().ok_or("按键格式无效")?))
                    .collect::<Result<Vec<_>, String>>()?;
                let mut held = Vec::new();
                let down = (|| {
                    for key in keys {
                        self.allowed(human)?;
                        held.push(key);
                        input::key(key, false)?;
                    }
                    Ok::<(), String>(())
                })();
                let mut released = Ok(());
                for key in held.into_iter().rev() {
                    if let Err(error) = input::key(key, true) {
                        released = Err(error)
                    }
                }
                down?;
                released?;
            }
            "scroll" => input::scroll(
                args["deltaX"].as_f64().unwrap_or(0.0),
                args["deltaY"].as_f64().unwrap_or(0.0),
            )?,
            "drag" => {
                let (x, y) = coordinate("x", "y")?;
                let (ex, ey) = coordinate("endX", "endY")?;
                input::move_to(x, y)?;
                let pressed = input::button("left", true);
                let moved = (|| {
                    pressed?;
                    for n in 1..=12 {
                        self.allowed(human)?;
                        input::move_to(x + (ex - x) * n / 12, y + (ey - y) * n / 12)?;
                        std::thread::sleep(Duration::from_millis(16));
                    }
                    Ok::<(), String>(())
                })();
                let released = input::button("left", false);
                moved?;
                released?;
            }
            _ => return Err("不支持的本机桌面操作".into()),
        }
        std::thread::sleep(Duration::from_millis(100));
        self.snapshot(human)
    }
}
