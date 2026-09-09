//! Local desktop capture and input. No vendor transport or account is required.
use crate::{
    capture::Capture,
    input::{self, Bounds},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::GetModuleHandleW, StationsAndDesktops::*},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetDoubleClickTime, IsWindowEnabled, VK_CONTROL, VK_LBUTTON, VK_LWIN,
            VK_MBUTTON, VK_MENU, VK_RBUTTON, VK_RWIN, VK_SHIFT, VK_XBUTTON1, VK_XBUTTON2,
        },
        WindowsAndMessaging::*,
    },
};

static HUMAN_CONTROL: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
#[derive(Clone)]
struct HookDiagnostic {
    sequence: u64,
    kind: &'static str,
    classification: input::HookClassification,
    lower_integrity: bool,
    phase: &'static str,
    last_phase: &'static str,
}
impl HookDiagnostic {
    fn value(&self) -> Value {
        json!({"sequence":self.sequence,"device":self.kind,"source":if self.classification.injected {"injected"} else {"physical"},"ownMarker":self.classification.own_marker,"low32MarkerMatch":self.classification.low32_marker_match,"lowerIntegrityInjected":self.lower_integrity,"injectionPhase":self.phase,"lastInjectionPhase":self.last_phase})
    }
}
struct ControlDiagnostics {
    sequence: u64,
    own_events: u64,
    physical_events: u64,
    foreign_injected_events: u64,
    last_event: Option<HookDiagnostic>,
    takeover: Option<HookDiagnostic>,
    mode_change: &'static str,
}
impl ControlDiagnostics {
    const fn new() -> Self {
        Self {
            sequence: 0,
            own_events: 0,
            physical_events: 0,
            foreign_injected_events: 0,
            last_event: None,
            takeover: None,
            mode_change: "initial",
        }
    }
    fn record(
        &mut self,
        kind: &'static str,
        classification: input::HookClassification,
        lower_integrity: bool,
    ) {
        self.sequence = self.sequence.saturating_add(1);
        if classification.is_own() {
            self.own_events = self.own_events.saturating_add(1);
        } else if classification.injected {
            self.foreign_injected_events = self.foreign_injected_events.saturating_add(1);
        } else {
            self.physical_events = self.physical_events.saturating_add(1);
        }
        let (phase, last_phase) = input::injection_phases();
        let event = HookDiagnostic {
            sequence: self.sequence,
            kind,
            classification,
            lower_integrity,
            phase,
            last_phase,
        };
        if !classification.is_own() && self.takeover.is_none() {
            self.takeover = Some(event.clone());
        }
        self.last_event = Some(event);
    }
    fn value(&self) -> Value {
        let (phase, last_phase) = input::injection_phases();
        json!({"markerWidthBits":32,"ownEvents":self.own_events,"physicalEvents":self.physical_events,"foreignInjectedEvents":self.foreign_injected_events,"modeChange":self.mode_change,"injectionPhase":phase,"lastInjectionPhase":last_phase,"lastEvent":self.last_event.as_ref().map(HookDiagnostic::value),"takeover":self.takeover.as_ref().map(HookDiagnostic::value)})
    }
}
static CONTROL_DIAGNOSTICS: Mutex<ControlDiagnostics> = Mutex::new(ControlDiagnostics::new());
pub fn control_diagnostics() -> Value {
    CONTROL_DIAGNOSTICS
        .lock()
        .map(|diagnostics| diagnostics.value())
        .unwrap_or_else(|_| json!({"available":false}))
}
fn observe_input(kind: &'static str, extra_info: usize, injected: bool, lower_integrity: bool) {
    let classification = input::classify_hook(extra_info, injected);
    if let Ok(mut diagnostics) = CONTROL_DIAGNOSTICS.try_lock() {
        diagnostics.record(kind, classification, lower_integrity);
    }
    if !classification.is_own() {
        human_input();
    }
}
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
        observe_input(
            "keyboard",
            event.dwExtraInfo,
            event.flags & LLKHF_INJECTED != 0,
            event.flags & LLKHF_LOWER_IL_INJECTED != 0,
        );
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
unsafe extern "system" fn mouse_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == HC_ACTION as i32 {
        let event = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        observe_input(
            "mouse",
            event.dwExtraInfo,
            event.flags & LLMHF_INJECTED != 0,
            event.flags & LLMHF_LOWER_IL_INJECTED != 0,
        );
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
        if let Ok(mut diagnostics) = CONTROL_DIAGNOSTICS.lock() {
            *diagnostics = ControlDiagnostics::new();
            diagnostics.mode_change = if paused.load(Ordering::SeqCst) {
                "start-human"
            } else {
                "start-agent"
            };
        }
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
fn window_targets() -> Result<Value, String> {
    unsafe extern "system" fn collect(hwnd: HWND, parameter: LPARAM) -> i32 {
        let rows = unsafe { &mut *(parameter as *mut Vec<Value>) };
        if rows.len() >= 256 {
            return 0;
        }
        if unsafe { IsWindowVisible(hwnd) } == 0 || unsafe { IsIconic(hwnd) } != 0 {
            return 1;
        }
        let mut cloaked = 0u32;
        if unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as u32,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        } == 0
            && cloaked != 0
        {
            return 1;
        }
        let length = unsafe { GetWindowTextLengthW(hwnd) };
        if !(1..=4096).contains(&length) {
            return 1;
        }
        let mut title = vec![0u16; length as usize + 1];
        let length = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
        let mut rect = RECT::default();
        if length <= 0
            || unsafe { GetWindowRect(hwnd, &mut rect) } == 0
            || rect.right <= rect.left
            || rect.bottom <= rect.top
        {
            return 1;
        }
        rows.push(json!({"windowId":hwnd as usize,"title":String::from_utf16_lossy(&title[..length as usize]),"visible":true,"foreground":unsafe { GetForegroundWindow() } == hwnd,"bounds":{"left":rect.left,"top":rect.top,"width":rect.right-rect.left,"height":rect.bottom-rect.top}}));
        1
    }
    let mut rows = Vec::<Value>::new();
    let success = unsafe { EnumWindows(Some(collect), &mut rows as *mut Vec<Value> as LPARAM) };
    if success == 0 && rows.len() < 256 {
        return Err("无法枚举可见窗口".into());
    }
    Ok(json!({"windows":rows}))
}
fn requested_wait_ms(args: &Value, default_ms: u64) -> Result<u64, String> {
    match args.get("waitMs") {
        None => Ok(default_ms),
        Some(value) => value
            .as_u64()
            .filter(|value| *value <= 10000)
            .ok_or_else(|| "等待时间必须为 0 至 10000 毫秒".into()),
    }
}

fn validate_pointer_target(
    target: Option<usize>,
    hit_test: impl FnOnce(usize) -> (usize, bool),
) -> Result<(), String> {
    if let Some(target) = target {
        let (root, enabled) = hit_test(target);
        if !enabled || root != target {
            return Err("COMPUTER_USE_TARGET_OBSCURED: 输入点被其他窗口遮挡或目标窗口已禁用，请人工处理遮挡后重新观察".into());
        }
    }
    Ok(())
}

fn caption_coordinates(rect: RECT, state: u32) -> Vec<(i32, i32)> {
    // TITLEBARINFO: unavailable, invisible, or offscreen captions are unusable.
    if state & (0x00000001 | 0x00008000 | 0x00010000) != 0 {
        return Vec::new();
    }
    let width = i64::from(rect.right) - i64::from(rect.left);
    let height = i64::from(rect.bottom) - i64::from(rect.top);
    if width <= 0 || height <= 0 {
        return Vec::new();
    }
    let y = (i64::from(rect.top) + height / 2) as i32;
    let mut points = Vec::new();
    for (numerator, denominator) in [(1, 2), (1, 3), (2, 3), (1, 4), (3, 4)] {
        let x = (i64::from(rect.left) + width * numerator / denominator) as i32;
        if caption_lparam(x, y).is_some() && !points.contains(&(x, y)) {
            points.push((x, y));
        }
    }
    points
}

fn caption_lparam(x: i32, y: i32) -> Option<isize> {
    let x = i16::try_from(x).ok()? as u16 as u32;
    let y = i16::try_from(y).ok()? as u16 as u32;
    Some((x | (y << 16)) as i32 as isize)
}

fn verified_caption(
    target: usize,
    hit: usize,
    root: usize,
    enabled: bool,
    result: Option<usize>,
) -> bool {
    hit == target && root == target && enabled && result == Some(HTCAPTION as usize)
}

fn activation_input_idle() -> bool {
    unsafe {
        [
            VK_LBUTTON,
            VK_RBUTTON,
            VK_MBUTTON,
            VK_XBUTTON1,
            VK_XBUTTON2,
            VK_SHIFT,
            VK_CONTROL,
            VK_MENU,
            VK_LWIN,
            VK_RWIN,
        ]
        .iter()
        .all(|key| GetAsyncKeyState(i32::from(*key)) >= 0)
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
    held: input::HeldInputs,
    held_deadline: Option<Instant>,
    held_foreground: Option<usize>,
    control_id: String,
    input_generation: Arc<AtomicU64>,
    active_input_generation: u64,
    last_caption_click: Option<Instant>,
}
impl Engine {
    pub fn open(
        args: &Value,
        paused: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        input_generation: Arc<AtomicU64>,
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
            held: input::HeldInputs::default(),
            held_deadline: None,
            held_foreground: None,
            control_id: format!(
                "desktop-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            input_generation,
            active_input_generation: 0,
            last_caption_click: None,
        })
    }
    fn release_inputs(&mut self) -> Result<(), String> {
        let result = self.held.release_all(input::release_held);
        if self.held.is_empty() {
            self.held_deadline = None;
            self.held_foreground = None;
        }
        result
    }
    fn remember_input(&mut self, value: input::HeldInput) {
        self.held.press(value);
        self.held_deadline = Some(Instant::now() + Duration::from_secs(5));
        self.held_foreground = Some(unsafe { GetForegroundWindow() } as usize);
    }
    pub fn poll(&mut self) {
        if !self.held.is_empty()
            && (self
                .held_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
                || self.held_foreground != Some(unsafe { GetForegroundWindow() } as usize)
                || !desktop_available())
        {
            let _ = self.release_inputs();
        }
    }
    pub fn check_start_target(&self, args: &Value) -> Result<(), String> {
        if let Some(value) = args.get("windowId") {
            let requested = match value {
                Value::Null => None,
                value => Some(
                    value
                        .as_u64()
                        .filter(|id| *id > 0 && *id <= isize::MAX as u64)
                        .ok_or("窗口 ID 无效")? as usize,
                ),
            };
            if requested != self.target {
                return Err("切换目标窗口前请先关闭当前控制会话，再使用 windowId 连接".into());
            }
        }
        Ok(())
    }
    pub fn state(&self) -> Value {
        let (w, h) = self.observed.map(|(w, h, _)| (w, h)).unwrap_or((0, 0));
        let title = self.target.map(|target| {
            let mut text = [0u16; 4097];
            let length =
                unsafe { GetWindowTextW(target as _, text.as_mut_ptr(), text.len() as i32) };
            String::from_utf16_lossy(&text[..length.max(0) as usize])
        });
        let foreground = self
            .target
            .map(|target| unsafe { GetForegroundWindow() } as usize == target);
        json!({"title":"本机桌面","targetTitle":title,"foreground":foreground,"connected":true,"interactive":self.observed.is_some(),"phase":if self.observed.is_some(){"ready"}else{"waiting-for-frame"},"viewport":{"width":w,"height":h},"windowId":self.target,"controlId":self.control_id,"manualInterventionAvailable":self.intervention.available,"escapeAvailable":self.intervention.available,"controlDiagnostics":control_diagnostics()})
    }
    pub fn control_mode(&mut self, manual: bool) -> Result<(), String> {
        self.paused.store(true, Ordering::SeqCst);
        self.observed = None;
        self.release_inputs()?;
        self.capture.invalidate()?;
        if let Ok(mut diagnostics) = CONTROL_DIAGNOSTICS.lock() {
            diagnostics.mode_change = if manual {
                "gui-takeover"
            } else {
                "gui-resume-agent"
            };
            if !manual {
                diagnostics.takeover = None;
            }
        }
        self.paused
            .store(manual || !self.intervention.available, Ordering::SeqCst);
        Ok(())
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
    fn input_allowed(&self, human: bool) -> Result<(), String> {
        self.allowed(human)?;
        if self
            .target
            .is_some_and(|target| unsafe { GetForegroundWindow() } as usize != target)
        {
            return Err("COMPUTER_USE_WINDOW_NOT_FOCUSED: 目标窗口不在前台，请使用 focus_window 聚焦并重新观察".into());
        }
        Ok(())
    }
    fn pointer_allowed(&self, human: bool, x: i32, y: i32) -> Result<(), String> {
        self.input_allowed(human)?;
        validate_pointer_target(self.target, |target| unsafe {
            let hit = WindowFromPoint(POINT { x, y });
            let root = GetAncestor(hit, GA_ROOT);
            (root as usize, IsWindowEnabled(target as _) != 0)
        })
    }
    fn caption_info(&self, target: usize) -> Result<TITLEBARINFO, String> {
        unsafe {
            if self.target != Some(target)
                || IsWindowVisible(target as _) == 0
                || IsWindowEnabled(target as _) == 0
                || IsIconic(target as _) != 0
                || IsHungAppWindow(target as _) != 0
                || GetAncestor(target as _, GA_ROOT) as usize != target
                || GetWindowLongPtrW(target as _, GWL_STYLE) as u32 & WS_CAPTION != WS_CAPTION
                || GetWindowLongPtrW(target as _, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE != 0
            {
                return Err(
                    "COMPUTER_USE_WINDOW_NOT_FOCUSED: 目标窗口未提供可操作的普通标题栏".into(),
                );
            }
            let mut info = TITLEBARINFO {
                cbSize: std::mem::size_of::<TITLEBARINFO>() as u32,
                ..Default::default()
            };
            if GetTitleBarInfo(target as _, &mut info) == 0
                || caption_coordinates(info.rcTitleBar, info.rgstate[0]).is_empty()
            {
                return Err("COMPUTER_USE_WINDOW_NOT_FOCUSED: 无法验证目标窗口的系统标题栏".into());
            }
            Ok(info)
        }
    }
    fn caption_point(&self, target: usize, x: i32, y: i32) -> bool {
        let Ok(info) = self.caption_info(target) else {
            return false;
        };
        let rect = info.rcTitleBar;
        if x < rect.left || x >= rect.right || y < rect.top || y >= rect.bottom {
            return false;
        }
        let Some(position) = caption_lparam(x, y) else {
            return false;
        };
        unsafe {
            let hit = WindowFromPoint(POINT { x, y });
            let root = GetAncestor(hit, GA_ROOT);
            if hit as usize != target || root as usize != target {
                return false;
            }
            let mut result = 0usize;
            let received = SendMessageTimeoutW(
                target as _,
                WM_NCHITTEST,
                0,
                position,
                SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                100,
                &mut result,
            );
            let hit = WindowFromPoint(POINT { x, y });
            let root = GetAncestor(hit, GA_ROOT);
            verified_caption(
                target,
                hit as usize,
                root as usize,
                IsWindowEnabled(target as _) != 0,
                (received != 0).then_some(result),
            )
        }
    }
    fn wait_foreground(
        &self,
        target: usize,
        human: bool,
        duration: Duration,
    ) -> Result<bool, String> {
        let deadline = Instant::now() + duration;
        loop {
            self.allowed(human)?;
            if unsafe { GetForegroundWindow() } as usize == target {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn activate_caption(&mut self, target: usize, human: bool) -> Result<(), String> {
        self.allowed(human)?;
        self.caption_info(target)?;
        // A repeated activation must not become a caption double-click/maximize.
        if let Some(previous) = self.last_caption_click {
            let interval = Duration::from_millis(
                u64::from(unsafe { GetDoubleClickTime() }.clamp(1, 5000)) + 25,
            );
            while previous.elapsed() < interval {
                self.allowed(human)?;
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.allowed(human)?;
        self.caption_info(target)?;
        unsafe {
            // Keep the normal Z-order group and never activate by this operation.
            SetWindowPos(
                target as _,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS,
            );
        }
        let deadline = Instant::now() + Duration::from_millis(600);
        let point = loop {
            self.allowed(human)?;
            let info = self.caption_info(target)?;
            let mut found = None;
            for (x, y) in caption_coordinates(info.rcTitleBar, info.rgstate[0]) {
                self.allowed(human)?;
                if self.caption_point(target, x, y) {
                    found = Some((x, y));
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
            }
            if let Some(point) = found {
                break point;
            }
            if Instant::now() >= deadline {
                return Err(
                    "COMPUTER_USE_WINDOW_NOT_FOCUSED: 标题栏被遮挡或未返回HTCAPTION，未发送点击"
                        .into(),
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        self.allowed(human)?;
        if !activation_input_idle() {
            return Err(
                "COMPUTER_USE_WINDOW_NOT_FOCUSED: 存在未释放的物理按键，未发送标题栏点击".into(),
            );
        }
        if !self.caption_point(target, point.0, point.1) {
            return Err("COMPUTER_USE_WINDOW_NOT_FOCUSED: 标题栏命中已变化，未发送点击".into());
        }
        self.allowed(human)?;
        input::move_to(point.0, point.1)?;
        self.allowed(human)?;
        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) } == 0
            || (i64::from(cursor.x) - i64::from(point.0)).abs() > 1
            || (i64::from(cursor.y) - i64::from(point.1)).abs() > 1
            || !self.caption_point(target, cursor.x, cursor.y)
        {
            return Err("COMPUTER_USE_WINDOW_NOT_FOCUSED: 标题栏命中已变化，未发送点击".into());
        }
        self.allowed(human)?;
        if !activation_input_idle() {
            return Err(
                "COMPUTER_USE_WINDOW_NOT_FOCUSED: 存在未释放的物理按键，未发送标题栏点击".into(),
            );
        }
        let mut verified_cursor = POINT::default();
        if unsafe { GetCursorPos(&mut verified_cursor) } == 0
            || verified_cursor.x != cursor.x
            || verified_cursor.y != cursor.y
        {
            return Err("COMPUTER_USE_WINDOW_NOT_FOCUSED: 游标已移动，未发送标题栏点击".into());
        }
        self.remember_input(input::HeldInput::Button("left".into()));
        self.last_caption_click = Some(Instant::now());
        let down = input::button("left", true);
        let up = self.held.release(
            &input::HeldInput::Button("left".into()),
            input::release_held,
        );
        down?;
        up?;
        Ok(())
    }
    fn snapshot(&mut self, human: bool) -> Result<Value, String> {
        self.allowed(human)?;
        let rect = bounds(self.target, self.monitor)?;
        let paused = self.paused.clone();
        let cancelled = self.cancelled.clone();
        let input_generation = self.input_generation.clone();
        let generation = self.active_input_generation;
        let result = self.capture.read(
            || {
                !cancelled.load(Ordering::SeqCst)
                    && input_generation.load(Ordering::SeqCst) == generation
                    && (human || !paused.load(Ordering::SeqCst))
                    && desktop_available()
            },
            Duration::from_secs(3),
        );
        let frame = match result {
            Ok(frame) => frame,
            Err(error) => {
                if cancelled.load(Ordering::SeqCst) {
                    return Err("COMPUTER_USE_ABORTED".into());
                }
                if input_generation.load(Ordering::SeqCst) != generation {
                    return Err("COMPUTER_USE_CAPTURE_INTERRUPTED".into());
                }
                self.observed = None;
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
    fn settle(&self, args: &Value, human: bool, default_ms: u64) -> Result<(), String> {
        let ms = requested_wait_ms(args, default_ms)?;
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            self.allowed(human)?;
            if self.input_generation.load(Ordering::SeqCst) != self.active_input_generation {
                return Err("COMPUTER_USE_CAPTURE_INTERRUPTED".into());
            }
            std::thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            );
        }
        Ok(())
    }
    pub fn act(
        &mut self,
        args: &Value,
        human: bool,
        cancelled: Arc<AtomicBool>,
        generation: u64,
    ) -> Result<Value, String> {
        self.cancelled = cancelled;
        self.active_input_generation = generation;
        self.poll();
        let result = self.act_inner(args, human);
        if result.is_err() {
            let _ = self.capture.invalidate();
        }
        if result.as_ref().is_err_and(|error| {
            !matches!(
                error.as_str(),
                "COMPUTER_USE_CAPTURE_INTERRUPTED"
                    | "COMPUTER_USE_STALE_CONTROL"
                    | "COMPUTER_USE_HUMAN_REQUIRED"
            )
        }) {
            let _ = self.release_inputs();
        }
        result
    }
    fn act_inner(&mut self, args: &Value, human: bool) -> Result<Value, String> {
        let action = args["action"].as_str().ok_or("缺少操作")?;
        requested_wait_ms(args, 0)?;
        let direct = matches!(
            action,
            "mouse_move" | "mouse_down" | "mouse_up" | "key_down" | "key_up" | "release_inputs"
        );
        if direct {
            input::validate_direct_control(human, args["controlId"].as_str(), &self.control_id)?;
            // Release edges must remain usable after focus, layout or desktop changes.
            match action {
                "release_inputs" => {
                    self.release_inputs()?;
                    self.capture.invalidate()?;
                    return Ok(json!({"state":self.state()}));
                }
                "key_up" => {
                    let code = input::key_code(args["key"].as_str().ok_or("缺少按键")?)?;
                    self.held
                        .release(&input::HeldInput::Key(code), input::release_held)?;
                    self.capture.invalidate()?;
                    return Ok(json!({"state":self.state()}));
                }
                "mouse_up" => {
                    let button = args["button"].as_str().unwrap_or("left");
                    self.held.release(
                        &input::HeldInput::Button(button.into()),
                        input::release_held,
                    )?;
                    self.capture.invalidate()?;
                    return Ok(json!({"state":self.state()}));
                }
                _ => {}
            }
        } else if args.get("controlId").is_some()
            && args["controlId"].as_str() != Some(self.control_id.as_str())
        {
            return Err("COMPUTER_USE_STALE_CONTROL".into());
        }
        self.allowed(human)?;
        if action == "status" {
            return Ok(json!({"state":self.state()}));
        }
        if action == "capture" {
            self.settle(args, human, 0)?;
            return self.snapshot(human);
        }
        if action == "list_windows" {
            return window_targets();
        }
        if action == "focus_window" {
            let target = self
                .target
                .ok_or("请先选择目标窗口并重新连接，再聚焦窗口")?;
            self.observed = None;
            self.release_inputs()?;
            self.capture.invalidate()?;
            bounds(Some(target), self.monitor)?;
            if unsafe { IsWindowVisible(target as _) } == 0
                || unsafe { IsWindowEnabled(target as _) } == 0
            {
                return Err("目标窗口不可见或已禁用，请人工恢复后重试".into());
            }
            self.allowed(human)?;
            let mut method = "foreground";
            if unsafe { GetForegroundWindow() } as usize != target {
                unsafe {
                    SetForegroundWindow(target as _);
                }
                if !self.wait_foreground(target, human, Duration::from_millis(500))? {
                    self.activate_caption(target, human)?;
                    method = "caption-click";
                    if !self.wait_foreground(target, human, Duration::from_millis(500))? {
                        return Err(
                            "COMPUTER_USE_WINDOW_NOT_FOCUSED: 已验证标题栏点击后窗口仍未激活"
                                .into(),
                        );
                    }
                }
            }
            self.capture.invalidate()?;
            let mut snapshot = self.snapshot(human)?;
            self.input_allowed(human)?;
            snapshot["focusMethod"] = json!(method);
            return Ok(snapshot);
        }
        let (w, h, rect) = self.observed.ok_or("请先观察桌面画面，再发送输入")?;
        if bounds(self.target, self.monitor)? != rect {
            self.observed = None;
            return Err("桌面布局已变化，请重新截图".into());
        }
        if human {
            self.paused.store(true, Ordering::SeqCst)
        }
        self.input_allowed(human)?;
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
            "mouse_move" | "mouse_down" => {
                let (x, y) = coordinate("x", "y")?;
                let button = args["button"].as_str().unwrap_or("left");
                if action == "mouse_down"
                    && !matches!(button, "left" | "right" | "middle" | "back" | "forward")
                {
                    return Err("不支持的鼠标按键".into());
                }
                self.pointer_allowed(human, x, y)?;
                input::move_to(x, y)?;
                if action == "mouse_down" {
                    self.pointer_allowed(human, x, y)?;
                    self.remember_input(input::HeldInput::Button(button.into()));
                    input::button(button, true)?;
                    self.held_foreground = Some(unsafe { GetForegroundWindow() } as usize);
                } else if !self.held.is_empty() {
                    self.held_deadline = Some(Instant::now() + Duration::from_secs(5));
                }
            }
            "key_down" => {
                let code = input::key_code(args["key"].as_str().ok_or("缺少按键")?)?;
                self.remember_input(input::HeldInput::Key(code));
                input::key(code, false)?;
            }
            "click" | "double_click" => {
                let (x, y) = coordinate("x", "y")?;
                let button = args["button"].as_str().unwrap_or("left");
                if !matches!(button, "left" | "right" | "middle" | "back" | "forward") {
                    return Err("不支持的鼠标按键".into());
                }
                self.pointer_allowed(human, x, y)?;
                input::move_to(x, y)?;
                for _ in 0..if action == "double_click" { 2 } else { 1 } {
                    self.pointer_allowed(human, x, y)?;
                    let held = input::HeldInput::Button(button.into());
                    self.remember_input(held.clone());
                    let down = input::button(button, true);
                    let up = self.held.release(&held, input::release_held);
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
                    self.input_allowed(human)?;
                    let held = input::HeldInput::Unicode(unit);
                    self.remember_input(held.clone());
                    let down = input::unicode(unit, false);
                    let up = self.held.release(&held, input::release_held);
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
                        self.input_allowed(human)?;
                        held.push(key);
                        self.remember_input(input::HeldInput::Key(key));
                        input::key(key, false)?;
                    }
                    Ok::<(), String>(())
                })();
                let mut released = Ok(());
                for key in held.into_iter().rev() {
                    if let Err(error) = self
                        .held
                        .release(&input::HeldInput::Key(key), input::release_held)
                    {
                        released = Err(error)
                    }
                }
                down?;
                released?;
            }
            "scroll" => {
                if args.get("x").is_some() || args.get("y").is_some() {
                    let (x, y) = coordinate("x", "y")?;
                    self.pointer_allowed(human, x, y)?;
                    input::move_to(x, y)?;
                    self.pointer_allowed(human, x, y)?;
                }
                input::scroll(
                    args["deltaX"].as_f64().unwrap_or(0.0),
                    args["deltaY"].as_f64().unwrap_or(0.0),
                )?;
            }
            "drag" => {
                let (x, y) = coordinate("x", "y")?;
                let (ex, ey) = coordinate("endX", "endY")?;
                self.pointer_allowed(human, x, y)?;
                input::move_to(x, y)?;
                self.pointer_allowed(human, x, y)?;
                self.remember_input(input::HeldInput::Button("left".into()));
                let pressed = input::button("left", true);
                let moved = (|| {
                    pressed?;
                    for n in 1..=12 {
                        let next_x = x + (ex - x) * n / 12;
                        let next_y = y + (ey - y) * n / 12;
                        self.pointer_allowed(human, next_x, next_y)?;
                        input::move_to(next_x, next_y)?;
                        std::thread::sleep(Duration::from_millis(16));
                    }
                    Ok::<(), String>(())
                })();
                let released = self.held.release(
                    &input::HeldInput::Button("left".into()),
                    input::release_held,
                );
                moved?;
                released?;
            }
            _ => return Err("不支持的本机桌面操作".into()),
        }
        self.capture.invalidate()?;
        if direct || args["includeScreenshot"] == false {
            return Ok(json!({"state":self.state()}));
        }
        self.settle(args, human, 100)?;
        self.snapshot(human)
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.paused.store(true, Ordering::SeqCst);
        let _ = self.release_inputs();
    }
}
#[cfg(test)]
mod diagnostics_tests {
    use super::*;
    #[test]
    fn activation_uses_only_visible_system_caption_coordinates() {
        let rect = RECT {
            left: -1900,
            top: -100,
            right: -1300,
            bottom: -70,
        };
        let points = caption_coordinates(rect, 0);
        assert_eq!(points.len(), 5);
        assert!(points.iter().all(|(x, y)| *x >= rect.left
            && *x < rect.right
            && *y >= rect.top
            && *y < rect.bottom));
        for state in [0x1, 0x8000, 0x10000] {
            assert!(caption_coordinates(rect, state).is_empty());
        }
        assert!(
            caption_coordinates(
                RECT {
                    right: rect.left,
                    ..rect
                },
                0
            )
            .is_empty()
        );
        assert!(
            caption_coordinates(
                RECT {
                    left: 40000,
                    right: 41000,
                    top: 0,
                    bottom: 30
                },
                0
            )
            .is_empty()
        );
    }
    #[test]
    fn caption_hit_test_refuses_controls_children_occlusion_and_timeouts() {
        assert!(verified_caption(42, 42, 42, true, Some(HTCAPTION as usize)));
        for hit in [
            HTCLIENT,
            HTCLOSE,
            HTMINBUTTON,
            HTMAXBUTTON,
            HTSYSMENU,
            HTBORDER,
        ] {
            assert!(!verified_caption(42, 42, 42, true, Some(hit as usize)));
        }
        assert!(!verified_caption(42, 42, 42, true, None));
        assert!(!verified_caption(
            42,
            43,
            42,
            true,
            Some(HTCAPTION as usize)
        ));
        assert!(!verified_caption(
            42,
            43,
            43,
            true,
            Some(HTCAPTION as usize)
        ));
        assert!(!verified_caption(
            42,
            42,
            42,
            false,
            Some(HTCAPTION as usize)
        ));
    }
    #[test]
    fn nonclient_hit_test_preserves_signed_multimonitor_coordinates() {
        for (x, y) in [(-1900, -100), (32767, -32768), (123, 456)] {
            let packed = caption_lparam(x, y).unwrap() as u32;
            assert_eq!(i32::from(packed as u16 as i16), x);
            assert_eq!(i32::from((packed >> 16) as u16 as i16), y);
        }
        assert!(caption_lparam(32768, 0).is_none());
        assert!(caption_lparam(0, -32769).is_none());
    }
    #[test]
    fn pointer_target_guard_accepts_only_the_bound_enabled_root() {
        assert!(validate_pointer_target(Some(42), |target| (target, true)).is_ok());
        assert!(validate_pointer_target(Some(42), |_| (43, true)).is_err());
        assert!(validate_pointer_target(Some(42), |_| (0, true)).is_err());
        assert!(validate_pointer_target(Some(42), |target| (target, false)).is_err());
    }
    #[test]
    fn whole_monitor_control_does_not_infer_a_window_target() {
        validate_pointer_target(None, |_| {
            panic!("whole-monitor mode does not inspect a bound target")
        })
        .unwrap();
    }
    #[test]
    fn wait_duration_rejects_malformed_values_before_input() {
        assert_eq!(requested_wait_ms(&json!({}), 100).unwrap(), 100);
        assert_eq!(requested_wait_ms(&json!({"waitMs":0}), 100).unwrap(), 0);
        assert_eq!(
            requested_wait_ms(&json!({"waitMs":10000}), 100).unwrap(),
            10000
        );
        for value in [
            json!(-1),
            json!(10001),
            json!(1.5),
            json!("100"),
            json!(null),
            json!(true),
        ] {
            assert!(requested_wait_ms(&json!({"waitMs":value}), 100).is_err());
        }
    }
    #[test]
    fn diagnostic_retains_first_takeover_after_own_cleanup_events() {
        let mut diagnostics = ControlDiagnostics::new();
        diagnostics.record("mouse", input::classify_hook(0, true), false);
        diagnostics.record(
            "keyboard",
            input::classify_hook(input::INPUT_MARKER, true),
            false,
        );
        let value = diagnostics.value();
        assert_eq!(value["takeover"]["device"], "mouse");
        assert_eq!(value["takeover"]["source"], "injected");
        assert_eq!(value["takeover"]["ownMarker"], false);
        assert_eq!(value["lastEvent"]["device"], "keyboard");
        assert_eq!(value["ownEvents"], 1);
        let encoded = value.to_string();
        for private_field in ["keyCode", "text", "coordinates", "extraInfo"] {
            assert!(!encoded.contains(private_field));
        }
    }
}
