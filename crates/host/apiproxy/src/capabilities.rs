//! Persistent Skills and MCP management with runtime enforcement.
use cordis::Context;
use dsh_mcp_client::{
    ReconnectingStdioClient, RemoteHttpClient, RemoteHttpConfig, StdioClient, StdioConfig,
};
use dsh_skill::{SkillRegistry, SkillResourceBase, SkillViewOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerConfig {
    name: String,
    #[serde(default = "stdio")]
    transport: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    enabled: bool,
}
fn stdio() -> String {
    "stdio".to_string()
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    disabled_skills: BTreeSet<String>,
    #[serde(default)]
    servers: Vec<ServerConfig>,
}

enum Connection {
    Stdio(Arc<ReconnectingStdioClient>),
    Http(Arc<RemoteHttpClient>),
}
impl Connection {
    fn count(&self) -> usize {
        match self {
            Self::Stdio(c) => c.tool_count(),
            Self::Http(c) => c.tool_count(),
        }
    }
    async fn close(self) -> Result<(), String> {
        match self {
            Self::Stdio(c) => c.close().await,
            Self::Http(c) => c.close().await,
        }
        .map_err(|e| e.to_string())
    }
}
struct State {
    document: Document,
    connections: BTreeMap<String, Connection>,
    errors: BTreeMap<String, String>,
}

pub struct CapabilityManager {
    ctx: Context,
    root: PathBuf,
    cwd: String,
    skills: Arc<SkillRegistry>,
    state: tokio::sync::Mutex<State>,
}
impl cordis::Service for CapabilityManager {
    fn service_name(&self) -> &'static str {
        "capabilityManager"
    }
}

impl CapabilityManager {
    /// Install once after filesystem, skills, tools and subprocess are composed.
    pub async fn install(
        ctx: &Context,
        data_root: PathBuf,
        cwd: String,
    ) -> Result<Arc<Self>, String> {
        let root = data_root.join("capabilities");
        tokio::fs::create_dir_all(root.join("skills"))
            .await
            .map_err(|e| e.to_string())?;
        let document: Document = match tokio::fs::read(root.join("settings.json")).await {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| format!("capability settings: {e}"))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Document::default(),
            Err(e) => return Err(e.to_string()),
        };
        let skills = ctx
            .get_typed::<Arc<SkillRegistry>>("skills", false)
            .map(|s| s.as_ref().clone())
            .ok_or("capabilities require skills")?;
        for name in &document.disabled_skills {
            skills.set_enabled(name, false)?;
        }
        let provider = dsh_skill_filesystem::apply(
            ctx,
            dsh_skill_filesystem::Config {
                provider_name: Some("capabilities-filesystem".into()),
                custom_skill_dirs: Some(vec![root.join("skills").to_string_lossy().into_owned()]),
                watch: Some(false),
                ..Default::default()
            },
        )
        .await?;
        let manager = Arc::new(Self {
            ctx: ctx.clone(),
            root,
            cwd,
            skills,
            state: tokio::sync::Mutex::new(State {
                document,
                connections: BTreeMap::new(),
                errors: BTreeMap::new(),
            }),
        });
        ctx.register_service(manager.clone());
        let weak = Arc::downgrade(&manager);
        ctx.effect(
            "capabilities: runtime lifecycle",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let weak = weak.clone();
                    let provider = provider.clone();
                    Box::pin(async move {
                        if let Some(manager) = weak.upgrade() {
                            let connections =
                                std::mem::take(&mut manager.state.lock().await.connections);
                            for (_, connection) in connections {
                                let _ = connection.close().await;
                            }
                        }
                        provider().await;
                    })
                }))
            }),
        );
        let weak = Arc::downgrade(&manager);
        tokio::spawn(async move {
            if let Some(manager) = weak.upgrade() {
                let mut state = manager.state.lock().await;
                let configs = state.document.servers.clone();
                for config in configs.into_iter().filter(|s| s.enabled) {
                    manager.reconnect(&mut state, &config).await;
                }
            }
        });
        Ok(manager)
    }

    pub async fn invoke(&self, method: &str, payload: Value) -> Result<Value, String> {
        if method == "capabilities.list" {
            return self.list(payload.get("cwd").and_then(Value::as_str)).await;
        }
        let mut state = self.state.lock().await;
        if let Some(expected) = payload.get("expectedRevision").and_then(Value::as_u64) {
            if expected != state.document.revision {
                return Err("能力配置已被其他窗口修改，请刷新后重试".into());
            }
        }
        match method {
            "capabilities.skillRead" => {
                let name = required(&payload, "name")?;
                let path = self.managed_skill_path(name)?;
                let content = tokio::fs::read_to_string(path)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({"name":name,"content":content}))
            }
            "capabilities.skillSave" => {
                let name = required(&payload, "name")?;
                let content = required(&payload, "content")?;
                if content.len() > 256 * 1024 {
                    return Err("Skill 文件超过 256 KiB".into());
                }
                let path = self.managed_skill_path(name)?;
                if path.exists()
                    && !payload
                        .get("overwrite")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    return Err("同名 Skill 已存在，请使用编辑".into());
                }
                // Parse in a staging directory outside all discovery roots before publication.
                let staging = self.root.join("skill-draft.md");
                atomic(&staging, content.as_bytes()).await?;
                let parsed = dsh_skill_filesystem::parse_skill_file(
                    &staging.to_string_lossy(),
                    &self.ctx,
                    None,
                    true,
                )
                .await?;
                let _ = tokio::fs::remove_file(staging).await;
                if !parsed.is_some_and(|skill| skill.name == name) {
                    return Err(
                        "Skill 必须包含有效的 YAML name、description，且 name 与名称一致".into(),
                    );
                }
                atomic(&path, content.as_bytes()).await?;
                let document = state.document.clone();
                self.persist(&mut state, document).await?;
                self.skills.refresh();
                Ok(json!({"saved":true}))
            }
            "capabilities.skillRemove" => {
                let path = self.managed_skill_path(required(&payload, "name")?)?;
                if !path.is_file() {
                    return Err("仅可移除能力管理器中的本地 Skill".into());
                }
                let trash = self.root.join("trash");
                tokio::fs::create_dir_all(&trash)
                    .await
                    .map_err(|e| e.to_string())?;
                let name = required(&payload, "name")?;
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                tokio::fs::rename(
                    path.parent().unwrap(),
                    trash.join(format!("{name}-{stamp}")),
                )
                .await
                .map_err(|e| e.to_string())?;
                let document = state.document.clone();
                self.persist(&mut state, document).await?;
                self.skills.refresh();
                Ok(json!({"removed":true}))
            }
            "capabilities.skillToggle" => {
                let name = required(&payload, "name")?;
                if !dsh_skill::is_skill_name(name) {
                    return Err("无效的 Skill 名称".into());
                }
                let enabled = payload
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or("missing enabled")?;
                let mut document = state.document.clone();
                if enabled {
                    document.disabled_skills.remove(name);
                } else {
                    document.disabled_skills.insert(name.to_string());
                }
                self.persist(&mut state, document).await?;
                self.skills.set_enabled(name, enabled)?;
                Ok(json!({"enabled":enabled,"revision":state.document.revision}))
            }
            "capabilities.serverSave" => {
                let value = payload.get("server").cloned().ok_or("missing server")?;
                let mut server: ServerConfig =
                    serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                validate_server(&server)?;
                if let Some(previous) = state
                    .document
                    .servers
                    .iter()
                    .find(|s| s.name == server.name)
                {
                    if value.get("env").is_none() {
                        server.env = previous.env.clone();
                    }
                    if value.get("headers").is_none() {
                        server.headers = previous.headers.clone();
                    }
                }
                let mut document = state.document.clone();
                document.servers.retain(|s| s.name != server.name);
                document.servers.push(server.clone());
                document.servers.sort_by(|a, b| a.name.cmp(&b.name));
                self.persist(&mut state, document).await?;
                self.reconnect(&mut state, &server).await;
                Ok(server_view(&server, &state))
            }
            "capabilities.serverToggle"
            | "capabilities.serverRemove"
            | "capabilities.serverTest" => {
                let name = required(&payload, "name")?;
                let mut server = state
                    .document
                    .servers
                    .iter()
                    .find(|s| s.name == name)
                    .cloned()
                    .ok_or("MCP server not found")?;
                if method == "capabilities.serverTest" {
                    if server.enabled {
                        self.reconnect(&mut state, &server).await;
                        return Ok(server_view(&server, &state));
                    }
                    let count = self.probe(&server).await?;
                    return Ok(json!({"status":"tested","toolCount":count,"enabled":false}));
                }
                let mut document = state.document.clone();
                document.servers.retain(|s| s.name != name);
                if method == "capabilities.serverToggle" {
                    server.enabled = payload
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .ok_or("missing enabled")?;
                    document.servers.push(server.clone());
                }
                self.persist(&mut state, document).await?;
                if method == "capabilities.serverRemove" {
                    server.enabled = false;
                }
                self.reconnect(&mut state, &server).await;
                Ok(json!({"revision":state.document.revision,"server":server_view(&server,&state)}))
            }
            _ => Err("unknown capabilities method".into()),
        }
    }

    async fn list(&self, cwd: Option<&str>) -> Result<Value, String> {
        self.skills.refresh();
        let catalog = self
            .skills
            .management_catalog(SkillViewOptions {
                cwd: Some(cwd.unwrap_or(&self.cwd).to_string()),
                ..Default::default()
            })
            .await?;
        let state = self.state.lock().await;
        let skills=catalog.into_iter().map(|s| {
            let path=match s.resource_base { Some(SkillResourceBase::Directory {path})=>Some(path), _=>None };
            let managed=path.as_ref().is_some_and(|path|Path::new(path).starts_with(self.root.join("skills")));
            json!({"name":s.name,"description":s.description,"source":s.source,"path":path,"managed":managed,"enabled":self.skills.is_enabled(&s.name)})
        }).collect::<Vec<_>>();
        Ok(
            json!({"revision":state.document.revision,"skills":skills,"servers":state.document.servers.iter().map(|s|server_view(s,&state)).collect::<Vec<_>>(),"skillDirectory":self.root.join("skills")}),
        )
    }

    fn managed_skill_path(&self, name: &str) -> Result<PathBuf, String> {
        if !dsh_skill::is_skill_name(name) || name.len() > 80 {
            return Err("Skill 名称只能使用小写字母、数字与连字符，最长 80 字符".into());
        }
        let root = self.root.join("skills");
        let directory = root.join(name);
        let path = directory.join("SKILL.md");
        for candidate in [&root, &directory, &path] {
            if let Ok(metadata) = std::fs::symlink_metadata(candidate) {
                if metadata.file_type().is_symlink() {
                    return Err("无法写入链接形式的 Skill 路径".into());
                }
            }
        }
        Ok(path)
    }

    async fn persist(&self, state: &mut State, mut document: Document) -> Result<(), String> {
        document.revision += 1;
        atomic(
            &self.root.join("settings.json"),
            &serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?,
        )
        .await?;
        state.document = document;
        Ok(())
    }
    async fn probe(&self, server: &ServerConfig) -> Result<usize, String> {
        if server.transport == "stdio" {
            StdioClient::probe(
                &self.ctx,
                StdioConfig {
                    server_name: server.name.clone(),
                    command: server.command.clone(),
                    args: server.args.clone(),
                    env: server
                        .env
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    cwd: if server.cwd.trim().is_empty() {
                        self.cwd.clone()
                    } else {
                        server.cwd.clone()
                    },
                    request_timeout: Duration::from_secs(20),
                    close_timeout: Duration::from_secs(3),
                },
            )
            .await
            .map_err(|e| e.to_string())
        } else {
            RemoteHttpClient::probe(
                &self.ctx,
                RemoteHttpConfig {
                    server_name: server.name.clone(),
                    endpoint: server.endpoint.clone(),
                    headers: server.headers.clone(),
                    request_timeout: Duration::from_secs(20),
                },
            )
            .await
            .map_err(|e| e.to_string())
        }
    }
    async fn connect(&self, server: &ServerConfig) -> Result<Connection, String> {
        let result = if server.transport == "stdio" {
            StdioClient::connect_reconnecting(
                &self.ctx,
                StdioConfig {
                    server_name: server.name.clone(),
                    command: server.command.clone(),
                    args: server.args.clone(),
                    env: server
                        .env
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    cwd: if server.cwd.trim().is_empty() {
                        self.cwd.clone()
                    } else {
                        server.cwd.clone()
                    },
                    request_timeout: Duration::from_secs(20),
                    close_timeout: Duration::from_secs(3),
                },
            )
            .await
            .map(Connection::Stdio)
        } else {
            RemoteHttpClient::connect(
                &self.ctx,
                RemoteHttpConfig {
                    server_name: server.name.clone(),
                    endpoint: server.endpoint.clone(),
                    headers: server.headers.clone(),
                    request_timeout: Duration::from_secs(20),
                },
            )
            .await
            .map(Connection::Http)
        };
        result.map_err(|e| e.to_string())
    }
    async fn reconnect(&self, state: &mut State, server: &ServerConfig) {
        state.errors.remove(&server.name);
        if let Some(connection) = state.connections.remove(&server.name) {
            if let Err(error) = connection.close().await {
                state.errors.insert(server.name.clone(), error);
            }
        }
        if !server.enabled {
            return;
        }
        match self.connect(server).await {
            Ok(connection) => {
                state.connections.insert(server.name.clone(), connection);
                state.errors.remove(&server.name);
            }
            Err(error) => {
                state.errors.insert(server.name.clone(), error);
            }
        }
    }
}

fn validate_server(server: &ServerConfig) -> Result<(), String> {
    if server.name.is_empty()
        || server.name.len() > 32
        || !server
            .name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err("MCP 名称须为 1–32 个字母、数字、下划线或连字符".into());
    }
    match server.transport.as_str() {
        "stdio" if !server.command.trim().is_empty() => (),
        "http" if !server.endpoint.trim().is_empty() => (),
        _ => return Err("请选择 stdio 命令或 HTTP 地址".into()),
    }
    if !server.cwd.is_empty() && !Path::new(&server.cwd).is_dir() {
        return Err("MCP 工作目录不存在".into());
    }
    Ok(())
}
fn server_view(server: &ServerConfig, state: &State) -> Value {
    json!({"name":server.name,"transport":server.transport,"command":server.command,"args":server.args,"cwd":server.cwd,"endpoint":server.endpoint,"enabled":server.enabled,"hasSecrets":!server.env.is_empty()||!server.headers.is_empty(),"status":if state.connections.contains_key(&server.name){"connected"}else if state.errors.contains_key(&server.name){"error"}else if server.enabled{"pending"}else{"disabled"},"error":state.errors.get(&server.name),"toolCount":state.connections.get(&server.name).map_or(0,Connection::count)})
}
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing {key}"))
}
async fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    dsh_atomic_write::write_file_atomic(
        path,
        bytes,
        dsh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "dsh-capabilities-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[tokio::test]
    async fn skill_switch_persists_and_blocks_loading_until_reenabled() {
        let root = root();
        let ctx = Context::root();
        let skills = SkillRegistry::install(&ctx, Default::default()).unwrap();
        let manager = CapabilityManager::install(&ctx, root.clone(), root.to_string_lossy().into())
            .await
            .unwrap();
        let content = "---\nname: regression-skill\ndescription: Check runtime skill switches\n---\nVerify the output.";
        manager
            .invoke(
                "capabilities.skillSave",
                json!({"name":"regression-skill","content":content}),
            )
            .await
            .unwrap();
        assert!(
            skills
                .get("regression-skill", Default::default())
                .await
                .unwrap()
                .is_some()
        );
        manager
            .invoke(
                "capabilities.skillToggle",
                json!({"name":"regression-skill","enabled":false,"expectedRevision":1}),
            )
            .await
            .unwrap();
        assert!(
            skills
                .get("regression-skill", Default::default())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !skills
                .snapshot(Default::default())
                .await
                .unwrap()
                .skills
                .iter()
                .any(|s| s.name == "regression-skill")
        );
        assert!(
            skills
                .management_catalog(Default::default())
                .await
                .unwrap()
                .iter()
                .any(|s| s.name == "regression-skill")
        );
        let ctx2 = Context::root();
        let skills2 = SkillRegistry::install(&ctx2, Default::default()).unwrap();
        let reopened =
            CapabilityManager::install(&ctx2, root.clone(), root.to_string_lossy().into())
                .await
                .unwrap();
        assert!(!skills2.is_enabled("regression-skill"));
        assert!(
            reopened
                .invoke(
                    "capabilities.skillToggle",
                    json!({"name":"regression-skill","enabled":true,"expectedRevision":1})
                )
                .await
                .is_err()
        );
        reopened
            .invoke(
                "capabilities.skillToggle",
                json!({"name":"regression-skill","enabled":true,"expectedRevision":2}),
            )
            .await
            .unwrap();
        assert!(
            skills2
                .get("regression-skill", Default::default())
                .await
                .unwrap()
                .is_some()
        );
        assert!(manager.managed_skill_path("../outside").is_err());
        reopened
            .invoke(
                "capabilities.skillRemove",
                json!({"name":"regression-skill","expectedRevision":3}),
            )
            .await
            .unwrap();
        assert!(
            skills2
                .get("regression-skill", Default::default())
                .await
                .unwrap()
                .is_none()
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn mcp_credentials_are_preserved_and_redacted_and_failures_are_visible() {
        let root = root();
        let ctx = Context::root();
        SkillRegistry::install(&ctx, Default::default()).unwrap();
        let manager = CapabilityManager::install(&ctx, root.clone(), root.to_string_lossy().into())
            .await
            .unwrap();
        let config = json!({"name":"test","transport":"stdio","command":"definitely-missing-executable","enabled":false,"env":{"API_KEY":"private-token"}});
        let row = manager
            .invoke("capabilities.serverSave", json!({"server":config}))
            .await
            .unwrap();
        assert_eq!(row["hasSecrets"], true);
        assert!(!row.to_string().contains("private-token"));
        manager.invoke("capabilities.serverSave",json!({"server":{"name":"test","transport":"stdio","command":"missing","enabled":true}})).await.unwrap();
        let state = manager.state.lock().await;
        assert_eq!(state.document.servers[0].env["API_KEY"], "private-token");
        assert!(state.connections.is_empty());
        assert!(state.errors.contains_key("test"));
        drop(state);
        manager
            .invoke(
                "capabilities.serverToggle",
                json!({"name":"test","enabled":false}),
            )
            .await
            .unwrap();
        assert!(!manager.state.lock().await.document.servers[0].enabled);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
