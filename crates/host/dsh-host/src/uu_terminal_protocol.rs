//! Bounded parsing and explicitly addressed operations for the UU terminal CLI.
//!
//! A session inventory is usable only after a recognized table or an explicit
//! empty receipt. Opening a PTY alone does not establish a remote session, and
//! an inventory difference alone does not establish ownership.

use std::collections::BTreeSet;

const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_SESSIONS: usize = 1024;
const MAX_LINES: usize = 4096;
const CONNECTED: &str = "[Connect] Connected";
const ENTERED_TERMINAL: &str = "[Tips] Type 'exit' to end the remote session.";
// These localized literals are shipped by the inspected Windows CLI.
const CONNECTED_ZH: &str = "[连接] 连接成功";
const ENTERED_TERMINAL_ZH: &str = "[提示] 输入 'exit' 结束远程会话。";
const EMPTY_SESSIONS: &str = "No active sessions.";

fn protocol_error() -> String {
    "UU 终端返回了无法确认的协议数据".into()
}

/// Remove bounded CSI/OSC decoration without interpreting terminal commands.
/// Truncated escapes and other control characters are not valid receipts.
fn plain_text(text: &str) -> Result<String, String> {
    if text.len() > MAX_OUTPUT_BYTES {
        return Err("UU 终端返回数据超过大小限制".into());
    }
    let bytes = text.trim_start_matches('\u{feff}').as_bytes();
    let mut plain = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == 0x1b {
            let start = index;
            index += 1;
            match bytes.get(index) {
                Some(b'[') => {
                    index += 1;
                    let mut intermediates = false;
                    loop {
                        if index - start > 64 {
                            return Err(protocol_error());
                        }
                        let value = *bytes.get(index).ok_or_else(protocol_error)?;
                        index += 1;
                        match value {
                            0x40..=0x7e => break,
                            0x20..=0x2f => intermediates = true,
                            0x30..=0x3f if !intermediates => {}
                            _ => return Err(protocol_error()),
                        }
                    }
                }
                Some(b']') => {
                    index += 1;
                    loop {
                        if index - start > 4096 {
                            return Err(protocol_error());
                        }
                        match bytes.get(index) {
                            Some(0x07) => {
                                index += 1;
                                break;
                            }
                            Some(0x1b) if bytes.get(index + 1) == Some(&b'\\') => {
                                index += 2;
                                break;
                            }
                            Some(value) if *value >= 0x20 && *value != 0x7f => index += 1,
                            _ => return Err(protocol_error()),
                        }
                    }
                }
                _ => return Err(protocol_error()),
            }
        } else {
            if byte < 0x20 && !matches!(byte, b'\r' | b'\n' | b'\t') || byte == 0x7f {
                return Err(protocol_error());
            }
            plain.push(byte);
            index += 1;
        }
    }
    let plain = String::from_utf8(plain).map_err(|_| protocol_error())?;
    if plain
        .chars()
        .any(|value| value.is_control() && !matches!(value, '\r' | '\n' | '\t'))
    {
        return Err(protocol_error());
    }
    Ok(plain)
}

fn session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value.len() == 1 || !value.starts_with('0'))
        && value.parse::<u64>().is_ok()
}

fn opaque_cell(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

/// Parse the whitespace table headed SESSION_ID, SHELL, STATE, LAST_ACTIVE.
/// State and last-active values are display data, not lifecycle authority.
/// LAST_ACTIVE may contain spaces. Malformed rows, duplicate IDs and
/// unrecognized diagnostic rows fail.
pub(super) fn parse_sessions(stdout: &str) -> Result<BTreeSet<String>, String> {
    let text = plain_text(stdout)?;
    let mut sessions = BTreeSet::new();
    let mut header = false;
    let mut explicit_empty = false;
    let mut connected = false;
    for (index, raw) in text.split(['\r', '\n']).enumerate() {
        if index >= MAX_LINES || raw.len() > 4096 {
            return Err("UU 终端会话列表超过大小限制".into());
        }
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line, CONNECTED | CONNECTED_ZH) && !connected && !header && !explicit_empty {
            connected = true;
            continue;
        }
        if line == EMPTY_SESSIONS && !explicit_empty && sessions.is_empty() {
            explicit_empty = true;
            continue;
        }
        let columns: Vec<_> = line.split_ascii_whitespace().collect();
        if columns == ["SESSION_ID", "SHELL", "STATE", "LAST_ACTIVE"] {
            if header || explicit_empty {
                return Err(protocol_error());
            }
            header = true;
            continue;
        }
        if !header || explicit_empty || columns.len() < 4 {
            return Err(protocol_error());
        }
        if !session_id(columns[0])
            || !matches!(columns[1], "powershell" | "cmd" | "zsh" | "bash")
            || !opaque_cell(columns[2], 128)
            || !opaque_cell(&columns[3..].join(" "), 256)
        {
            return Err(protocol_error());
        }
        if sessions.len() >= MAX_SESSIONS || !sessions.insert(columns[0].to_string()) {
            return Err(protocol_error());
        }
    }
    if !explicit_empty && (!header || sessions.is_empty()) {
        return Err(protocol_error());
    }
    Ok(sessions)
}

/// The caller supplies the bounded, accumulated startup output, including
/// fragments received in earlier PTY reads.
pub(super) fn ready(text: &str) -> bool {
    let Ok(text) = plain_text(text) else {
        return false;
    };
    if blocked_plain_startup(&text) || connection_taken_over(&text) {
        return false;
    }
    let mut connected = false;
    for line in text.split(['\r', '\n']).map(str::trim) {
        if matches!(line, CONNECTED | CONNECTED_ZH) {
            connected = true;
        } else if matches!(line, ENTERED_TERMINAL | ENTERED_TERMINAL_ZH) && connected {
            return true;
        }
    }
    false
}

pub(super) fn connection_taken_over(text: &str) -> bool {
    const TAKEN_OVER: &str = "Terminal session attached from another window (code 2001)";
    text.len() <= MAX_OUTPUT_BYTES
        && plain_text(text).map_or_else(
            |_| text.contains(TAKEN_OVER),
            |plain| plain.contains(TAKEN_OVER),
        )
}

/// A known initial Windows shell prompt; continuation/nested prompts and
/// arbitrary terminal applications are not a command boundary for cleanup.
pub(super) fn shell_prompt(text: &str, shell: crate::uu_cli::TerminalShell) -> Option<String> {
    let plain = plain_text(text).ok()?;
    let line = plain.rsplit('\n').next()?.trim_matches(['\r', ' ']);
    if line.len() > 512 || line.ends_with(">>") {
        return None;
    }
    let path = match shell {
        crate::uu_cli::TerminalShell::PowerShell => line.strip_prefix("PS ")?.strip_suffix('>')?,
        crate::uu_cli::TerminalShell::Cmd => line.strip_suffix('>')?,
        _ => return None,
    };
    let bytes = path.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == b":\\")
        .then(|| line.to_string())
}

fn blocked_plain_startup(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "[system] unlocking",
        "unlock password",
        "device is locked",
        "not logged in",
        "not logged-in",
        "please log in",
        "please login",
        "login required",
        "login is required",
        "unsupported os",
        "unsupported operating system",
        "only supports windows",
        "only supported on windows",
        "仅支持 windows",
        "仅支持windows",
        "请先登录",
        "未登录",
        "已锁屏",
        "账户密码",
        "解锁密码",
        "正在解锁",
        "解锁失败",
        "请在被控设备上登录",
        "操作系统不支持远程终端",
        "与远端设备的连接已断开",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// A locked desktop or missing login is not an invitation to inject input.
/// A partial ANSI escape can be completed by the next PTY read; readiness
/// remains false until its complete handshake can be parsed.
pub(super) fn blocked_startup(text: &str) -> bool {
    if text.len() > MAX_OUTPUT_BYTES {
        return true;
    }
    plain_text(text)
        .map(|text| blocked_plain_startup(&text))
        .unwrap_or_else(|_| blocked_plain_startup(text))
}

pub(super) fn list_arguments(device: &str) -> Result<Vec<String>, String> {
    if !crate::uu_cli::valid_device_id(device) {
        return Err("UU 设备 ID 无效".into());
    }
    Ok(["term", "--device-id", device, "--list-sessions"]
        .into_iter()
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "SESSION_ID  SHELL  STATE  LAST_ACTIVE\n";

    fn ids(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn installed_chinese_connection_messages_are_supported_without_unlock_input() {
        let connected = format!("{CONNECTED_ZH}\r\n{ENTERED_TERMINAL_ZH}\r\n");
        assert!(ready(&connected));
        for blocked in [
            "[系统] 检测到被控端已锁屏，请输入被控端账户密码验证身份，此过程不会解锁被控端。",
            "[系统] 请输入被控端解锁密码: ",
            "[系统] 正在解锁...",
            "[系统] 被控设备操作系统不支持远程终端功能。",
        ] {
            assert!(blocked_startup(blocked));
            assert!(!ready(&(connected.clone() + blocked)));
        }
    }

    #[test]
    fn only_explicit_empty_is_an_empty_inventory() {
        assert!(
            parse_sessions("No active sessions.\r\n")
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_sessions(&format!("{HEADER}No active sessions.\n"))
                .unwrap()
                .is_empty()
        );
        for invalid in ["", "\n", HEADER, "[]", "No sessions", "private-value"] {
            let error = parse_sessions(invalid).unwrap_err();
            assert!(!error.contains("private-value"));
        }
    }

    #[test]
    fn accepts_bounded_table_rows_and_ansi_decoration() {
        let text = format!(
            "\u{feff}\x1b]0;UU terminal\x07\x1b[32m[Connect] Connected\x1b[0m\r\n{HEADER}\
             1  powershell  attached  2026-09-08 10:11:12\n\
             18446744073709551615  cmd  detached  2026-09-08T10:11:12Z\n"
        );
        assert_eq!(
            parse_sessions(&text).unwrap(),
            ids(&["1", "18446744073709551615"])
        );
        assert!(
            parse_sessions(
                "\x1b]8;;https://example.invalid\x1b\\No active sessions.\x1b]8;;\x1b\\"
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn rejects_ambiguous_or_unknown_inventory_rows() {
        for row in [
            "1 powershell active",
            "1 unknown active now",
            "--help powershell active now",
            "-1 powershell active now",
            "+1 powershell active now",
            "01 powershell active now",
            "18446744073709551616 cmd active now",
            "[Error] private-value",
            "1|powershell|active|now",
        ] {
            assert!(
                parse_sessions(&format!("{HEADER}{row}\n")).is_err(),
                "{row}"
            );
        }
        assert_eq!(
            parse_sessions(&format!("{HEADER}1 powershell 使用中 a few seconds ago\n")).unwrap(),
            ids(&["1"])
        );
        for text in [
            format!("{HEADER}1 cmd active now\n1 cmd active now"),
            format!("{HEADER}{HEADER}1 cmd active now"),
            format!("{HEADER}1 cmd active now\nNo active sessions."),
            format!("No active sessions.\n{HEADER}1 cmd active now"),
            format!("{HEADER}No active sessions.\nNo active sessions."),
        ] {
            assert!(parse_sessions(&text).is_err());
        }
    }

    #[test]
    fn inventories_and_escapes_have_fixed_memory_bounds() {
        assert!(parse_sessions(&"x".repeat(MAX_OUTPUT_BYTES + 1)).is_err());
        assert!(parse_sessions(&"\n".repeat(MAX_LINES + 1)).is_err());
        let mut table = HEADER.to_string();
        for id in 0..=MAX_SESSIONS {
            table.push_str(&format!("{id} cmd active now\n"));
        }
        assert!(parse_sessions(&table).is_err());
        for invalid in ["\x1b[32", "\x1b]title", "\x1bX", "\0", "\u{85}"] {
            assert!(parse_sessions(&format!("{invalid}No active sessions.")).is_err());
        }
    }

    #[test]
    fn readiness_requires_the_ordered_complete_handshake() {
        let mut output = String::new();
        for fragment in [
            "\x1b[32m[Con",
            "nect] Connected\x1b[0m\r\n",
            "[Tips] Type 'exit' to end the remote ses",
        ] {
            output.push_str(fragment);
            assert!(!ready(&output));
        }
        output.push_str("sion.\r\n");
        assert!(ready(&output));
        for end in 0..output.len() {
            let prefix = &output[..end];
            assert!(!blocked_startup(prefix));
            if !prefix.contains(ENTERED_TERMINAL) {
                assert!(!ready(prefix));
            }
        }
        for text in [
            CONNECTED.to_string(),
            ENTERED_TERMINAL.to_string(),
            format!("{ENTERED_TERMINAL}\n{CONNECTED}"),
            format!("echo {CONNECTED}\n{ENTERED_TERMINAL}"),
            "[Tips] Close the window to detach (session stays alive in background).".into(),
        ] {
            assert!(!ready(&text));
        }
    }

    #[test]
    fn locked_login_and_unsupported_startups_never_become_ready() {
        for blocker in [
            "[System] Unlocking...",
            "[Error] Please log in first",
            "[Error] Not logged in",
            "[Error] Unsupported OS",
            "Only supported on Windows",
            "请先登录 UU 远程",
        ] {
            let output = format!("{CONNECTED}\n{blocker}\n{ENTERED_TERMINAL}");
            assert!(blocked_startup(&output));
            assert!(!ready(&output));
        }
        assert!(!blocked_startup(&format!(
            "{CONNECTED}\n{ENTERED_TERMINAL}"
        )));
        assert!(!blocked_startup("\x1b["));
        assert!(!ready("\x1b["));
        assert!(blocked_startup("[System] Unlocking...\n\x1b["));
    }

    #[test]
    fn list_only_uses_the_explicit_validated_device() {
        assert_eq!(
            list_arguments("device_1").unwrap(),
            ["term", "--device-id", "device_1", "--list-sessions"]
        );
        for device in ["", "--all", "a b", "a\nb", "a;b", "a=b"] {
            assert!(list_arguments(device).is_err());
        }
    }
    #[test]
    fn takeover_is_not_readiness_and_prompt_boundaries_are_explicit() {
        let output = format!(
            "{CONNECTED}\n{ENTERED_TERMINAL}\nError: Terminal session attached from another window (code 2001)"
        );
        assert!(connection_taken_over(&output));
        assert!(!ready(&output));
        assert_eq!(
            shell_prompt("PS C:\\fixture> ", crate::uu_cli::TerminalShell::PowerShell),
            Some("PS C:\\fixture>".into())
        );
        for value in [
            "PS C:\\fixture>> ",
            ">> ",
            "Password: ",
            "application> ",
            "PS C:\\fixture>\n",
        ] {
            assert!(shell_prompt(value, crate::uu_cli::TerminalShell::PowerShell).is_none());
        }
    }
}
