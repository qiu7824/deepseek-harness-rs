//! UU account/device bridge over the vendor's documented local CLI.
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
    time::Duration,
};

const WEBSITE: &str = "https://uuyc.163.com/";
const NS: &str = "uu-remote";

fn reply(status: StatusCode, value: Value) -> WebResponse {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(value.to_string()))
        .unwrap()
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
async fn command(path: &Path, args: &[&str]) -> Result<Value, String> {
    let output = dsh_native_command::run_native_command_bounded(
        &path.to_string_lossy(),
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        None,
        dsh_native_command::NativeCommandLimits {
            timeout: Duration::from_secs(12),
            stdout_bytes: 1024 * 1024,
            stderr_bytes: 8192,
        },
    )
    .await
    .map_err(|e| match e.code.as_deref() {
        Some("2") => "请打开 UU 远程客户端并登录账号".into(),
        Some("5") => "UU 远程响应超时，请检查客户端连接".into(),
        _ => format!(
            "UU 远程操作失败（{}）",
            e.code.as_deref().unwrap_or("连接异常")
        ),
    })?;
    let value: Value = serde_json::from_str(output.stdout.trim_start_matches('\u{feff}').trim())
        .map_err(|_| "UU 远程没有返回有效数据，请更新官方客户端".to_string())?;
    if value["success"] != true {
        return Err("UU 远程未完成操作，请在客户端检查登录与设备状态".into());
    }
    Ok(value["data"].clone())
}
fn connected_target(data: &Value, id: &str) -> bool {
    data["devices"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["targetId"] == id))
}
struct Bridge {
    settings: Arc<SettingsProvider>,
    lock: tokio::sync::Mutex<()>,
}
impl Bridge {
    fn settings(&self) -> Value {
        self.settings
            .get(&settings_namespace(NS).unwrap())
            .and_then(|v| v.to_json())
            .unwrap_or_else(|| json!({}))
    }
    async fn devices(&self, cli: &Path) -> Result<(String, Value, Vec<Value>), String> {
        let user = command(cli, &["user", "info"]).await?;
        let identity = user["userId"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| user["userId"].as_u64().map(|n| n.to_string()))
            .filter(|v| !v.is_empty())
            .ok_or("请在 UU 远程客户端登录账号")?;
        let account = dsh_workspace_resources::digest(identity.as_bytes());
        let data = command(cli, &["device", "list"]).await?;
        let local_id = dsh_native_command::run_native_command_bounded(
            &cli.to_string_lossy(),
            &["-d".to_string()],
            None,
            dsh_native_command::NativeCommandLimits {
                timeout: Duration::from_secs(3),
                stdout_bytes: 1024,
                stderr_bytes: 1024,
            },
        )
        .await
        .ok()
        .map(|output| output.stdout.trim().to_string())
        .filter(|id| !id.is_empty() && id.len() <= 256 && id.bytes().all(|c| c.is_ascii_digit()));
        let rows=data["devices"].as_array().ok_or("UU 设备列表格式无效")?.iter().take(256).filter_map(|row| {
            let id=row["deviceId"].as_str()?;
            if id.is_empty()||id.len()>256||!id.chars().all(|c|c.is_ascii_alphanumeric()||c=='-'||c=='_'){return None}
            Some(json!({"id":id,"name":row["deviceName"].as_str().unwrap_or(id).chars().take(128).collect::<String>(),"online":row["isOnline"]==true,"platform":row["platform"],"local":local_id.as_deref()==Some(id)}))
        }).collect();
        Ok((
            account,
            json!({"name":user["nickname"].as_str().unwrap_or("UU 账号").chars().take(128).collect::<String>()}),
            rows,
        ))
    }
    async fn action(&self, action: &str, args: Value) -> Result<Value, String> {
        let _guard = self.lock.lock().await;
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
        let (account, user, devices) = match self.devices(&cli).await {
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
            return Ok(
                json!({"installed":true,"signedIn":true,"cliPath":cli,"website":WEBSITE,"account":user,"devices":devices,"boundDeviceId":binding,"capabilities":["device-list","connect","disconnect"]}),
            );
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
        let result = command(&cli, &["device", action, id]).await?;
        if action == "connect" && !connected_target(&result, id) {
            return Err("UU 没有为该设备发起连接，请检查设备状态后重试".into());
        }
        Ok(json!({"deviceId":id,"action":action,"completed":true}))
    }
}
pub fn register(
    ctx: &Context,
    web: &Arc<WebServer>,
    settings: Arc<SettingsProvider>,
) -> Result<(), String> {
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
    });
    let route = web.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/__dsh-devices".into(),
        handler: Arc::new(move |request: WebRequest| {
            let bridge = bridge.clone();
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
