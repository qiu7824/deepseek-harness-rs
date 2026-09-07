//! Account-scoped room negotiation against the installed UU client version.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Globalization::GetUserDefaultLocaleName,
    System::{
        Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW},
        SystemInformation::GetSystemFirmwareTable,
    },
};

const CLIENT_HASH: &str = "2a3263062c9cbfe0dcaf81d9ec95cca480802a86b7d2fc89ef033f86a61b3853";
const CLIENT_VERSION: &str = "4.38.3.9325";
const SIGNING_KEY_RVA: u32 = 0x3b06e78;
const API: &str = "https://api.nrd.nie.163.com";
pub struct Account {
    pub device_id: String,
    token: String,
    user_id: String,
    client_id: String,
    system_id: String,
    locale: String,
    channel: String,
    bin: PathBuf,
    client: reqwest::blocking::Client,
}
fn ini(path: &Path, key: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|_| "无法读取 UU 登录信息，请先打开 UU 远程并登录")?;
    if bytes.len() > 1024 * 1024 {
        return Err("UU 配置文件超过大小限制".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "UU 配置编码不可用")?;
    let mut general = false;
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.starts_with('[') {
            general = line == "[General]";
            continue;
        }
        if !general {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            if name == key {
                let value = value.trim().trim_matches('"');
                if value.is_empty()
                    || value.len() > 16384
                    || value.starts_with('@')
                    || value.chars().any(char::is_control)
                {
                    return Err("UU 登录配置格式无效".into());
                }
                return Ok(value.to_string());
            }
        }
    }
    Err(format!("UU 登录配置缺少 {key}"))
}
fn fingerprint(path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(|_| "未找到 UU 客户端")?;
    let size = file.metadata().map_err(|_| "无法检查 UU 客户端")?.len();
    if size > 512 * 1024 * 1024 {
        return Err("UU 客户端文件超过大小限制".into());
    }
    let mut hash = Sha256::new();
    let mut buf = vec![0; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|_| "读取 UU 客户端失败")?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    if format!("{:x}", hash.finalize()) != CLIENT_HASH {
        return Err(format!(
            "当前 UU 客户端版本尚未通过接口验证；支持版本为 {CLIENT_VERSION}"
        ));
    }
    Ok(())
}
fn signing_key(bin: &Path) -> Result<[u8; 24], String> {
    let mut file = File::open(bin.join("GameViewer.exe")).map_err(|_| "UU 客户端不可读")?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos).map_err(|_| "UU 客户端头部无效")?;
    let pe = u32::from_le_bytes(dos[60..64].try_into().unwrap());
    file.seek(SeekFrom::Start(pe as u64))
        .map_err(|_| "UU 客户端头部无效")?;
    let mut nt = [0u8; 24];
    file.read_exact(&mut nt).map_err(|_| "UU 客户端头部无效")?;
    let count = u16::from_le_bytes(nt[6..8].try_into().unwrap());
    let optional = u16::from_le_bytes(nt[20..22].try_into().unwrap());
    if &nt[..4] != b"PE\0\0" || count > 96 {
        return Err("UU 客户端结构无效".into());
    }
    file.seek(SeekFrom::Current(optional as i64))
        .map_err(|_| "UU 客户端结构无效")?;
    for _ in 0..count {
        let mut section = [0u8; 40];
        file.read_exact(&mut section)
            .map_err(|_| "UU 客户端结构无效")?;
        let rva = u32::from_le_bytes(section[12..16].try_into().unwrap());
        let size = u32::from_le_bytes(section[16..20].try_into().unwrap());
        let raw = u32::from_le_bytes(section[20..24].try_into().unwrap());
        if SIGNING_KEY_RVA >= rva
            && SIGNING_KEY_RVA
                .checked_add(24)
                .is_some_and(|end| end <= rva.saturating_add(size))
        {
            file.seek(SeekFrom::Start(u64::from(raw + SIGNING_KEY_RVA - rva)))
                .map_err(|_| "UU 接口数据不可读")?;
            let mut key = [0; 24];
            file.read_exact(&mut key).map_err(|_| "UU 接口数据不可读")?;
            return Ok(key);
        }
    }
    Err("UU 接口数据布局不匹配".into())
}
fn sign(key: &[u8], value: &[u8]) -> String {
    let mut inside = [0x36; 64];
    let mut outside = [0x5c; 64];
    for (i, b) in key.iter().enumerate() {
        inside[i] ^= b;
        outside[i] ^= b;
    }
    let mut hash = Sha256::new();
    hash.update(inside);
    hash.update(value);
    let inner = hash.finalize();
    let mut hash = Sha256::new();
    hash.update(outside);
    hash.update(inner);
    format!("{:x}", hash.finalize())
}
fn system_uuid() -> Result<String, String> {
    let provider = u32::from_be_bytes(*b"RSMB");
    let size = unsafe { GetSystemFirmwareTable(provider, 0, std::ptr::null_mut(), 0) };
    if size < 8 || size > 4 * 1024 * 1024 {
        return Err("无法读取本机设备标识".into());
    }
    let mut bytes = vec![0u8; size as usize];
    if unsafe { GetSystemFirmwareTable(provider, 0, bytes.as_mut_ptr().cast(), size) } != size {
        return Err("无法读取本机设备标识".into());
    }
    let mut at = 8;
    while at + 4 <= bytes.len() {
        let kind = bytes[at];
        let len = bytes[at + 1] as usize;
        if len < 4 || at + len > bytes.len() {
            break;
        }
        if kind == 1 && len >= 24 {
            let b = &bytes[at + 8..at + 24];
            if b.iter().all(|v| *v == 0) || b.iter().all(|v| *v == 255) {
                break;
            }
            return Ok(format!(
                "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                u32::from_le_bytes(b[..4].try_into().unwrap()),
                u16::from_le_bytes(b[4..6].try_into().unwrap()),
                u16::from_le_bytes(b[6..8].try_into().unwrap()),
                b[8],
                b[9],
                b[10],
                b[11],
                b[12],
                b[13],
                b[14],
                b[15]
            ));
        }
        at += len;
        while at + 1 < bytes.len() && bytes[at..at + 2] != [0, 0] {
            at += 1
        }
        at += 2;
    }
    Err("本机设备标识不可用".into())
}
impl Account {
    pub fn open(bin: PathBuf, expected_account: &str) -> Result<Self, String> {
        fingerprint(&bin.join("GameViewer.exe"))?;
        let root = PathBuf::from(std::env::var_os("ProgramData").ok_or("ProgramData 不可用")?)
            .join("Netease/GameViewer");
        let info = root.join("user_info.ini");
        let user_id = ini(&info, "userId")?;
        if expected_account.is_empty()
            || format!("{:x}", Sha256::digest(user_id.as_bytes())) != expected_account
        {
            return Err("UU 账号已变化，请在设置中重新绑定设备".into());
        }
        let device_id = ini(&info, "deviceId")?;
        let token = ini(&info, "token")?;
        let client_id = ini(&root.join("config.ini"), "uuid")?;
        let mut locale = [0u16; 85];
        let n = unsafe { GetUserDefaultLocaleName(locale.as_mut_ptr(), locale.len() as i32) };
        if n <= 1 {
            return Err("本机区域标识不可用".into());
        }
        let locale = String::from_utf16_lossy(&locale[..n as usize - 1]);
        let mut channel = [0u16; 256];
        let mut size = (channel.len() * 2) as u32;
        let result = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                windows_sys::w!("SOFTWARE\\Netease\\GameViewerSetup"),
                windows_sys::w!("Channel"),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                channel.as_mut_ptr().cast(),
                &mut size,
            )
        };
        let channel = if result == 0 {
            String::from_utf16_lossy(&channel[..channel.iter().position(|v| *v == 0).unwrap_or(0)])
        } else {
            "nochannel".into()
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(12))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "无法初始化 UU 连接")?;
        Ok(Self {
            device_id,
            token,
            user_id,
            client_id,
            system_id: system_uuid()?,
            locale,
            channel,
            bin,
            client,
        })
    }
    fn request(&self, method: &str, path: &str, body: &str) -> Result<Value, String> {
        let mut headers = BTreeMap::from([
            ("x-param-client-id", self.client_id.clone()),
            ("x-param-device-id", self.device_id.clone()),
            ("x-param-system-id", self.system_id.clone()),
            ("x-param-user-id", self.user_id.clone()),
            ("x-param-plat", "1".into()),
            ("x-param-vn", CLIENT_VERSION.into()),
            ("x-param-vc", "9325".into()),
            ("x-param-pkgn", "com.netease.uuremote".into()),
            ("x-param-chn", self.channel.clone()),
            ("x-param-lang", "zh-CN".into()),
            ("x-param-cnt", self.locale.clone()),
            ("x-param-rel", "prod".into()),
            ("x-param-opr", String::new()),
            (
                "x-param-ts",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| "本机时间无效")?
                    .as_secs()
                    .to_string(),
            ),
        ]);
        let canonical = headers
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let mut key = signing_key(&self.bin)?;
        let signature = sign(&key, format!("{method}{path}{canonical}{body}").as_bytes());
        key.fill(0);
        headers.insert("x-param-sign", signature);
        let mut request = self
            .client
            .request(
                if method == "GET" {
                    reqwest::Method::GET
                } else {
                    reqwest::Method::POST
                },
                format!("{API}{path}"),
            )
            .bearer_auth(&self.token);
        for (k, v) in headers {
            request = request.header(k, v)
        }
        if !body.is_empty() {
            request = request
                .header("Content-Type", "application/json")
                .body(body.to_string())
        }
        let response = request
            .send()
            .map_err(|_| "UU 服务连接失败，请检查网络和客户端登录状态")?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "UU 服务响应读取失败")?;
        if bytes.len() > 1024 * 1024 {
            return Err("UU 服务响应超过大小限制".into());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "UU 服务响应格式无效")?;
        if !status.is_success() || value["code"] != 0 {
            if value["code"] == 2001 {
                return Err(
                    if path == format!("/api/v1/room/join/by_device/{}", self.device_id) {
                        "UU 服务拒绝本机连接（2001：被控设备不在线），未建立自连".into()
                    } else {
                        "UU 服务报告被控设备不在线（2001），请检查远端客户端连接状态".into()
                    },
                );
            }
            return Err(format!(
                "UU 连接请求未完成（HTTP {}，代码 {}）",
                status.as_u16(),
                value["code"].as_i64().unwrap_or(-1)
            ));
        }
        value
            .get("data")
            .cloned()
            .ok_or("UU 服务缺少连接数据".into())
    }
    pub fn join(&self, target: &str) -> Result<(Value, String), String> {
        if target.len() > 128
            || target.is_empty()
            || !target
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("设备 ID 无效".into());
        }
        let groups = self.request("GET", "/api/v1/device/groups/of/my", "")?;
        let device = groups["desktop_devices"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["device_id"] == target))
            .ok_or("设备不属于当前 UU 账号")?;
        if device["controllable"] != true || device["publisher_availability_status"] != "ready" {
            return Err("设备当前不可控制，请检查远端在线和授权状态".into());
        }
        let name = device["alias"]
            .as_str()
            .unwrap_or(target)
            .chars()
            .take(128)
            .collect();
        let room = self.request(
            "POST",
            &format!("/api/v1/room/join/by_device/{target}"),
            &json!({"force_join":false}).to_string(),
        )?;
        Ok((room, name))
    }
}
