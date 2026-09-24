use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME: usize = 4 * 1024 * 1024;
pub const MAX_LOG: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub host: String,
    #[serde(default)]
    pub user: String,
    pub port: u16,
    pub workspace: String,
    pub helper: String,
    #[serde(default)]
    pub config_file: String,
}
impl Connection {
    pub fn validate(&self) -> Result<(), String> {
        uuid::Uuid::parse_str(&self.id).map_err(|_| "invalid connection id")?;
        if self.host.is_empty()
            || self.host.len() > 253
            || self.host.starts_with('-')
            || !self
                .host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".-_:".contains(&c))
        {
            return Err("invalid SSH host".into());
        }
        if self.user.len() > 128
            || self.user.starts_with('-')
            || !self
                .user
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            return Err("invalid SSH user".into());
        }
        if self.port == 0 {
            return Err("invalid SSH port".into());
        }
        // SSH invokes a remote login shell. Limit the only command word to safe filename
        // characters; commands, expansions and shell syntax cannot enter this channel.
        if self.helper.is_empty()
            || self.helper.starts_with('-')
            || self.helper.len() > 4096
            || !self
                .helper
                .chars()
                .all(|c| c.is_alphanumeric() || "/\\:._- ".contains(c))
        {
            return Err("helper must be a program path, not a shell command".into());
        }
        if self.workspace.is_empty()
            || self.workspace.len() > 4096
            || self.workspace.chars().any(char::is_control)
        {
            return Err("invalid remote workspace".into());
        }
        if !self.config_file.is_empty()
            && (!std::path::Path::new(&self.config_file).is_absolute()
                || !std::path::Path::new(&self.config_file).is_file())
        {
            return Err("SSH config must be an existing absolute local file".into());
        }
        Ok(())
    }
    pub fn argv(&self) -> Vec<String> {
        let mut args = [
            "ssh",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=10",
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
            "-o",
            "SendEnv=-*",
            "-p",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        args.push(self.port.to_string());
        if !self.user.is_empty() {
            args.extend(["-l".into(), self.user.clone()]);
        }
        if !self.config_file.is_empty() {
            args.extend(["-F".into(), self.config_file.clone()]);
        }
        args.extend([self.host.clone(), format!("\"{}\" --stdio", self.helper)]);
        args
    }
    pub fn uri(&self) -> String {
        format!("dsh-remote://{}/", self.id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handshake {
    pub protocol_version: u32,
    pub host_id: String,
    pub backend_id: String,
    pub context_id: String,
    pub os: String,
    pub arch: String,
    pub workspace: String,
    pub permission_ceiling: String,
    pub shell_path: Option<String>,
    pub shell_kind: String,
    pub python_path: Option<String>,
    pub tools: std::collections::BTreeMap<String, String>,
    pub capabilities: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub protocol_version: u32,
    pub request_id: String,
    pub workspace: String,
    pub action: String,
    #[serde(default)]
    pub context_id: Option<String>,
    #[serde(default)]
    pub execution_id: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub payload: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    pub protocol_version: u32,
    pub request_id: String,
    pub host_id: String,
    pub context_id: String,
    pub backend_id: String,
    pub ok: bool,
    pub value: Value,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Execution {
    pub execution_id: String,
    pub context_id: String,
    pub workspace: String,
    pub permission_mode: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub timeout_ms: u64,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub stdin: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionState {
    pub execution_id: String,
    pub context_id: String,
    pub state: String,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub updated_at: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub error: Option<String>,
}
impl ExecutionState {
    pub fn terminal(&self) -> bool {
        matches!(
            self.state.as_str(),
            "completed" | "failed" | "cancelled" | "timed_out"
        )
    }
}
pub fn digest(value: &Value) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(value.to_string().as_bytes()))
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn split_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("dsh-remote://")?;
    let (id, path) = rest.split_once('/')?;
    uuid::Uuid::parse_str(id).ok()?;
    if path.chars().any(char::is_control) || path.len() > 16384 {
        return None;
    }
    Some((id.into(), path.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_arguments_preserve_host_key_verification_and_refuse_command_injection() {
        let mut c=Connection {id:uuid::Uuid::new_v4().to_string(),host:"server.example.com".into(),user:"worker".into(),port:22,workspace:"/work".into(),helper:"/opt/bin/dsh-remote-helper".into(),config_file:String::new()};
        c.validate().unwrap();let argv=c.argv();assert!(argv.iter().any(|s|s=="StrictHostKeyChecking=yes"));assert!(argv.iter().any(|s|s=="ForwardAgent=no"));
        c.host="-oProxyCommand=bad".into();assert!(c.validate().is_err());c.host="server".into();
        for helper in ["helper; command","$(command)","helper\ncommand","helper\" command"] {c.helper=helper.into();assert!(c.validate().is_err());}
    }
}
