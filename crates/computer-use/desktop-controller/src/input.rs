//! Windows input injection with coordinates derived from an observed frame.
use windows_sys::Win32::UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*};
pub const INPUT_MARKER: usize = 0x4453484445534b;

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
fn send(input: INPUT) -> Result<(), String> {
    if unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) } != 1 {
        return Err("Windows 未接受输入，请检查目标窗口权限或人工接管".into());
    }
    Ok(())
}
fn mouse(dx: i32, dy: i32, data: u32, flags: u32) -> Result<(), String> {
    send(INPUT {
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
    })
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
        _ => return Err("不支持的鼠标按键".into()),
    };
    mouse(0, 0, 0, flags)
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
    let extended = matches!(code, 33..=40 | 45 | 46 | 91 | 92);
    send(INPUT {
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
    })
}
pub fn unicode(unit: u16, up: bool) -> Result<(), String> {
    send(INPUT {
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
    })
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
    }
}
