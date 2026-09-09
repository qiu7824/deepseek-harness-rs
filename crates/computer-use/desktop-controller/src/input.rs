//! Windows input injection with coordinates derived from an observed frame.
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU8, Ordering};
use windows_sys::Win32::UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*};
// Keep the marker representable by both 32-bit and 64-bit ULONG_PTR transports.
pub const INPUT_MARKER: usize = 0x4453484e;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HookClassification {
    pub injected: bool,
    pub own_marker: bool,
    pub low32_marker_match: bool,
}
impl HookClassification {
    pub fn is_own(self) -> bool {
        self.injected && self.own_marker
    }
}
pub fn classify_hook(extra_info: usize, injected: bool) -> HookClassification {
    HookClassification {
        injected,
        own_marker: extra_info == INPUT_MARKER,
        low32_marker_match: extra_info as u32 == INPUT_MARKER as u32,
    }
}

static INJECTION_PHASE: AtomicU8 = AtomicU8::new(0);
static LAST_INJECTION_PHASE: AtomicU8 = AtomicU8::new(0);
fn phase_name(phase: u8) -> &'static str {
    match phase {
        1 => "mouse-move",
        2 => "mouse-down",
        3 => "mouse-up",
        4 => "scroll",
        5 => "key-down",
        6 => "key-up",
        7 => "text-down",
        8 => "text-up",
        _ => "idle",
    }
}
pub fn injection_phases() -> (&'static str, &'static str) {
    (
        phase_name(INJECTION_PHASE.load(Ordering::SeqCst)),
        phase_name(LAST_INJECTION_PHASE.load(Ordering::SeqCst)),
    )
}
struct InjectionPhase;
impl Drop for InjectionPhase {
    fn drop(&mut self) {
        INJECTION_PHASE.store(0, Ordering::SeqCst);
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HeldInput {
    Key(u16),
    Unicode(u16),
    Button(String),
}

#[derive(Default)]
pub struct HeldInputs(BTreeSet<HeldInput>);
impl HeldInputs {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn press(&mut self, input: HeldInput) {
        self.0.insert(input);
    }
    pub fn release(
        &mut self,
        input: &HeldInput,
        mut inject: impl FnMut(&HeldInput) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.0.contains(input) {
            inject(input)?;
            self.0.remove(input);
        }
        Ok(())
    }
    pub fn release_all(
        &mut self,
        mut inject: impl FnMut(&HeldInput) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut error = None;
        for input in self.0.clone() {
            if let Err(failure) = self.release(&input, &mut inject) {
                error.get_or_insert(failure);
            }
        }
        error.map_or(Ok(()), Err)
    }
}
pub fn release_held(input: &HeldInput) -> Result<(), String> {
    match input {
        HeldInput::Key(code) => key(*code, true),
        HeldInput::Unicode(unit) => unicode(*unit, true),
        HeldInput::Button(name) => button(name, false),
    }
}

pub fn validate_direct_control(
    human: bool,
    supplied: Option<&str>,
    current: &str,
) -> Result<(), String> {
    if !human {
        return Err("COMPUTER_USE_HUMAN_REQUIRED".into());
    }
    if supplied != Some(current) {
        return Err("COMPUTER_USE_STALE_CONTROL".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}
pub fn position(
    x: f64,
    y: f64,
    width: u32,
    height: u32,
    bounds: Bounds,
) -> Result<(i32, i32), String> {
    if width == 0
        || height == 0
        || bounds.width <= 0
        || bounds.height <= 0
        || !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= width as f64
        || y >= height as f64
    {
        return Err("坐标不在当前画面内，请重新观察".into());
    }
    Ok((
        bounds.left + (x * bounds.width as f64 / width as f64).floor() as i32,
        bounds.top + (y * bounds.height as f64 / height as f64).floor() as i32,
    ))
}
fn send(input: INPUT, phase: u8) -> Result<(), String> {
    LAST_INJECTION_PHASE.store(phase, Ordering::SeqCst);
    INJECTION_PHASE.store(phase, Ordering::SeqCst);
    let _phase = InjectionPhase;
    if unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) } != 1 {
        return Err("Windows 未接受输入，请检查目标窗口权限或人工接管".into());
    }
    Ok(())
}
fn mouse(dx: i32, dy: i32, data: u32, flags: u32) -> Result<(), String> {
    let phase = if flags & MOUSEEVENTF_MOVE != 0 {
        1
    } else if flags
        & (MOUSEEVENTF_LEFTDOWN
            | MOUSEEVENTF_RIGHTDOWN
            | MOUSEEVENTF_MIDDLEDOWN
            | MOUSEEVENTF_XDOWN)
        != 0
    {
        2
    } else if flags
        & (MOUSEEVENTF_LEFTUP | MOUSEEVENTF_RIGHTUP | MOUSEEVENTF_MIDDLEUP | MOUSEEVENTF_XUP)
        != 0
    {
        3
    } else {
        4
    };
    send(
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: INPUT_MARKER,
                },
            },
        },
        phase,
    )
}
pub fn move_to(x: i32, y: i32) -> Result<(), String> {
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if width <= 1 || height <= 1 || x < left || y < top || x >= left + width || y >= top + height {
        return Err("桌面布局已变化，请重新截图".into());
    }
    let dx = ((x - left) as f64 * 65535.0 / (width - 1) as f64).round() as i32;
    let dy = ((y - top) as f64 * 65535.0 / (height - 1) as f64).round() as i32;
    mouse(
        dx,
        dy,
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}
pub fn button(name: &str, down: bool) -> Result<(), String> {
    let flags = match (name, down) {
        ("left", true) => MOUSEEVENTF_LEFTDOWN,
        ("left", false) => MOUSEEVENTF_LEFTUP,
        ("right", true) => MOUSEEVENTF_RIGHTDOWN,
        ("right", false) => MOUSEEVENTF_RIGHTUP,
        ("middle", true) => MOUSEEVENTF_MIDDLEDOWN,
        ("middle", false) => MOUSEEVENTF_MIDDLEUP,
        ("back" | "forward", true) => MOUSEEVENTF_XDOWN,
        ("back" | "forward", false) => MOUSEEVENTF_XUP,
        _ => return Err("不支持的鼠标按键".into()),
    };
    let data = match name {
        "back" => 1,
        "forward" => 2,
        _ => 0,
    };
    mouse(0, 0, data, flags)
}
pub fn scroll(x: f64, y: f64) -> Result<(), String> {
    if !x.is_finite() || !y.is_finite() || x.abs() > 10000.0 || y.abs() > 10000.0 {
        return Err("滚动距离无效".into());
    }
    if x != 0.0 {
        mouse(0, 0, x.round() as i32 as u32, MOUSEEVENTF_HWHEEL)?;
    }
    if y != 0.0 {
        mouse(0, 0, (-y.round()) as i32 as u32, MOUSEEVENTF_WHEEL)?;
    }
    Ok(())
}
pub fn key(code: u16, up: bool) -> Result<(), String> {
    let extended = matches!(code, 33..=40 | 45 | 46 | 91..=93 | 111 | 144 | 163 | 165);
    send(
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: code,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }
                        | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 },
                    time: 0,
                    dwExtraInfo: INPUT_MARKER,
                },
            },
        },
        if up { 6 } else { 5 },
    )
}
pub fn unicode(unit: u16, up: bool) -> Result<(), String> {
    send(
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: INPUT_MARKER,
                },
            },
        },
        if up { 8 } else { 7 },
    )
}
pub fn key_code(name: &str) -> Result<u16, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => 17,
        "shift" => 16,
        "alt" => 18,
        "meta" | "win" | "windows" => 91,
        "enter" => 13,
        "tab" => 9,
        "escape" | "esc" => 27,
        "backspace" => 8,
        "delete" => 46,
        "space" => 32,
        "left" | "arrowleft" => 37,
        "up" | "arrowup" => 38,
        "right" | "arrowright" => 39,
        "down" | "arrowdown" => 40,
        "home" => 36,
        "end" => 35,
        "pageup" => 33,
        "pagedown" => 34,
        "insert" => 45,
        "capslock" => 20,
        "numlock" => 144,
        "scrolllock" => 145,
        "pause" => 19,
        "printscreen" => 44,
        "contextmenu" => 93,
        "controlleft" => 162,
        "controlright" => 163,
        "shiftleft" => 160,
        "shiftright" => 161,
        "altleft" => 164,
        "altright" => 165,
        "metaleft" => 91,
        "metaright" => 92,
        "semicolon" => 186,
        "equal" => 187,
        "comma" => 188,
        "minus" => 189,
        "period" => 190,
        "slash" => 191,
        "backquote" => 192,
        "bracketleft" => 219,
        "backslash" => 220,
        "bracketright" => 221,
        "quote" => 222,
        "numpadadd" => 107,
        "numpadsubtract" => 109,
        "numpadmultiply" => 106,
        "numpaddivide" => 111,
        "numpaddecimal" => 110,
        "numpadenter" => 13,
        value
            if value.starts_with("key")
                && value.len() == 4
                && value.as_bytes()[3].is_ascii_alphabetic() =>
        {
            u16::from(value.as_bytes()[3].to_ascii_uppercase())
        }
        value
            if value.starts_with("digit")
                && value.len() == 6
                && value.as_bytes()[5].is_ascii_digit() =>
        {
            u16::from(value.as_bytes()[5])
        }
        value
            if value.starts_with("numpad")
                && value.len() == 7
                && value.as_bytes()[6].is_ascii_digit() =>
        {
            96 + u16::from(value.as_bytes()[6] - b'0')
        }
        _ if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() => {
            name.as_bytes()[0].to_ascii_uppercase() as u16
        }
        _ if name.len() >= 2 && name.starts_with(['f', 'F']) => {
            let n = name[1..].parse::<u16>().map_err(|_| "按键无效")?;
            if !(1..=24).contains(&n) {
                return Err("按键无效".into());
            }
            111 + n
        }
        _ => return Err("不支持的键盘按键".into()),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hook_marker_survives_32_bit_transport_without_trusting_foreign_input() {
        assert_eq!(INPUT_MARKER, INPUT_MARKER as u32 as usize);
        assert!(classify_hook(INPUT_MARKER, true).is_own());
        assert!(!classify_hook(INPUT_MARKER, false).is_own());
        assert!(!classify_hook(0, false).is_own());
        assert!(!classify_hook(0, true).is_own());
        #[cfg(target_pointer_width = "64")]
        {
            let different = INPUT_MARKER | (1usize << 32);
            let event = classify_hook(different, true);
            assert!(event.low32_marker_match);
            assert!(!event.own_marker);
            assert!(!event.is_own());
            let old_marker = 0x4453484445534busize;
            assert_ne!(old_marker, old_marker as u32 as usize);
        }
    }
    #[test]
    fn observed_coordinates_map_to_physical_pixels() {
        let b = Bounds {
            left: -1920,
            top: 100,
            width: 1920,
            height: 1080,
        };
        assert_eq!(position(480.0, 270.0, 960, 540, b).unwrap(), (-960, 640));
        assert!(position(960.0, 0.0, 960, 540, b).is_err());
        assert!(position(f64::NAN, 0.0, 960, 540, b).is_err());
    }
    #[test]
    fn keyboard_names_are_validated_before_injection() {
        assert_eq!(key_code("Control").unwrap(), 17);
        assert_eq!(key_code("F12").unwrap(), 123);
        assert!(key_code("F25").is_err());
        assert_eq!(key_code("KeyZ").unwrap(), 90);
        assert_eq!(key_code("Digit7").unwrap(), 55);
        assert_eq!(key_code("Numpad7").unwrap(), 103);
        assert_eq!(key_code("ControlRight").unwrap(), 163);
        assert_eq!(key_code("BracketLeft").unwrap(), 219);
        assert!(key_code("Key1").is_err());
        assert!(key_code("Numpad10").is_err());
    }
    #[test]
    fn direct_input_requires_human_and_current_control_session() {
        assert!(validate_direct_control(false, Some("current"), "current").is_err());
        assert!(validate_direct_control(true, None, "current").is_err());
        assert!(validate_direct_control(true, Some("older"), "current").is_err());
        assert!(validate_direct_control(true, Some("current"), "current").is_ok());
    }
    #[test]
    fn release_attempts_every_held_input_and_retains_failed_releases() {
        let mut held = HeldInputs::default();
        held.press(HeldInput::Key(17));
        held.press(HeldInput::Key(17));
        held.press(HeldInput::Unicode(0x4e2d));
        held.press(HeldInput::Button("left".into()));
        let mut attempts = Vec::new();
        assert!(
            held.release_all(|input| {
                attempts.push(input.clone());
                if matches!(input, HeldInput::Key(17) | HeldInput::Unicode(0x4e2d)) {
                    Err("blocked".into())
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert_eq!(attempts.len(), 3);
        assert!(!held.is_empty());
        attempts.clear();
        held.release_all(|input| {
            attempts.push(input.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            attempts,
            vec![HeldInput::Key(17), HeldInput::Unicode(0x4e2d)]
        );
        assert!(held.is_empty());
        held.release(&HeldInput::Key(17), |_| {
            panic!("released input must not be repeated")
        })
        .unwrap();
        held.release(&HeldInput::Unicode(0x4e2d), |_| {
            panic!("released Unicode input must not be repeated")
        })
        .unwrap();
    }
}
