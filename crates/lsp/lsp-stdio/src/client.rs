use std::sync::Arc;
use std::time::Duration;

use dsh_lsp::{LspHover, LspLocation, LspPosition};
use dsh_subprocess::{
    SubprocessCollect, SubprocessHandle, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::{MessageDecoder, encode_message};

#[derive(Debug, Clone)]
pub struct ClientSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub max_message_bytes: usize,
    pub max_stderr_bytes: u64,
    pub shutdown_timeout_ms: u64,
    pub kill_grace_ms: u64,
}

struct Transport {
    stdin: Box<dyn AsyncWrite + Unpin + Send>,
    stdout: Box<dyn AsyncRead + Unpin + Send>,
    decoder: MessageDecoder,
    next_id: u64,
}

pub struct LspClient {
    _runtime: Arc<dyn SubprocessRuntime>,
    handle: Arc<dyn SubprocessHandle>,
    transport: Mutex<Transport>,
    transient_query: Mutex<()>,
    spec: ClientSpec,
}

impl LspClient {
    pub fn spawn(runtime: Arc<dyn SubprocessRuntime>, spec: ClientSpec) -> Result<Self, String> {
        let mut argv = vec![spec.command.clone()];
        argv.extend(spec.args.clone());
        let handle = runtime.spawn(SubprocessSpawnSpec {
            argv,
            cwd: spec.cwd.clone(),
            stdio: SubprocessStdio {
                stdin: SubprocessStdinMode::Pipe,
                stdout: SubprocessOutputMode::Pipe,
                stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: spec.max_stderr_bytes,
                    spill: None,
                }),
            },
            grace_ms: spec.kill_grace_ms,
            signal: None,
            env: None,
        })?;
        let stdin = handle
            .stdin()
            .ok_or_else(|| "LSP child stdin was not piped".to_string())?;
        let stdout = handle
            .stdout()
            .ok_or_else(|| "LSP child stdout was not piped".to_string())?;
        Ok(Self {
            _runtime: runtime,
            handle,
            transport: Mutex::new(Transport {
                stdin,
                stdout,
                decoder: MessageDecoder::new(spec.max_message_bytes),
                next_id: 1,
            }),
            transient_query: Mutex::new(()),
            spec,
        })
    }

    pub async fn initialize(&self, workspace_uri: &str) -> Result<Value, String> {
        let result = self
            .request(
                "initialize",
                json!({
                    "processId": null,
                    "rootUri": workspace_uri,
                    "capabilities": { "general": { "positionEncodings": ["utf-16"] } }
                }),
            )
            .await?;
        self.notify("initialized", json!({})).await?;
        Ok(result)
    }

    pub async fn definition(
        &self,
        uri: &str,
        language_id: &str,
        text: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>, String> {
        let result = self
            .transient_request(
                "textDocument/definition",
                uri,
                language_id,
                text,
                position,
                None,
            )
            .await?;
        serde_json::from_value(result)
            .map_err(|error| format!("malformed definition response: {error}"))
    }

    pub async fn references(
        &self,
        uri: &str,
        language_id: &str,
        text: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>, String> {
        let result = self
            .transient_request(
                "textDocument/references",
                uri,
                language_id,
                text,
                position,
                Some(json!({ "includeDeclaration": true })),
            )
            .await?;
        serde_json::from_value(result)
            .map_err(|error| format!("malformed references response: {error}"))
    }

    pub async fn implementation(
        &self,
        uri: &str,
        language_id: &str,
        text: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>, String> {
        let result = self
            .transient_request(
                "textDocument/implementation",
                uri,
                language_id,
                text,
                position,
                None,
            )
            .await?;
        serde_json::from_value(result)
            .map_err(|error| format!("malformed implementation response: {error}"))
    }

    pub async fn hover(
        &self,
        uri: &str,
        language_id: &str,
        text: &str,
        position: LspPosition,
    ) -> Result<Option<LspHover>, String> {
        let result = self
            .transient_request("textDocument/hover", uri, language_id, text, position, None)
            .await?;
        serde_json::from_value(result).map_err(|error| format!("malformed hover response: {error}"))
    }

    async fn transient_request(
        &self,
        method: &str,
        uri: &str,
        language_id: &str,
        text: &str,
        position: LspPosition,
        context: Option<Value>,
    ) -> Result<Value, String> {
        let _query = self.transient_query.lock().await;
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": language_id, "version": 1, "text": text } }),
        )
        .await?;
        let mut params = json!({ "textDocument": { "uri": uri }, "position": position });
        if let Some(context) = context {
            params["context"] = context;
        }
        let result = self.request(method, params).await;
        let close = self
            .notify(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await;
        match result {
            Ok(value) => {
                close?;
                Ok(value)
            }
            Err(error) => {
                let _ = close;
                Err(error)
            }
        }
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut transport = self.transport.lock().await;
        let id = transport.next_id;
        transport.next_id += 1;
        let frame = encode_message(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .map_err(|error| error.to_string())?;
        transport
            .stdin
            .write_all(&frame)
            .await
            .map_err(|error| format!("LSP stdin write failed: {error}"))?;
        transport
            .stdin
            .flush()
            .await
            .map_err(|error| format!("LSP stdin flush failed: {error}"))?;
        let mut chunk = vec![0_u8; 8 * 1024];
        loop {
            let read = transport
                .stdout
                .read(&mut chunk)
                .await
                .map_err(|error| format!("LSP stdout read failed: {error}"))?;
            if read == 0 {
                let tail = self.stderr_tail();
                return Err(if tail.trim().is_empty() {
                    "language server exited before responding".to_string()
                } else {
                    format!(
                        "language server exited before responding; stderr: {}",
                        tail.trim()
                    )
                });
            }
            let messages = transport
                .decoder
                .push(&chunk[..read])
                .map_err(|error| error.to_string())?;
            for message in messages {
                if message.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("LSP error response")
                        .to_string());
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let mut transport = self.transport.lock().await;
        let frame =
            encode_message(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
                .map_err(|error| error.to_string())?;
        transport
            .stdin
            .write_all(&frame)
            .await
            .map_err(|error| format!("LSP stdin write failed: {error}"))?;
        transport
            .stdin
            .flush()
            .await
            .map_err(|error| format!("LSP stdin flush failed: {error}"))
    }

    pub fn stderr_tail(&self) -> String {
        self.handle
            .collected()
            .stderr
            .map(|reader| reader.read_from(0).text)
            .unwrap_or_default()
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        let graceful = async {
            self.request("shutdown", Value::Null).await?;
            self.notify("exit", Value::Null).await?;
            self.handle.done().await.map(|_| ())
        };
        if tokio::time::timeout(
            Duration::from_millis(self.spec.shutdown_timeout_ms),
            graceful,
        )
        .await
        .is_ok_and(|result| result.is_ok())
        {
            return Ok(());
        }
        self.handle.terminate();
        if self.handle.wait_for_exit(None).await {
            Ok(())
        } else {
            Err("language server process tree did not exit".to_string())
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.handle.terminate();
    }
}
