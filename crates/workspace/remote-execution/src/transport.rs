use crate::protocol::*;
use dsh_subprocess::{
    SubprocessCollect, SubprocessOutputMode, SubprocessRuntime, SubprocessSpawnSpec,
    SubprocessStdinMode, SubprocessStdio,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
pub struct RemoteConnection {
    pub connection: Connection,
    pub handshake: Handshake,
}
#[derive(Clone, Serialize, Deserialize)]
struct Journal {
    connection_id: String,
    execution: Execution,
    state: String,
}
pub struct RemoteRuntime {
    root: PathBuf,
    local: Arc<dyn SubprocessRuntime>,
    connections: Mutex<BTreeMap<String, RemoteConnection>>,
    save_gate: tokio::sync::Mutex<()>,
    dispatch_gate: tokio::sync::Mutex<()>,
}
impl cordis::Service for RemoteRuntime {
    fn service_name(&self) -> &'static str {
        "remoteExecution"
    }
}
struct Kill(Arc<dyn dsh_subprocess::SubprocessHandle>);
impl Drop for Kill {
    fn drop(&mut self) {
        self.0.terminate();
    }
}
async fn persist(path: &std::path::Path, value: &impl Serialize) -> Result<(), String> {
    dsh_atomic_write::write_file_atomic(
        path,
        &serde_json::to_vec(value).map_err(|e| e.to_string())?,
        dsh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .map_err(|e| e.to_string())
}
fn read<T: for<'a> Deserialize<'a>>(path: &std::path::Path) -> Option<T> {
    let m = std::fs::metadata(path).ok()?;
    if m.len() > 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
impl RemoteRuntime {
    pub fn install(
        ctx: &cordis::Context,
        root: PathBuf,
        local: Arc<dyn SubprocessRuntime>,
    ) -> Arc<Self> {
        let connections =
            read::<BTreeMap<String, RemoteConnection>>(&root.join("connections-v1.json"))
                .filter(|c| {
                    c.len() <= 16
                        && c.values().all(|r| {
                            r.connection.validate().is_ok()
                                && r.handshake.protocol_version == PROTOCOL_VERSION
                        })
                })
                .unwrap_or_default();
        let runtime = Arc::new(Self {
            root,
            local,
            connections: Mutex::new(connections),
            save_gate: tokio::sync::Mutex::new(()),
            dispatch_gate: tokio::sync::Mutex::new(()),
        });
        ctx.register_service(runtime.clone());
        runtime
    }
    pub fn list(&self) -> Value {
        json!({"protocolVersion":PROTOCOL_VERSION,"items":self.connections.lock().values().cloned().collect::<Vec<_>>()})
    }
    pub fn connection(&self, id: &str) -> Result<RemoteConnection, String> {
        self.connections
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("remote connection {id} is not configured"))
    }
    pub fn by_uri(&self, cwd: &str) -> Result<(RemoteConnection, String), String> {
        let (id, path) = split_uri(cwd).ok_or("invalid remote workspace URI")?;
        Ok((self.connection(&id)?, path))
    }
    async fn exchange(
        &self,
        config: &Connection,
        request: &Request,
        signal: Option<dsh_subprocess::SubprocessAbort>,
    ) -> Result<Reply, String> {
        config.validate()?;
        let frame = serde_json::to_string(request).map_err(|e| e.to_string())?;
        if frame.len() > MAX_FRAME {
            return Err("remote request budget exceeded".into());
        }
        let child = self.local.spawn(SubprocessSpawnSpec {
            argv: config.argv(),
            cwd: std::env::current_dir()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
            env: None,
            signal,
            grace_ms: 300,
            stdio: SubprocessStdio {
                stdin: SubprocessStdinMode::Data(frame),
                stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: MAX_FRAME as u64,
                    spill: None,
                }),
                stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 4096,
                    spill: None,
                }),
            },
        })?;
        let _kill = Kill(child.clone());
        let outcome = tokio::time::timeout(Duration::from_secs(20), child.done())
            .await
            .map_err(|_| "SSH response timed out; remote execution state is unknown")??;
        let outputs = child.collected();
        if outcome.exit_code != Some(0) {
            return Err(format!(
                "SSH/helper unavailable (exit {:?}); verify known_hosts, authentication and explicit remote helper installation. {}",
                outcome.exit_code,
                outputs
                    .stderr
                    .map(|r| r.read_from(0).text)
                    .unwrap_or_default()
                    .chars()
                    .take(1024)
                    .collect::<String>()
            ));
        }
        let stdout = outputs
            .stdout
            .ok_or("remote response missing")?
            .read_from(0);
        if stdout.lossy {
            return Err("remote response frame exceeded budget".into());
        }
        let reply: Reply = serde_json::from_str(&stdout.text).map_err(
            |_| "invalid remote protocol frame; stdout cannot contain banners or child output",
        )?;
        if reply.protocol_version != PROTOCOL_VERSION || reply.request_id != request.request_id {
            return Err("remote protocol version or correlation mismatch".into());
        }
        if let Some(expected) = &request.context_id {
            if &reply.context_id != expected {
                return Err("remote execution environment identity changed".into());
            }
        }
        if !reply.ok {
            return Err(reply
                .error
                .unwrap_or_else(|| "remote operation failed".into()));
        }
        Ok(reply)
    }
    pub async fn connect(&self, connection: Connection) -> Result<RemoteConnection, String> {
        connection.validate()?;
        let reply = self
            .exchange(
                &connection,
                &Request {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    workspace: connection.workspace.clone(),
                    action: "handshake".into(),
                    context_id: None,
                    execution_id: None,
                    permission_mode: None,
                    payload: Value::Null,
                },
                None,
            )
            .await?;
        let handshake: Handshake =
            serde_json::from_value(reply.value).map_err(|e| e.to_string())?;
        if handshake.protocol_version != PROTOCOL_VERSION
            || handshake.host_id != reply.host_id
            || handshake.context_id != reply.context_id
            || !handshake
                .capabilities
                .iter()
                .any(|c| c == "structured-process")
        {
            return Err("remote helper capability or identity mismatch".into());
        }
        let row = RemoteConnection {
            connection,
            handshake,
        };
        let _guard = self.save_gate.lock().await;
        let mut next = self.connections.lock().clone();
        if next.len() >= 16 && !next.contains_key(&row.connection.id) {
            return Err("remote connection limit reached".into());
        }
        next.insert(row.connection.id.clone(), row.clone());
        persist(&self.root.join("connections-v1.json"), &next).await?;
        *self.connections.lock() = next;
        Ok(row)
    }
    pub async fn remove(&self, id: &str) -> Result<(), String> {
        let _guard = self.save_gate.lock().await;
        let mut next = self.connections.lock().clone();
        next.remove(id);
        persist(&self.root.join("connections-v1.json"), &next).await?;
        *self.connections.lock() = next;
        Ok(())
    }
    pub async fn call(
        &self,
        id: &str,
        action: &str,
        payload: Value,
        mode: &str,
        execution_id: Option<String>,
        signal: Option<dsh_subprocess::SubprocessAbort>,
    ) -> Result<Value, String> {
        let row = self.connection(id)?;
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: uuid::Uuid::new_v4().to_string(),
            workspace: row.handshake.workspace,
            action: action.into(),
            context_id: Some(row.handshake.context_id),
            execution_id,
            permission_mode: Some(mode.into()),
            payload,
        };
        let reply = self.exchange(&row.connection, &request, signal).await?;
        let mut value = reply.value;
        if value.is_object() {
            value["hostId"] = json!(reply.host_id);
            value["backendId"] = json!(reply.backend_id);
            value["contextId"] = json!(reply.context_id);
        }
        Ok(value)
    }
    pub async fn submit(
        &self,
        id: &str,
        mut execution: Execution,
    ) -> Result<(String, Value), String> {
        let _guard = self.dispatch_gate.lock().await;
        let fingerprint = digest(
            &json!({"connection":id,"context":execution.context_id,"cwd":execution.cwd,"argv":execution.argv,"env":execution.env,"stdin":execution.stdin,"mode":execution.permission_mode}),
        );
        let journal_path = self
            .root
            .join("executions")
            .join(format!("{fingerprint}.json"));
        if let Some(previous) = read::<Journal>(&journal_path) {
            if !matches!(
                previous.state.as_str(),
                "completed" | "failed" | "cancelled" | "timed_out"
            ) {
                let value=self.call(id,"query",json!({}),&execution.permission_mode,Some(previous.execution.execution_id.clone()),None).await.map_err(|error|format!("[REMOTE_UNKNOWN] executionId={}; query original execution before repeating: {error}",previous.execution.execution_id))?;
                return Ok((previous.execution.execution_id, value));
            }
        }
        execution.execution_id = uuid::Uuid::new_v4().to_string();
        let execution_id = execution.execution_id.clone();
        persist(
            &journal_path,
            &Journal {
                connection_id: id.into(),
                execution: execution.clone(),
                state: "submitted".into(),
            },
        )
        .await?;
        let result = self
            .call(
                id,
                "execute",
                serde_json::to_value(&execution).unwrap(),
                &execution.permission_mode,
                Some(execution_id.clone()),
                None,
            )
            .await;
        match result {
            Ok(value) => {
                let state = value["state"].as_str().unwrap_or("unknown").to_string();
                persist(
                    &journal_path,
                    &Journal {
                        connection_id: id.into(),
                        execution,
                        state,
                    },
                )
                .await?;
                Ok((execution_id, value))
            }
            Err(error) => Err(format!(
                "[REMOTE_UNKNOWN] executionId={execution_id}; remote dispatch may have started; query this id without replaying: {error}"
            )),
        }
    }
    pub async fn query(
        &self,
        id: &str,
        execution_id: &str,
        mode: &str,
        stdout_offset: u64,
        stderr_offset: u64,
        cancel: bool,
    ) -> Result<Value, String> {
        let value = self
            .call(
                id,
                if cancel { "cancel" } else { "query" },
                json!({"stdoutOffset":stdout_offset,"stderrOffset":stderr_offset}),
                mode,
                Some(execution_id.into()),
                None,
            )
            .await?;
        // Update only an exact known id; the journal guards repeated model retries after disconnect.
        if let Ok(entries) = std::fs::read_dir(self.root.join("executions")) {
            for entry in entries.filter_map(Result::ok).take(512) {
                if let Some(mut record) = read::<Journal>(&entry.path()) {
                    if record.connection_id == id && record.execution.execution_id == execution_id {
                        record.state = value["state"].as_str().unwrap_or("unknown").into();
                        persist(&entry.path(), &record).await?;
                        break;
                    }
                }
            }
        }
        Ok(value)
    }
}
