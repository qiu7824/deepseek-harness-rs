//! UU account/device bridge over the vendor's documented local CLI.
use crate::uu_cli::{self, Command};
use axum::body::{Body, to_bytes};
use cordis::Context;
use dsh_host_webserver::{WebRequest, WebResponse, WebRoute, WebRouteKind, WebServer};
use dsh_schemastery::{Data, Schema};
use dsh_settings::{SettingsProvider, SettingsRegisterOptions, settings_namespace};
use http::{Response, StatusCode, header};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const WEBSITE: &str = "https://uuyc.163.com/";
const NS: &str = "uu-remote";
const VERIFIED_DESKTOP_SDK_CLIENT: &str = "4.39.2.1561";
const SUPPORTED_DESKTOP_SDK_CLIENTS: &[&str] = &["4.39.2.1561", "4.38.3.9325"];

fn reply(status: StatusCode, value: Value) -> WebResponse {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(value.to_string()))
        .unwrap()
}

fn self_connect_diagnostic(local_device_id: Option<&str>, devices: &[Value]) -> Value {
    let Some(local_id) = local_device_id.filter(|id| !id.is_empty()) else {
        return json!({
            "verification": "not-run",
            "canProbe": false,
            "listed": false,
            "reason": "UU 客户端未提供本机设备 ID",
        });
    };
    let listed = devices.iter().any(|device| device["id"].as_str() == Some(local_id));
    let online = devices.iter().any(|device| {
        device["id"].as_str() == Some(local_id) && device["online"] == true
    });
    json!({
        "verification": "not-run",
        "canProbe": true,
        "listed": listed,
        "online": online,
        "reason": "设备列表不能单独证明或否定自连；需要验证连接状态、真实帧来源和输入回读",
    })
}
fn checked_cli(path: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "uuyc-cli.exe"
    } else {
        "uuyc-cli"
    };
    (path.is_absolute() && path.file_name()?.to_str()?.eq_ignore_ascii_case(name) && path.is_file())
        .then(|| path.canonicalize().ok())
        .flatten()
}
pub(crate) fn installed_cli(configured: &str) -> Option<PathBuf> {
    if !configured.is_empty() {
        return checked_cli(Path::new(configured));
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{
            HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
            RegGetValueW,
        };
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            for branch in [
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\GameViewer",
                "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\GameViewer",
            ] {
                let key = format!("{branch}\0").encode_utf16().collect::<Vec<_>>();
                let field = "DisplayIcon\0".encode_utf16().collect::<Vec<_>>();
                let mut buffer = vec![0u16; 4096];
                let mut bytes = (buffer.len() * 2) as u32;
                let result = unsafe {
                    RegGetValueW(
                        hive,
                        key.as_ptr(),
                        field.as_ptr(),
                        RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                        std::ptr::null_mut(),
                        buffer.as_mut_ptr().cast(),
                        &mut bytes,
                    )
                };
                if result != 0 {
                    continue;
                }
                let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
                let raw = String::from_utf16_lossy(&buffer[..end]);
                let path = PathBuf::from(raw.trim_matches('"'));
                if let Some(parent) = path.parent() {
                    for candidate in [parent.join("bin/uuyc-cli.exe"), parent.join("uuyc-cli.exe")]
                    {
                        if let Some(path) = checked_cli(&candidate) {
                            return Some(path);
                        }
                    }
                }
            }
        }
        for variable in ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"] {
            if let Some(root) = std::env::var_os(variable) {
                if let Some(path) =
                    checked_cli(&PathBuf::from(root).join("Netease/GameViewer/bin/uuyc-cli.exe"))
                {
                    return Some(path);
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        for path in [
            "/Applications/UU远程.app/Contents/MacOS/uuyc-cli",
            "/Applications/GameViewer.app/Contents/MacOS/uuyc-cli",
            "/usr/local/bin/uuyc-cli",
        ] {
            if let Some(path) = checked_cli(Path::new(path)) {
                return Some(path);
            }
        }
    }
    None
}
fn connected_target(data: &Value, id: &str) -> bool {
    data["devices"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["targetId"] == id && row.get("success") != Some(&Value::Bool(false)))
    })
}
fn device_rows(data: &Value, local_id: Option<&str>) -> Result<Vec<Value>, String> {
    let rows = data["devices"].as_array().ok_or("UU 设备列表格式无效")?;
    if rows.len() > 256 {
        return Err("UU 设备列表超过大小限制".into());
    }
    rows.iter().map(|row| {
        let id = row["deviceId"].as_str().filter(|id| uu_cli::valid_device_id(id))
            .ok_or("UU 设备列表包含无效设备 ID")?;
        Ok(json!({"id":id,"name":row["deviceName"].as_str().unwrap_or(id).chars().filter(|c|!c.is_control()).take(128).collect::<String>(),"online":row["isOnline"]==true,"platform":row["platform"],"local":local_id==Some(id)}))
    }).collect()
}
pub(crate) struct Bridge {
    settings: Arc<SettingsProvider>,
    lock: tokio::sync::Mutex<()>,
    version_cache: parking_lot::Mutex<Option<(PathBuf, Instant, String)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalTarget {
    pub cli: PathBuf,
    pub account: String,
    pub device_id: String,
    pub device_name: String,
}

impl Bridge {
    fn settings(&self) -> Value {
        self.settings
            .get(&settings_namespace(NS).unwrap())
            .and_then(|v| v.to_json())
            .unwrap_or_else(|| json!({}))
    }
    async fn cli_version(&self, cli: &Path) -> Option<String> {
        let cached = self
            .version_cache
            .lock()
            .as_ref()
            .filter(|(path, time, _)| path == cli && time.elapsed() < Duration::from_secs(60))
            .map(|(_, _, version)| version.clone());
        if cached.is_some() {
            return cached;
        }
        let version = uu_cli::version(cli).await.ok()?;
        *self.version_cache.lock() = Some((cli.to_path_buf(), Instant::now(), version.clone()));
        Some(version)
    }
    async fn devices(
        &self,
        cli: &Path,
        signal: Option<dsh_native_command::NativeCommandAbort>,
    ) -> Result<(String, Value, Vec<Value>, Option<String>), String> {
        let user = uu_cli::invoke(cli, Command::UserInfo, signal.clone()).await?;
        let identity = user["userId"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| user["userId"].as_u64().map(|n| n.to_string()))
            .filter(|v| !v.is_empty())
            .ok_or("请在 UU 远程客户端登录账号")?;
        let account = dsh_workspace_resources::digest(identity.as_bytes());
        let data = uu_cli::invoke(cli, Command::DeviceList, signal).await?;
        let local_id = uu_cli::local_device_id(&identity);
        let rows = device_rows(&data, local_id.as_deref())?;
        Ok((
            account,
            json!({"name":user["nickname"].as_str().unwrap_or("UU 账号").chars().take(128).collect::<String>()}),
            rows,
            local_id,
        ))
    }

    pub(crate) async fn bound_terminal_target(
        &self,
        expected_device: Option<&str>,
        signal: Option<dsh_native_command::NativeCommandAbort>,
    ) -> Result<TerminalTarget, String> {
        let _guard = self.lock.lock().await;
        let settings = self.settings();
        let cli = installed_cli(settings["cliPath"].as_str().unwrap_or(""))
            .ok_or("未找到 UU 远程 CLI，请检查安装路径")?;
        let (account, _, devices, _) = self.devices(&cli, signal).await?;
        if settings["account"] != account {
            return Err("UU 账号已变化，请重新绑定设备".into());
        }
        let id = settings["deviceId"]
            .as_str()
            .filter(|id| uu_cli::valid_device_id(id))
            .ok_or("请先在设置中绑定 UU 远端设备")?;
        if expected_device.is_some_and(|expected| expected != id) {
            return Err("请求设备与当前 UU 绑定不一致".into());
        }
        let row = devices
            .iter()
            .find(|row| row["id"] == id)
            .ok_or("绑定设备不属于当前 UU 账号")?;
        if row["local"] == true {
            return Err("UU 远程终端不用于本机，请使用本机终端".into());
        }
        if row["online"] != true {
            return Err("绑定的 UU 远端设备当前离线".into());
        }
        if row["platform"] != 1 {
            return Err("UU CLI 远程终端目前仅支持 Windows 目标".into());
        }
        Ok(TerminalTarget {
            cli,
            account,
            device_id: id.to_string(),
            device_name: row["name"].as_str().unwrap_or(id).to_string(),
        })
    }

    /// A remembered terminal may be closed after switching the selected
    /// device, but never through a different logged-in account. Input still
    /// requires the original device to remain the active binding.
    pub(crate) async fn validate_terminal_target(
        &self,
        target: &TerminalTarget,
        require_binding: bool,
        signal: Option<dsh_native_command::NativeCommandAbort>,
    ) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        let cli = checked_cli(&target.cli).ok_or("原 UU 终端客户端不可用")?;
        let (account, _, devices, _) = self.devices(&cli, signal).await?;
        if account != target.account {
            return Err("UU 账号已变化，拒绝操作原账号的终端".into());
        }
        let settings = self.settings();
        if require_binding
            && (settings["account"] != account || settings["deviceId"] != target.device_id)
        {
            return Err("UU 绑定已变化，拒绝向原设备继续输入".into());
        }
        if !devices.iter().any(|row| row["id"] == target.device_id) {
            return Err("原 UU 设备不在当前账号列表中".into());
        }
        Ok(())
    }
    pub(crate) async fn action(&self, action: &str, args: Value) -> Result<Value, String> {
        let _guard = self.lock.lock().await;
        if !matches!(
            action,
            "status" | "open-client" | "bind" | "unbind" | "connect" | "disconnect"
        ) {
            return Err("未知设备操作".into());
        }
        let settings = self.settings();
        let configured = settings["cliPath"].as_str().unwrap_or("");
        let Some(cli) = installed_cli(configured) else {
            return if action == "status" {
                Ok(
                    json!({"installed":false,"website":WEBSITE,"message":"请先安装 UU 远程，登录账号后刷新设备"}),
                )
            } else {
                Err("未找到 UU 远程 CLI，请安装官方客户端或设置安装路径".into())
            };
        };
        if action == "open-client" {
            #[cfg(windows)]
            {
                let app = cli
                    .parent()
                    .ok_or("UU 安装目录无效")?
                    .join("GameViewer.exe");
                if !app.is_file() {
                    return Err("未找到 UU 远程客户端".into());
                }
                std::process::Command::new(app)
                    .spawn()
                    .map_err(|_| "无法打开 UU 远程".to_string())?;
            }
            #[cfg(not(windows))]
            {
                std::process::Command::new("open")
                    .args(["-a", "UU远程"])
                    .spawn()
                    .map_err(|_| "无法打开 UU 远程".to_string())?;
            }
            return Ok(json!({"opened":true}));
        }
        let (account, user, devices, local_device_id) = match self.devices(&cli, None).await {
            Ok(v) => v,
            Err(error) if action == "status" => {
                return Ok(
                    json!({"installed":true,"signedIn":false,"cliPath":cli,"website":WEBSITE,"message":error,"devices":[]}),
                );
            }
            Err(error) => return Err(error),
        };
        let binding = if settings["account"] == account {
            settings["deviceId"].as_str().unwrap_or("")
        } else {
            ""
        };
        if action == "status" {
            let version = self.cli_version(&cli).await;
            let connections = uu_cli::invoke(&cli, Command::DeviceStatus, None)
                .await
                .and_then(|data| uu_cli::connected_devices(&data));
            let (connected_ids, status_error) = match connections {
                Ok(ids) => (Some(ids), None),
                Err(error) => (None, Some(error)),
            };
            return Ok(json!({
                "installed":true,
                "signedIn":true,
                "cliPath":cli,
                "cliVersion":version,
                "desktopSdkCompatibility": {
                    "verified": version.as_deref().is_some_and(|value| SUPPORTED_DESKTOP_SDK_CLIENTS.contains(&value)),
                    "verifiedClientVersion": VERIFIED_DESKTOP_SDK_CLIENT,
                    "verifiedClientVersions": SUPPORTED_DESKTOP_SDK_CLIENTS,
                    "reason": if version.as_deref().is_some_and(|value| SUPPORTED_DESKTOP_SDK_CLIENTS.contains(&value)) { "已验证" } else { "当前 UU 客户端版本将由控制进程按导出接口识别；CLI 设备查询仍可用" },
                },
                "website":WEBSITE,
                "documentation":uu_cli::DOCUMENTATION,
                "account":user,
                "devices":devices,
                "localDeviceId":local_device_id,
                "selfConnect":self_connect_diagnostic(local_device_id.as_deref(), &devices),
                "boundDeviceId":binding,
                "connectedDeviceIds":connected_ids,
                "connectionStatusError":status_error,
                "capabilities":["device-list","device-status","connect","disconnect"]
            }));
        }
        if action == "unbind" {
            self.settings
                .update(
                    &settings_namespace(NS)?,
                    json!({"account":"","deviceId":""}),
                    None,
                )
                .await?;
            return Ok(json!({"unbound":true}));
        }
        let id = args["deviceId"].as_str().unwrap_or("");
        let device = devices
            .iter()
            .find(|d| d["id"] == id)
            .ok_or("设备不属于当前登录账号，请刷新列表")?;
        if action == "bind" {
            self.settings
                .update(
                    &settings_namespace(NS)?,
                    json!({"account":account,"deviceId":id}),
                    None,
                )
                .await?;
            return Ok(json!({"boundDeviceId":id}));
        }
        if !matches!(action, "connect" | "disconnect") {
            return Err("未知设备操作".into());
        }
        if binding != id {
            return Err("请先绑定当前账号下的设备".into());
        }
        if device["local"] == true {
            return Err("这是当前设备；请直接使用本机桌面，浏览器自动化请打开受控浏览器".into());
        }
        if action == "connect" && device["online"] != true {
            return Err("设备当前离线".into());
        }
        let command = if action == "connect" {
            Command::Connect(id)
        } else {
            Command::Disconnect(id)
        };
        let result = uu_cli::invoke(&cli, command, None).await?;
        if action == "connect" && !connected_target(&result, id) {
            return Err("UU 没有为该设备发起连接，请检查设备状态后重试".into());
        }
        Ok(json!({"deviceId":id,"action":action,"completed":true}))
    }
}
pub(crate) fn register(
    ctx: &Context,
    web: &Arc<WebServer>,
    settings: Arc<SettingsProvider>,
) -> Result<Arc<Bridge>, String> {
    settings.register(
        ctx,
        settings_namespace(NS)?,
        Schema::object(indexmap::IndexMap::from([
            (
                "cliPath".into(),
                Schema::string().default(Data::String(String::new())),
            ),
            (
                "account".into(),
                Schema::string().default(Data::String(String::new())),
            ),
            (
                "deviceId".into(),
                Schema::string().default(Data::String(String::new())),
            ),
        ])),
        SettingsRegisterOptions::default(),
    )?;
    let bridge = Arc::new(Bridge {
        settings,
        lock: Default::default(),
        version_cache: Default::default(),
    });
    let route_bridge = bridge.clone();
    let route = web.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/__dsh-devices".into(),
        handler: Arc::new(move |request: WebRequest| {
            let bridge = route_bridge.clone();
            Box::pin(async move {
                if !super::trusted_web_request(&request, false)
                    || !request
                        .headers()
                        .get(header::HOST)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| super::allowed_web_authority(v, false))
                {
                    return Ok(reply(
                        StatusCode::FORBIDDEN,
                        json!({"message":"请求来源不可信"}),
                    ));
                }
                if request.method() != http::Method::POST {
                    return Ok(reply(
                        StatusCode::METHOD_NOT_ALLOWED,
                        json!({"message":"只允许 POST"}),
                    ));
                }
                let action = request
                    .uri()
                    .path()
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let data = match to_bytes(Body::new(request.into_body()), 8192).await {
                    Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
                    Err(_) => Value::Null,
                };
                if !data.is_object() {
                    return Ok(reply(
                        StatusCode::BAD_REQUEST,
                        json!({"message":"请求格式无效"}),
                    ));
                }
                Ok(match bridge.action(&action, data).await {
                    Ok(value) => reply(StatusCode::OK, value),
                    Err(error) => reply(StatusCode::BAD_REQUEST, json!({"message":error})),
                })
            })
        }),
    });
    let _ = ctx.effect(
        "UU device management",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                route();
                Box::pin(async {})
            }))
        }),
    );
    Ok(bridge)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_publication_does_not_claim_self_connect_support() {
        let absent = self_connect_diagnostic(Some("local"), &[]);
        assert_eq!(absent["verification"], "not-run");
        assert_eq!(absent["canProbe"], true);
        assert_eq!(absent["listed"], false);
        let published = self_connect_diagnostic(
            Some("local"),
            &[json!({"id":"local","online":true})],
        );
        assert_eq!(published["verification"], "not-run");
        assert_eq!(published["listed"], true);
        assert!(published.get("supported").is_none());
        assert_eq!(self_connect_diagnostic(None, &[])["canProbe"], false);
    }
    #[test]
    fn explicit_missing_cli_does_not_fall_back_to_another_installation() {
        assert!(installed_cli("Z:/missing/uuyc-cli.exe").is_none());
    }
    #[test]
    fn unrelated_executable_is_rejected() {
        assert!(checked_cli(Path::new("C:/Windows/System32/cmd.exe")).is_none());
    }
    #[test]
    fn empty_or_different_target_is_not_a_successful_connection() {
        assert!(!connected_target(&json!({"devices":[]}), "local"));
        assert!(!connected_target(
            &json!({"devices":[{"targetId":"other"}]}),
            "local"
        ));
        assert!(connected_target(
            &json!({"devices":[{"targetId":"local"}]}),
            "local"
        ));
        assert!(!connected_target(
            &json!({"devices":[{"targetId":"local","success":false}]}),
            "local"
        ));
    }
    #[test]
    fn device_rows_match_account_identity_and_exclude_unrelated_fields() {
        let data = json!({"devices":[{"deviceId":"account-device","deviceName":"Local","isOnline":true,"platform":1,"token":"private-value"}]});
        let rows = device_rows(&data, Some("account-device")).unwrap();
        assert_eq!(rows[0]["local"], true);
        assert!(
            !serde_json::to_string(&rows)
                .unwrap()
                .contains("private-value")
        );
        assert_eq!(
            device_rows(&data, Some("12345678")).unwrap()[0]["local"],
            false
        );
        assert!(device_rows(&json!({"devices":[{"deviceId":"--help"}]}), None).is_err());
    }
}
