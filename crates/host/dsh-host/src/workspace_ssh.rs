//! Explicit SSH forwarding to an existing remote Harness. No local workspace
//! registration, credential copying, remote installation or command execution.
use axum::body::{Body, to_bytes};
use cordis::Context;
use dsh_host_webserver::{WebRequest, WebResponse, WebRoute, WebRouteKind, WebServer};
use dsh_subprocess::{
    SubprocessCollect, SubprocessHandle, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use futures::FutureExt;
use http::{Response, StatusCode, header};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Connection {
    id: String,
    host: String,
    #[serde(default)]
    user: String,
    port: u16,
    remote_port: u16,
    path: String,
    #[serde(default)]
    config_file: String,
}

impl Connection {
    fn validate(&self) -> Result<(), String> {
        uuid::Uuid::parse_str(&self.id).map_err(|_| "连接标识无效")?;
        let host = self.host.as_bytes();
        if host.is_empty()
            || host.len() > 253
            || !host[0].is_ascii_alphanumeric()
            || !host
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || b".-_:".contains(c))
        {
            return Err("SSH 主机必须为主机名、地址或已有 SSH 配置别名".into());
        }
        if self.user.len() > 128
            || self.user.starts_with('-')
            || !self
                .user
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            return Err("SSH 用户名无效".into());
        }
        if self.port == 0 || self.remote_port == 0 {
            return Err("端口必须在 1–65535 之间".into());
        }
        if !self.config_file.is_empty() {
            let file = std::path::Path::new(&self.config_file);
            if !file.is_absolute()
                || !file.is_file()
                || self.config_file.chars().any(char::is_control)
            {
                return Err("SSH 配置文件必须是本机已有文件的绝对路径".into());
            }
            dsh_workspace_resources::checked_path(file)?;
        }
        let path = self.path.as_bytes();
        if path.is_empty()
            || path.len() > 4096
            || self.path.chars().any(char::is_control)
            || !(self.path.starts_with('/')
                || (path.len() > 2
                    && path[0].is_ascii_alphabetic()
                    && path[1] == b':'
                    && b"/\\".contains(&path[2])))
        {
            return Err("请填写远端已有工作目录的绝对路径".into());
        }
        Ok(())
    }
    fn argv(&self, local_port: u16) -> Vec<String> {
        let mut args: Vec<String> = [
            "ssh",
            "-N",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
            "-o",
            "ForwardAgent=no",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "RemoteCommand=none",
            "-L",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        args.extend([
            format!("127.0.0.1:{local_port}:127.0.0.1:{}", self.remote_port),
            "-p".into(),
            self.port.to_string(),
        ]);
        if !self.user.is_empty() {
            args.extend(["-l".into(), self.user.clone()]);
        }
        if !self.config_file.is_empty() {
            args.extend(["-F".into(), self.config_file.clone()]);
        }
        args.push(self.host.clone());
        args
    }
}

struct Entry {
    config: Connection,
    cancel: Arc<AtomicBool>,
    child: Option<Arc<dyn SubprocessHandle>>,
    url: Option<String>,
    error: Option<String>,
    connecting: bool,
}
struct Manager {
    entries: Mutex<BTreeMap<String, Entry>>,
    runtime: Arc<dsh_subprocess_local::LocalSubprocessRuntime>,
    file: PathBuf,
    cwd: String,
    load_error: Option<String>,
}
struct AttemptGuard<'a> {
    manager: &'a Manager,
    id: String,
    armed: bool,
}
impl Drop for AttemptGuard<'_> {
    fn drop(&mut self) {
        if self.armed
            && let Some(entry) = self.manager.entries.lock().get_mut(&self.id)
        {
            entry.connecting = false;
            entry.url = None;
            entry.cancel.store(true, Ordering::SeqCst);
            if let Some(child) = &entry.child {
                child.terminate();
            }
        }
    }
}
impl Manager {
    fn list(&self) -> Value {
        let entries = self.entries.lock();
        json!({"error":self.load_error,"items": entries.values().map(|e| {
            let connected = e.url.is_some() && e.child.as_ref().is_some_and(|c| c.done().now_or_never().is_none());
            json!({"connection":e.config,"state":if e.connecting {"connecting"} else if connected {"connected"} else {"disconnected"},"url":if connected {e.url.clone()} else {None},"error":e.error})
        }).collect::<Vec<_>>()})
    }
    fn persist(&self) -> Result<(), String> {
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        // Serialize while holding the registry lock to prevent stale concurrent saves.
        let entries = self.entries.lock();
        dsh_workspace_resources::persist_json(
            &self.file,
            &entries
                .values()
                .map(|e| e.config.clone())
                .collect::<Vec<_>>(),
        )
    }
    async fn disconnect(&self, id: &str) {
        let child = {
            let mut entries = self.entries.lock();
            entries.get_mut(id).and_then(|entry| {
                entry.cancel.store(true, Ordering::SeqCst);
                entry.url = None;
                entry.child.clone()
            })
        };
        if let Some(child) = child {
            child.terminate();
            let _ = child.done().await;
        }
    }
    async fn connect(&self, config: Connection) -> Result<Value, String> {
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        config.validate()?;
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut entries = self.entries.lock();
            if entries.get(&config.id).is_some_and(|e| {
                e.connecting
                    || e.child
                        .as_ref()
                        .is_some_and(|c| c.done().now_or_never().is_none())
            }) {
                return Err("连接正在使用，请先断开后重连".into());
            }
            if entries.len() >= 16 && !entries.contains_key(&config.id) {
                return Err("最多保存 16 个 SSH 工作目录，请先移除不用的连接".into());
            }
            entries.insert(
                config.id.clone(),
                Entry {
                    config: config.clone(),
                    cancel: cancel.clone(),
                    child: None,
                    url: None,
                    error: None,
                    connecting: true,
                },
            );
        }
        let mut guard = AttemptGuard {
            manager: self,
            id: config.id.clone(),
            armed: true,
        };
        let result = self.start(&config, cancel.clone()).await;
        if result.is_err() {
            self.disconnect(&config.id).await;
        }
        {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.get_mut(&config.id) {
                entry.connecting = false;
                match &result {
                    Ok(url) => entry.url = Some(url.clone()),
                    Err(error) => entry.error = Some(error.clone()),
                }
            }
        }
        // Failed connections are retained for correction, but never auto-reconnected.
        if let Err(error) = self.persist() {
            self.disconnect(&config.id).await;
            return Err(error);
        }
        guard.armed = false;
        result.map(|url| json!({"id":config.id,"url":url,"execution":"remote-host"}))
    }
    async fn start(&self, config: &Connection, cancel: Arc<AtomicBool>) -> Result<String, String> {
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "无法分配本机隧道端口")?;
        let port = listener
            .local_addr()
            .map_err(|_| "无法读取隧道端口")?
            .port();
        drop(listener);
        let collect = || {
            SubprocessOutputMode::Collect(SubprocessCollect {
                max_bytes: 16384,
                spill: None,
            })
        };
        let signal = cancel.clone();
        let child = self
            .runtime
            .spawn(SubprocessSpawnSpec {
                argv: config.argv(port),
                cwd: self.cwd.clone(),
                grace_ms: 500,
                stdio: SubprocessStdio {
                    stdin: SubprocessStdinMode::Ignore,
                    stdout: collect(),
                    stderr: collect(),
                },
                signal: Some(Arc::new(move || signal.load(Ordering::SeqCst))),
                env: None,
            })
            .map_err(|_| "无法启动 OpenSSH，请安装 SSH 客户端")?;
        if let Some(entry) = self.entries.lock().get_mut(&config.id) {
            entry.child = Some(child.clone());
        }
        let url = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| "无法创建隧道客户端")?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("SSH 连接已取消".into());
            }
            if child.done().now_or_never().is_some() {
                return Err(
                    "SSH 连接失败；请检查地址、密钥和已验证的 known_hosts；不支持交互密码登录"
                        .into(),
                );
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("远端 Harness 未就绪，请确认其已启动并仅监听指定的远端回环端口".into());
            }
            if let Ok(response) = client.post(format!("{url}/api/workspace.list")).json(&json!({"type":"client-request","method":"workspace.list","rpcId":"ssh-probe","payload":{}})).send().await
                && response.status().is_success() && let Ok(value) = bounded_json(response).await
                && value["result"]["ok"] == true && value["result"]["value"]["items"].is_array() { break; }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        if cancel.load(Ordering::SeqCst) {
            return Err("SSH 连接已取消".into());
        }
        let response = client.post(format!("{url}/api/workspace.create")).json(&json!({"type":"client-request","method":"workspace.create","rpcId":"ssh-adopt","payload":{"kind":"local","path":config.path}})).send().await.map_err(|_| "无法访问远端工作目录")?;
        let value = bounded_json(response).await?;
        if value["result"]["ok"] != true {
            return Err("远端目录不可用，请检查目录是否存在及远端用户权限".into());
        }
        if cancel.load(Ordering::SeqCst) {
            return Err("SSH 连接已取消；已在远端添加的目录不会删除".into());
        }
        Ok(url)
    }
}

async fn bounded_json(mut response: reqwest::Response) -> Result<Value, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "远端响应读取失败")? {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err("远端响应过大".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "远端不是兼容的 Harness 服务".into())
}
fn reply(status: StatusCode, value: Value) -> WebResponse {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(value.to_string()))
        .unwrap()
}
pub(crate) fn register(
    ctx: &Context,
    web: &Arc<WebServer>,
    root: &std::path::Path,
) -> Result<(), String> {
    let file = root.join("ssh-workspaces.json");
    let read_configs = || -> Result<Vec<Connection>, String> {
        if file.exists() {
            dsh_workspace_resources::checked_path(&file)?;
            if std::fs::metadata(&file).map_err(|e| e.to_string())?.len() > 131072 {
                return Err("SSH 连接配置过大".into());
            }
            let configs: Vec<Connection> =
                serde_json::from_slice(&std::fs::read(&file).map_err(|e| e.to_string())?)
                    .map_err(|_| "SSH 连接配置无效".to_string())?;
            if configs.len() > 16 {
                return Err("SSH 连接配置数量过多".into());
            }
            Ok(configs)
        } else {
            Ok(vec![])
        }
    };
    let (configs, load_error) = match read_configs() {
        Ok(configs) => (configs, None),
        Err(_) => (
            vec![],
            Some(
                "SSH 连接配置不可用，请检查数据目录中的 ssh-workspaces.json；本地工作区不受影响"
                    .to_string(),
            ),
        ),
    };
    let mut entries = BTreeMap::new();
    for config in configs {
        let error = config.validate().err();
        entries.insert(
            config.id.clone(),
            Entry {
                config,
                cancel: Arc::new(AtomicBool::new(false)),
                child: None,
                url: None,
                error,
                connecting: false,
            },
        );
    }
    let manager = Arc::new(Manager {
        entries: Mutex::new(entries),
        runtime: dsh_subprocess_local::LocalSubprocessRuntime::new(),
        file,
        cwd: root.to_string_lossy().into_owned(),
        load_error,
    });
    let route_manager = manager.clone();
    let route = web.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/__dsh-workspaces/ssh".into(),
        handler: Arc::new(move |request: WebRequest| {
            let manager = route_manager.clone();
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
                        json!({"message":"只允许本机可信页面管理 SSH 连接"}),
                    ));
                }
                if request.method() == http::Method::GET {
                    return Ok(reply(StatusCode::OK, manager.list()));
                }
                if request.method() != http::Method::POST {
                    return Ok(reply(
                        StatusCode::METHOD_NOT_ALLOWED,
                        json!({"message":"只允许 GET 或 POST"}),
                    ));
                }
                if !request
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.starts_with("application/json"))
                {
                    return Ok(reply(
                        StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        json!({"message":"必须发送 JSON"}),
                    ));
                }
                let action = request
                    .uri()
                    .path()
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let data: Value = match to_bytes(Body::new(request.into_body()), 8192).await {
                    Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
                    Err(_) => Value::Null,
                };
                let result = match action.as_str() {
                    "connect" => match serde_json::from_value::<Connection>(data) {
                        Ok(config) => manager.connect(config).await,
                        Err(_) => Err("SSH 连接参数无效".into()),
                    },
                    "disconnect" | "remove" => {
                        if let Some(id) = data["id"].as_str() {
                            manager.disconnect(id).await;
                            if action == "remove" {
                                if manager.entries.lock().get(id).is_some_and(|e| e.connecting) {
                                    return Ok(reply(
                                        StatusCode::CONFLICT,
                                        json!({"message":"连接仍在结束，请稍后移除"}),
                                    ));
                                }
                                manager.entries.lock().remove(id);
                            }
                            manager.persist().map(|()| json!({"disconnected":true}))
                        } else {
                            Err("缺少连接标识".into())
                        }
                    }
                    _ => Err("未知 SSH 操作".into()),
                };
                Ok(match result {
                    Ok(value) => reply(StatusCode::OK, value),
                    Err(message) => reply(StatusCode::BAD_REQUEST, json!({"message":message})),
                })
            })
        }),
    });
    let _ = ctx.effect(
        "SSH workspace connections",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                route();
                for entry in manager.entries.lock().values() {
                    entry.cancel.store(true, Ordering::SeqCst);
                    if let Some(child) = &entry.child {
                        child.terminate();
                    }
                }
                let manager = manager.clone();
                Box::pin(async move {
                    let ids: Vec<_> = manager.entries.lock().keys().cloned().collect();
                    for id in ids {
                        manager.disconnect(&id).await;
                    }
                })
            }))
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Connection {
        Connection {
            id: uuid::Uuid::new_v4().to_string(),
            host: "build-host".into(),
            user: "developer".into(),
            port: 22,
            remote_port: 58080,
            path: "/srv/project with spaces".into(),
            config_file: String::new(),
        }
    }
    #[test]
    fn ssh_never_interpolates_directory_into_commands_or_forwards_agent() {
        let c = config();
        c.validate().unwrap();
        let args = c.argv(32123);
        assert!(!args.iter().any(|a| a.contains("/srv/")));
        assert!(args.contains(&"StrictHostKeyChecking=yes".into()));
        assert!(args.contains(&"ForwardAgent=no".into()));
        assert!(args.contains(&"127.0.0.1:32123:127.0.0.1:58080".into()));
        assert_eq!(args.last().unwrap(), "build-host");
    }
    #[test]
    fn invalid_addresses_ports_paths_and_users_are_rejected() {
        for host in [
            "-oProxyCommand=x",
            "host;touch x",
            "user@host",
            "host\nother",
            "",
        ] {
            let mut c = config();
            c.host = host.into();
            assert!(c.validate().is_err());
        }
        for path in ["relative", "~/repo", "/repo\nother", ""] {
            let mut c = config();
            c.path = path.into();
            assert!(c.validate().is_err());
        }
        let mut c = config();
        c.port = 0;
        assert!(c.validate().is_err());
        c.port = 22;
        c.user = "-root".into();
        assert!(c.validate().is_err());
    }
}
