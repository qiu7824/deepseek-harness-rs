//! Documented UU CLI arguments and bounded local transport.
//!
//! Desktop frames and pointer/keyboard injection belong to the desktop
//! adapters. The vendor CLI exposes an interactive remote terminal, not a
//! stateless command, screenshot, or mouse-control API.
use dsh_native_command::{NativeCommandAbort, NativeCommandLimits};
use serde_json::Value;
use std::{path::Path, time::Duration};

pub(crate) const DOCUMENTATION: &str = "https://uuyc.163.com/help/cli.html";

pub(crate) fn valid_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command<'a> {
    UserInfo,
    DeviceList,
    DeviceStatus,
    Connect(&'a str),
    Disconnect(&'a str),
}

impl Command<'_> {
    pub(crate) fn arguments(&self) -> Result<Vec<String>, String> {
        let words = match self {
            Self::UserInfo => vec!["user", "info"],
            Self::DeviceList => vec!["device", "list"],
            Self::DeviceStatus => vec!["device", "status"],
            Self::Connect(id) | Self::Disconnect(id) => {
                if !valid_device_id(id) {
                    return Err("UU 设备 ID 无效".into());
                }
                vec![
                    "device",
                    if matches!(self, Self::Connect(_)) {
                        "connect"
                    } else {
                        "disconnect"
                    },
                    id,
                ]
            }
        };
        Ok(words.into_iter().map(str::to_string).collect())
    }
}

pub(crate) fn failure_message(code: Option<&str>) -> String {
    match code {
        Some("ENOENT") => "未找到 UU 远程 CLI，请检查安装路径".into(),
        Some("1") => "UU 远程配置不可用，请在客户端检查设置".into(),
        Some("2") => "请打开 UU 远程客户端并登录账号".into(),
        Some("3") => "当前 UU 远程版本不支持该命令，请更新客户端".into(),
        Some("4") => "UU 远程数据暂不可用，请检查账号与设备状态".into(),
        Some("5" | "TIMEOUT") => "UU 远程终端环境检查超时，请确认被控端已登录、在线并支持远程终端后重试".into(),
        Some("6") => "UU 远程终端操作失败".into(),
        Some("ABORT_ERR") => "UU 远程操作已取消".into(),
        Some("OUTPUT_LIMIT") => "UU 远程返回数据超过大小限制".into(),
        _ => "UU 远程操作失败，请检查客户端状态".into(),
    }
}

pub(crate) fn parse_response(stdout: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(stdout.trim_start_matches('\u{feff}').trim())
        .map_err(|_| "UU 远程没有返回有效 JSON，请更新官方客户端".to_string())?;
    if value.get("success") != Some(&Value::Bool(true)) {
        // Vendor diagnostics can include local account or connection data.
        // Do not forward arbitrary stdout/stderr or an untyped error object.
        return Err("UU 远程未完成操作，请在客户端检查登录与设备状态".into());
    }
    value
        .get("data")
        .filter(|data| data.is_object())
        .cloned()
        .ok_or_else(|| "UU 远程返回的数据格式无效".into())
}

pub(crate) async fn invoke(
    cli: &Path,
    command: Command<'_>,
    signal: Option<NativeCommandAbort>,
) -> Result<Value, String> {
    let output = dsh_native_command::run_native_command_bounded(
        &cli.to_string_lossy(),
        &command.arguments()?,
        signal,
        NativeCommandLimits {
            timeout: Duration::from_secs(12),
            stdout_bytes: 1024 * 1024,
            stderr_bytes: 8192,
        },
    )
    .await
    .map_err(|error| failure_message(error.code.as_deref()))?;
    parse_response(&output.stdout)
}

pub(crate) fn parse_version(stdout: &str) -> Result<String, String> {
    let text = stdout.trim_start_matches('\u{feff}').trim();
    let parts: Vec<_> = text.split('.').collect();
    if (2..=4).contains(&parts.len())
        && text.len() <= 64
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        Ok(text.to_string())
    } else {
        Err("UU 远程没有返回有效版本信息".into())
    }
}

pub(crate) async fn version(cli: &Path) -> Result<String, String> {
    let output = dsh_native_command::run_native_command_bounded(
        &cli.to_string_lossy(),
        &["--version".to_string()],
        None,
        NativeCommandLimits {
            timeout: Duration::from_secs(4),
            stdout_bytes: 1024,
            stderr_bytes: 1024,
        },
    )
    .await
    .map_err(|error| failure_message(error.code.as_deref()))?;
    parse_version(&output.stdout)
}

/// `device status` returns `connected_devices`, unlike a connect receipt's
/// `devices` list. Unknown rows remain an error, never an empty success.
pub(crate) fn connected_devices(data: &Value) -> Result<Vec<String>, String> {
    let rows = data
        .get("connected_devices")
        .and_then(Value::as_array)
        .ok_or("UU 连接状态格式无效")?;
    if rows.len() > 256 {
        return Err("UU 连接状态超过大小限制".into());
    }
    let mut ids = Vec::new();
    for row in rows {
        let id = row.as_str().or_else(|| {
            ["deviceId", "targetId", "device_id"]
                .iter()
                .find_map(|field| row.get(*field).and_then(Value::as_str))
        });
        let Some(id) = id.filter(|id| valid_device_id(id)) else {
            return Err("UU 连接状态包含无法识别的设备".into());
        };
        if !ids.iter().any(|known| known == id) {
            ids.push(id.to_string());
        }
    }
    Ok(ids)
}

/// Check the account-scoped local identity without returning login tokens.
/// The numeric `-d` assist ID is a different identifier and is not used here.
#[cfg(any(windows, test))]
fn parse_local_identity(text: &str, expected_user: &str) -> Option<String> {
    if text.len() > 1024 * 1024 || expected_user.is_empty() {
        return None;
    }
    let mut general = false;
    let mut user = None;
    let mut device = None;
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.starts_with('[') {
            general = line == "[General]";
            continue;
        }
        if !general {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match name.trim() {
            "userId" if user.is_none() => user = Some(value),
            "deviceId" if device.is_none() => device = Some(value),
            "userId" | "deviceId" => return None,
            _ => {}
        }
    }
    (user == Some(expected_user))
        .then_some(device?)
        .filter(|id| valid_device_id(id))
        .map(str::to_string)
}

pub(crate) fn local_device_id(expected_user: &str) -> Option<String> {
    #[cfg(windows)]
    {
        use std::io::Read;

        let path = std::path::PathBuf::from(std::env::var_os("ProgramData")?)
            .join("Netease/GameViewer/user_info.ini");
        let file = std::fs::File::open(path).ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() > 1024 * 1024 {
            return None;
        }
        let mut text = String::new();
        file.take(1024 * 1024 + 1).read_to_string(&mut text).ok()?;
        parse_local_identity(&text, expected_user)
    }
    #[cfg(not(windows))]
    {
        let _ = expected_user;
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalShell {
    PowerShell,
    Cmd,
    Zsh,
    Bash,
}

impl TerminalShell {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "powershell" => Ok(Self::PowerShell),
            "cmd" => Ok(Self::Cmd),
            "zsh" => Ok(Self::Zsh),
            "bash" => Ok(Self::Bash),
            _ => Err("UU 终端 Shell 类型无效".into()),
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::PowerShell => "powershell",
            Self::Cmd => "cmd",
            Self::Zsh => "zsh",
            Self::Bash => "bash",
        }
    }
}

/// Arguments only: the caller must supply an interactive PTY and own the
/// remote session lifecycle. Running this through captured one-shot stdio is
/// not a remote-exec implementation.
pub(crate) fn new_terminal_arguments(
    device_id: &str,
    shell: TerminalShell,
) -> Result<Vec<String>, String> {
    if !valid_device_id(device_id) {
        return Err("UU 设备 ID 无效".into());
    }
    Ok([
        "term",
        "--device-id",
        device_id,
        "--shell",
        shell.as_str(),
        "--new-session",
    ]
    .into_iter()
    .map(str::to_string)
    .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn structured_queries_and_version_use_the_observed_protocols() {
        assert_eq!(
            parse_version("\u{feff}4.38.3.9325\r\n").unwrap(),
            "4.38.3.9325"
        );
        assert!(parse_version("4.38.3.9325\nprivate-value").is_err());
        assert_eq!(
            parse_response("\u{feff}{\"success\":true,\"data\":{\"devices\":[]}}").unwrap(),
            json!({"devices":[]})
        );
        for value in [
            "{}",
            "{\"success\":true}",
            "{\"success\":true,\"data\":[]}",
            "{\"success\":false,\"error\":\"private-value\"}",
        ] {
            let error = parse_response(value).unwrap_err();
            assert!(!error.contains("private-value"));
        }
    }

    #[test]
    fn status_uses_connected_devices_and_rejects_unknown_rows() {
        assert_eq!(
            connected_devices(&json!({"connected_devices":[]})).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            connected_devices(
                &json!({"connected_devices":["one",{"deviceId":"two"},{"targetId":"one"}]})
            )
            .unwrap(),
            vec!["one", "two"]
        );
        assert!(connected_devices(&json!({"devices":[]})).is_err());
        assert!(
            connected_devices(&json!({"connected_devices":[{"token":"private-value"}]})).is_err()
        );
    }

    #[test]
    fn mutations_always_address_one_literal_device() {
        assert_eq!(
            Command::Disconnect("device_1").arguments().unwrap(),
            vec!["device", "disconnect", "device_1"]
        );
        for invalid in ["", "--help", "-all", "two devices", "one\ntwo", "one;exit"] {
            assert!(Command::Connect(invalid).arguments().is_err());
            assert!(Command::Disconnect(invalid).arguments().is_err());
            assert!(new_terminal_arguments(invalid, TerminalShell::Cmd).is_err());
        }
    }

    #[test]
    fn native_identity_requires_the_current_account_and_ignores_credentials() {
        let ini = "[Other]\ndeviceId=wrong\n[General]\nuserId=123\ndeviceId=local-device\ntoken=private-value\n";
        assert_eq!(
            parse_local_identity(ini, "123").as_deref(),
            Some("local-device")
        );
        assert!(parse_local_identity(ini, "456").is_none());
        assert!(parse_local_identity(&(ini.to_string() + "deviceId=duplicate\n"), "123").is_none());
        assert!(parse_local_identity("[General]\nuserId=123\ndeviceId=--help", "123").is_none());
    }

    #[test]
    fn remote_terminal_is_explicitly_new_and_has_no_invented_exec_flag() {
        for shell in ["powershell", "cmd", "zsh", "bash"] {
            assert_eq!(
                new_terminal_arguments("device_1", TerminalShell::parse(shell).unwrap()).unwrap(),
                vec![
                    "term",
                    "--device-id",
                    "device_1",
                    "--shell",
                    shell,
                    "--new-session"
                ]
            );
        }
        assert!(TerminalShell::parse("pwsh -Command anything").is_err());
        assert_eq!(failure_message(Some("TIMEOUT")), failure_message(Some("5")));
    }
}
