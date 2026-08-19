//! MCP client bridges for stdio servers and streamable HTTP endpoints:
//! serialized JSON-RPC correlation, model-tool registration, and bounded teardown.

mod http;

pub use http::{StreamableHttpClient, StreamableHttpConfig};

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use cordis::{Context, Disposer};
use dsh_llm::ContentBlock;
use dsh_subprocess::{
    SubprocessHandle, SubprocessOutputMode, SubprocessRuntime, SubprocessSpawnSpec,
    SubprocessStdinMode, SubprocessStdio,
};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub(crate) const PROTOCOL_VERSION: &str = "2025-11-25";

pub(crate) type RequestFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, McpClientError>> + Send + 'a>>;

pub(crate) trait McpTransport: Send + Sync {
    fn request(&self, method: &'static str, params: Value) -> RequestFuture<'_>;
}

/// Configuration for one spawned MCP stdio server.
#[derive(Clone, Debug)]
pub struct StdioConfig {
    pub server_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: String,
    pub request_timeout: Duration,
    pub close_timeout: Duration,
}

/// A fail-loud MCP protocol, process, or lifecycle failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpClientError(String);

impl McpClientError {
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for McpClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for McpClientError {}

/// One connected stdio MCP generation. The request gate covers write through
/// correlated read so concurrent tool calls cannot consume one another's
/// responses.
pub struct StdioClient {
    child: Arc<dyn SubprocessHandle>,
    input: tokio::sync::Mutex<BufReader<Box<dyn AsyncRead + Unpin + Send>>>,
    output: tokio::sync::Mutex<Option<Box<dyn AsyncWrite + Unpin + Send>>>,
    request_gate: tokio::sync::Mutex<()>,
    next_id: AtomicU64,
    request_timeout: Duration,
    close_timeout: Duration,
    stderr: Arc<parking_lot::Mutex<String>>,
    registrations: parking_lot::Mutex<Vec<Disposer>>,
    closed: AtomicBool,
}

/// Stable model-tool route over at most two stdio process generations.
pub struct ReconnectingStdioClient {
    ctx: Context,
    config: StdioConfig,
    current: tokio::sync::Mutex<Arc<StdioClient>>,
    catalog: Value,
    reconnect_gate: tokio::sync::Mutex<()>,
    reconnected: AtomicBool,
    registrations: parking_lot::Mutex<Vec<Disposer>>,
    closed: AtomicBool,
}

impl StdioClient {
    /// Spawn, initialize, discover, and register one server generation.
    pub async fn connect(ctx: &Context, config: StdioConfig) -> Result<Arc<Self>, McpClientError> {
        let (client, listed) = Self::connect_generation(ctx, &config).await?;
        let transport: Arc<dyn McpTransport> = client.clone();
        match register_tools(ctx, transport, &config.server_name, listed).await {
            Ok(registrations) => *client.registrations.lock() = registrations,
            Err(failure) => {
                let _ = client.close().await;
                return Err(failure);
            }
        }
        Ok(client)
    }

    /// Connect a stable route that may replace one failed process generation.
    pub async fn connect_reconnecting(
        ctx: &Context,
        config: StdioConfig,
    ) -> Result<Arc<ReconnectingStdioClient>, McpClientError> {
        let (generation, catalog) = Self::connect_generation(ctx, &config).await?;
        let routed = Arc::new(ReconnectingStdioClient {
            ctx: ctx.clone(),
            config: config.clone(),
            current: tokio::sync::Mutex::new(generation),
            catalog: catalog.clone(),
            reconnect_gate: tokio::sync::Mutex::new(()),
            reconnected: AtomicBool::new(false),
            registrations: parking_lot::Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        });
        let transport: Arc<dyn McpTransport> = routed.clone();
        match register_tools(ctx, transport, &config.server_name, catalog).await {
            Ok(registrations) => *routed.registrations.lock() = registrations,
            Err(failure) => {
                let _ = routed.close().await;
                return Err(failure);
            }
        }
        Ok(routed)
    }

    async fn connect_generation(
        ctx: &Context,
        config: &StdioConfig,
    ) -> Result<(Arc<Self>, Value), McpClientError> {
        validate_server_name(&config.server_name)?;
        if config.request_timeout.is_zero() || config.close_timeout.is_zero() {
            return Err(error("MCP timeouts must be positive"));
        }
        let subprocess = ctx
            .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| error("MCP stdio client requires the subprocess service"))?;
        let executable = subprocess
            .resolve_executable(&config.command, Some(&config.env), None)
            .await
            .map_err(|failure| error(format!("cannot resolve MCP server: {failure}")))?;
        let child = subprocess
            .spawn(SubprocessSpawnSpec {
                argv: std::iter::once(executable)
                    .chain(config.args.clone())
                    .collect(),
                cwd: config.cwd.clone(),
                stdio: SubprocessStdio {
                    stdin: SubprocessStdinMode::Pipe,
                    stdout: SubprocessOutputMode::Pipe,
                    stderr: SubprocessOutputMode::Pipe,
                },
                grace_ms: config.close_timeout.as_millis().min(u64::MAX as u128) as u64,
                signal: None,
                env: Some(
                    config
                        .env
                        .iter()
                        .map(|(key, value)| (key.clone(), Some(value.clone())))
                        .collect(),
                ),
            })
            .map_err(|failure| error(format!("cannot spawn MCP server: {failure}")))?;
        let input = child
            .stdout()
            .ok_or_else(|| error("MCP server stdout was not piped"))?;
        let output = child
            .stdin()
            .ok_or_else(|| error("MCP server stdin was not piped"))?;
        let stderr_stream = child
            .stderr()
            .ok_or_else(|| error("MCP server stderr was not piped"))?;
        let stderr = Arc::new(parking_lot::Mutex::new(String::new()));
        let stderr_for_task = stderr.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr_stream);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => stderr_for_task.lock().push_str(&line),
                }
            }
        });
        let client = Arc::new(Self {
            child,
            input: tokio::sync::Mutex::new(BufReader::new(input)),
            output: tokio::sync::Mutex::new(Some(output)),
            request_gate: tokio::sync::Mutex::new(()),
            next_id: AtomicU64::new(1),
            request_timeout: config.request_timeout,
            close_timeout: config.close_timeout,
            stderr,
            registrations: parking_lot::Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        });
        let startup = async {
            let initialized = client
                .request(
                    "initialize",
                    json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {},
                        "clientInfo": { "name": "dsh-mcp-client", "version": "0.1.0" }
                    }),
                )
                .await?;
            if !initialized.is_object() {
                return Err(error("MCP initialize response must be an object"));
            }
            client.notify("notifications/initialized", None).await?;
            client.request("tools/list", json!({})).await
        }
        .await;
        match startup {
            Ok(listed) => Ok((client, listed)),
            Err(failure) => {
                let _ = client.close().await;
                Err(failure)
            }
        }
    }

    /// Captured diagnostic stderr, never parsed as protocol traffic.
    pub fn stderr_snapshot(&self) -> String {
        self.stderr.lock().clone()
    }

    /// Unregister tools and stop the child within the configured bound.
    pub async fn close(&self) -> Result<(), McpClientError> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let registrations = std::mem::take(&mut *self.registrations.lock());
        for dispose in registrations.into_iter().rev() {
            dispose().await;
        }
        let mut output = self.output.lock().await.take();
        if let Some(output) = output.as_mut() {
            let _ = output.shutdown().await;
        }
        drop(output);
        match tokio::time::timeout(self.close_timeout, self.child.done()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(failure)) => Err(error(format!("MCP server close failed: {failure}"))),
            Err(_) => {
                self.child.terminate();
                tokio::time::timeout(self.close_timeout, self.child.done())
                    .await
                    .map_err(|_| error("MCP server did not exit after forced termination"))?
                    .map_err(|failure| {
                        error(format!("MCP server forced close failed: {failure}"))
                    })?;
                Ok(())
            }
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), McpClientError> {
        let mut output = self.output.lock().await;
        let output = output
            .as_mut()
            .ok_or_else(|| error("MCP connection is closed"))?;
        let frame = match params {
            Some(params) => json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            None => json!({ "jsonrpc": "2.0", "method": method }),
        };
        write_frame(&mut **output, &frame).await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(error("MCP connection is closed"));
        }
        let _gate = self.request_gate.lock().await;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        {
            let mut output = self.output.lock().await;
            let output = output
                .as_mut()
                .ok_or_else(|| error("MCP connection is closed"))?;
            write_frame(
                &mut **output,
                &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
            )
            .await?;
        }
        let read = async {
            let mut input = self.input.lock().await;
            loop {
                let mut line = String::new();
                let bytes = input
                    .read_line(&mut line)
                    .await
                    .map_err(|failure| error(format!("MCP stdout read failed: {failure}")))?;
                if bytes == 0 {
                    let outcome = self.child.done().await.map_err(error)?;
                    return Err(error(format!(
                        "MCP server closed stdout (code {:?}, signal {:?})",
                        outcome.exit_code, outcome.signal
                    )));
                }
                let frame: Value = serde_json::from_str(line.trim())
                    .map_err(|failure| error(format!("invalid MCP JSON-RPC frame: {failure}")))?;
                if frame.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(failure) = frame.get("error") {
                    return Err(error(format!("MCP request {method:?} failed: {failure}")));
                }
                return frame
                    .get("result")
                    .cloned()
                    .ok_or_else(|| error(format!("MCP request {method:?} returned no result")));
            }
        };
        tokio::time::timeout(self.request_timeout, read)
            .await
            .map_err(|_| error(format!("MCP request {method:?} timed out")))?
    }
}

impl McpTransport for StdioClient {
    fn request(&self, method: &'static str, params: Value) -> RequestFuture<'_> {
        Box::pin(StdioClient::request(self, method, params))
    }
}

impl ReconnectingStdioClient {
    pub async fn close(&self) -> Result<(), McpClientError> {
        let _lifecycle = self.reconnect_gate.lock().await;
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let registrations = std::mem::take(&mut *self.registrations.lock());
        for dispose in registrations.into_iter().rev() {
            dispose().await;
        }
        self.current.lock().await.close().await
    }

    async fn request(&self, method: &'static str, params: Value) -> Result<Value, McpClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(error("MCP connection is closed"));
        }
        let generation = self.current.lock().await.clone();
        match generation.request(method, params.clone()).await {
            Ok(value) => Ok(value),
            Err(first) => {
                let _reconnect = self.reconnect_gate.lock().await;
                if self.closed.load(Ordering::SeqCst) {
                    return Err(error("MCP connection is closed"));
                }
                let current = self.current.lock().await.clone();
                if Arc::ptr_eq(&current, &generation) {
                    if self.reconnected.swap(true, Ordering::SeqCst) {
                        return Err(first);
                    }
                    generation.child.terminate();
                    let exited = tokio::time::timeout(
                        self.config.close_timeout,
                        generation.child.wait_for_exit(None),
                    )
                    .await
                    .map_err(|_| {
                        error("MCP failed generation did not terminate before reconnect")
                    })?;
                    if !exited {
                        return Err(error(
                            "MCP failed generation process tree remained alive before reconnect",
                        ));
                    }
                    let (replacement, catalog) =
                        StdioClient::connect_generation(&self.ctx, &self.config).await?;
                    if catalog != self.catalog {
                        let _ = replacement.close().await;
                        return Err(error(
                            "MCP reconnect tool catalog changed; refusing to reuse stale registrations",
                        ));
                    }
                    *self.current.lock().await = replacement;
                }
                self.current
                    .lock()
                    .await
                    .clone()
                    .request(method, params)
                    .await
                    .map_err(|second| {
                        error(format!(
                            "MCP request failed before and after one reconnect: {first}; {second}"
                        ))
                    })
            }
        }
    }
}

impl McpTransport for ReconnectingStdioClient {
    fn request(&self, method: &'static str, params: Value) -> RequestFuture<'_> {
        Box::pin(ReconnectingStdioClient::request(self, method, params))
    }
}

async fn write_frame(
    output: &mut (dyn AsyncWrite + Unpin + Send),
    frame: &Value,
) -> Result<(), McpClientError> {
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|failure| error(format!("MCP JSON encode failed: {failure}")))?;
    bytes.push(b'\n');
    output
        .write_all(&bytes)
        .await
        .map_err(|failure| error(format!("MCP stdin write failed: {failure}")))?;
    output
        .flush()
        .await
        .map_err(|failure| error(format!("MCP stdin flush failed: {failure}")))
}

fn validate_server_name(name: &str) -> Result<(), McpClientError> {
    if name.is_empty()
        || name.len() > 32
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(error("MCP server_name must match [A-Za-z0-9_-]{1,32}"));
    }
    Ok(())
}

fn public_tool_name(server: &str, raw: &str) -> String {
    const MAX: usize = 64;
    let joined = format!("mcp__{server}__{raw}");
    let normalized: String = joined
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if normalized == joined && normalized.len() <= MAX {
        return normalized;
    }
    let hash = format!(
        "{:x}",
        Sha256::digest(format!("{server}\0{raw}").as_bytes())
    );
    format!(
        "{}_{}",
        &normalized[..normalized.len().min(MAX - 13)],
        &hash[..12]
    )
}

pub(crate) async fn register_tools(
    ctx: &Context,
    client: Arc<dyn McpTransport>,
    server: &str,
    listed: Value,
) -> Result<Vec<Disposer>, McpClientError> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| error("MCP client requires the tools service"))?;
    let entries = listed
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| error("MCP tools/list result requires a tools array"))?;
    let mut pending = Vec::new();
    for entry in entries {
        let raw_name = entry
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| error("MCP tool name must be a non-empty string"))?
            .to_string();
        let parameters = entry
            .get("inputSchema")
            .cloned()
            .ok_or_else(|| error(format!("MCP tool {raw_name:?} has no inputSchema")))?;
        dsh_tools::assert_object_json_schema(&parameters).map_err(|failure| {
            error(format!(
                "unsupported MCP inputSchema for {raw_name:?}: {failure}"
            ))
        })?;
        let public_name = public_tool_name(server, &raw_name);
        let description = entry
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let call_client = client.clone();
        let call_raw_name = raw_name.clone();
        let render_raw_name = raw_name.clone();
        pending.push(ToolDefinition {
            name: public_name,
            description,
            parameters,
            output: ToolOutputDefinition {
                schema: json!({
                    "type": "object",
                    "properties": {
                        "content": { "type": "array", "items": {} },
                        "structuredContent": {}
                    },
                    "required": ["content"],
                    "additionalProperties": false
                }),
                render: Arc::new(move |_args, value| {
                    Ok(vec![ContentBlock::Text {
                        text: extract_text(&value["content"], &render_raw_name),
                    }])
                }),
                presentation_meta: None,
            },
            timeout_ms: None,
            is_concurrency_safe: Some(Arc::new(|_args| true)),
            execute: Arc::new(move |args, _run| {
                let client = call_client.clone();
                let raw_name = call_raw_name.clone();
                let arguments = if args.is_object() {
                    args.clone()
                } else {
                    json!({})
                };
                Box::pin(async move {
                    let result = client
                        .request(
                            "tools/call",
                            json!({ "name": raw_name, "arguments": arguments }),
                        )
                        .await
                        .map_err(|failure| ToolBodyError::plain(failure.to_string()))?;
                    if result.get("isError").and_then(Value::as_bool) == Some(true) {
                        return Err(ToolBodyError::plain(extract_text(
                            result.get("content").unwrap_or(&Value::Null),
                            &raw_name,
                        )));
                    }
                    let content = result
                        .get("content")
                        .cloned()
                        .filter(Value::is_array)
                        .unwrap_or_else(|| json!([{ "type": "text", "text": "(no output)" }]));
                    let mut canonical = serde_json::Map::new();
                    canonical.insert("content".to_string(), content);
                    if let Some(structured) = result.get("structuredContent") {
                        canonical.insert("structuredContent".to_string(), structured.clone());
                    }
                    Ok(Value::Object(canonical))
                })
            }),
            finalize_content: None,
            present_call: None,
            present_result: None,
        });
    }
    let mut registrations = Vec::new();
    for definition in pending {
        match tools.register(ctx, definition) {
            Ok(disposer) => registrations.push(disposer),
            Err(failure) => {
                for dispose in registrations.into_iter().rev() {
                    dispose().await;
                }
                return Err(error(format!("MCP tool registration failed: {failure}")));
            }
        }
    }
    Ok(registrations)
}

fn extract_text(content: &Value, tool_name: &str) -> String {
    let mut parts = Vec::new();
    for block in content.as_array().into_iter().flatten() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
            Some("image") => parts.push(format!(
                "[image: {}, content discarded]",
                block
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            )),
            Some("audio") => parts.push(format!(
                "[audio: {}, content discarded]",
                block
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            )),
            Some("resource" | "resource_link") => {
                parts.push("[resource: content discarded]".to_string())
            }
            Some(other) => parts.push(format!("[unsupported content type: {other}]")),
            None => parts.push("[unsupported content type: unknown]".to_string()),
        }
    }
    if parts.is_empty() {
        format!("({tool_name} returned no text content)")
    } else {
        parts.join("\n")
    }
}

fn error(message: impl Into<String>) -> McpClientError {
    McpClientError(message.into())
}
