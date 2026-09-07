//! UU input envelopes. Coordinates are normalized against the observed image.
use serde_json::{Value, json};
pub fn mouse_move(x: f64, y: f64, width: u32, height: u32) -> Result<Value, String> {
    if width == 0
        || height == 0
        || !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= f64::from(width)
        || y >= f64::from(height)
    {
        return Err("坐标不在当前画面内，请重新观察画面".into());
    }
    Ok(
        json!({"action":"mouse_move_absolute","abs_x":x/f64::from(width),"abs_y":y/f64::from(height),"mousetype":0}),
    )
}
pub fn mouse_button(button: &str, down: bool) -> Result<Value, String> {
    let code = match button {
        "left" => 1,
        "right" => 2,
        "middle" => 4,
        _ => return Err("不支持的鼠标按键".into()),
    };
    Ok(json!({"action":if down{"mouse_press"}else{"mouse_release"},"button":code,"mousetype":0}))
}
pub fn wheel(x: f64, y: f64) -> Result<Value, String> {
    if !x.is_finite() || !y.is_finite() || x.abs() > 10000.0 || y.abs() > 10000.0 {
        return Err("滚动距离无效".into());
    }
    Ok(
        json!({"action":"mouse_scroll","delta_x":x.round()as i32,"delta_y":-y.round()as i32,"mousetype":0}),
    )
}
pub fn keyboard(code: u32, up: bool) -> Result<Value, String> {
    if code == 0 || code > 255 {
        return Err("无效的键盘按键".into());
    }
    Ok(json!({"action":if up{"kbd_release"}else{"kbd_press"},"key":code}))
}
pub fn key_code(name: &str) -> Result<u32, String> {
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
            u32::from(value.as_bytes()[3].to_ascii_uppercase())
        }
        value
            if value.starts_with("digit")
                && value.len() == 6
                && value.as_bytes()[5].is_ascii_digit() =>
        {
            u32::from(value.as_bytes()[5])
        }
        value
            if value.starts_with("numpad")
                && value.len() == 7
                && value.as_bytes()[6].is_ascii_digit() =>
        {
            96 + u32::from(value.as_bytes()[6] - b'0')
        }
        value if value.starts_with('f') && (2..=3).contains(&value.len()) => {
            let index = value[1..].parse::<u32>().map_err(|_| "无效的功能键")?;
            if !(1..=24).contains(&index) {
                return Err("无效的功能键".into());
            }
            111 + index
        }
        _ if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() => {
            u32::from(name.as_bytes()[0].to_ascii_uppercase())
        }
        _ => return Err("不支持的键盘按键".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observed_coordinates_are_required() {
        assert!(mouse_move(1.0, 2.0, 0, 540).is_err());
        assert!(mouse_move(f64::NAN, 1.0, 960, 540).is_err());
        assert!(mouse_move(960.0, 1.0, 960, 540).is_err());
        let p = mouse_move(480.0, 270.0, 960, 540).unwrap();
        assert_eq!(p["abs_x"], 0.5);
        assert_eq!(p["abs_y"], 0.5);
    }
    #[test]
    fn release_envelopes_match_the_pressed_input() {
        assert_eq!(
            mouse_button("left", false).unwrap()["action"],
            "mouse_release"
        );
        assert_eq!(
            keyboard(key_code("Control").unwrap(), true).unwrap()["key"],
            17
        );
        assert!(keyboard(0, false).is_err());
    }
    #[test]
    fn dom_physical_keys_include_modifiers_and_punctuation() {
        for (name, code) in [
            ("KeyF", 70),
            ("f", 70),
            ("Digit2", 50),
            ("ControlLeft", 162),
            ("ControlRight", 163),
            ("ShiftLeft", 160),
            ("AltRight", 165),
            ("F24", 135),
            ("Numpad1", 97),
            ("Quote", 222),
            ("Backslash", 220),
        ] {
            assert_eq!(key_code(name).unwrap(), code, "{name}");
        }
        assert!(key_code("F25").is_err());
        assert!(keyboard(65, false).unwrap().get("interrept").is_none());
    }
}
