//! TLS-capable streamable HTTP with bounded JSON/SSE decoding.
use crate::{
    McpClientError, McpTransport, PROTOCOL_VERSION, RequestFuture, error, register_tools,
    validate_server_name,
};
use cordis::{Context, Disposer};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct RemoteHttpConfig {
    pub server_name: String,
    pub endpoint: String,
    pub headers: BTreeMap<String, String>,
    pub request_timeout: Duration,
}

pub struct RemoteHttpClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    session: tokio::sync::Mutex<Option<String>>,
    request_gate: tokio::sync::Mutex<()>,
    next_id: AtomicU64,
    closed: AtomicBool,
    registrations: parking_lot::Mutex<Vec<Disposer>>,
    tool_count: AtomicU64,
}

impl RemoteHttpClient {
    pub async fn connect(
        ctx: &Context,
        config: RemoteHttpConfig,
    ) -> Result<Arc<Self>, McpClientError> {
        Self::connect_inner(ctx, config, true).await
    }

    /// Test a disabled server without registering its tools.
    pub async fn probe(ctx: &Context, config: RemoteHttpConfig) -> Result<usize, McpClientError> {
        let client = Self::connect_inner(ctx, config, false).await?;
        let count = client.tool_count();
        client.close().await?;
        Ok(count)
    }

    async fn connect_inner(
        ctx: &Context,
        config: RemoteHttpConfig,
        register: bool,
    ) -> Result<Arc<Self>, McpClientError> {
        validate_server_name(&config.server_name)?;
        let endpoint =
            reqwest::Url::parse(&config.endpoint).map_err(|_| error("invalid MCP endpoint URL"))?;
        let local = matches!(
            endpoint.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        );
        if !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && local)
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(error(
                "MCP endpoint requires HTTPS (HTTP is allowed on loopback only), without embedded credentials or fragments",
            ));
        }
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "accept",
            "application/json, text/event-stream".parse().unwrap(),
        );
        headers.insert("mcp-protocol-version", PROTOCOL_VERSION.parse().unwrap());
        for (key, value) in config.headers {
            let name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|_| error("invalid MCP header name"))?;
            if matches!(name.as_str(), "host" | "content-length" | "mcp-session-id") {
                return Err(error("reserved MCP HTTP header"));
            }
            headers.insert(
                name,
                value
                    .parse()
                    .map_err(|_| error("invalid MCP header value"))?,
            );
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| error(e.to_string()))?;
        let transport = Arc::new(Self {
            client,
            endpoint,
            session: tokio::sync::Mutex::new(None),
            request_gate: tokio::sync::Mutex::new(()),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            registrations: parking_lot::Mutex::new(Vec::new()),
            tool_count: AtomicU64::new(0),
        });
        let startup = async {
            let init = transport.request("initialize", json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"dsh","version":"0.1.0"}})).await?;
            if !init.is_object() { return Err(error("MCP initialize returned invalid result")); }
            transport.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}), None).await?;
            let mut listed = json!({"tools":[]});
            let mut cursor = None;
            let mut seen = std::collections::HashSet::new();
            loop {
                let page = transport.request("tools/list", cursor.as_ref().map(|v|json!({"cursor":v})).unwrap_or(json!({}))).await?;
                let rows = page.get("tools").and_then(Value::as_array).ok_or_else(|| error("MCP tools/list returned no tools array"))?;
                listed["tools"].as_array_mut().unwrap().extend(rows.iter().cloned());
                cursor = page.get("nextCursor").and_then(Value::as_str).map(str::to_string);
                let Some(next) = &cursor else { break; };
                if !seen.insert(next.clone()) || seen.len() > 100 { return Err(error("MCP tools pagination did not terminate")); }
            }
            transport.tool_count.store(listed["tools"].as_array().unwrap().len() as u64, Ordering::Relaxed);
            let route: Arc<dyn McpTransport> = transport.clone();
            if register { *transport.registrations.lock() = register_tools(ctx, route, &config.server_name, listed).await?; }
            Ok(())
        }.await;
        if let Err(failure) = startup {
            let _ = transport.close().await;
            return Err(failure);
        }
        Ok(transport)
    }

    pub fn tool_count(&self) -> usize {
        self.tool_count.load(Ordering::Relaxed) as usize
    }

    pub async fn close(&self) -> Result<(), McpClientError> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let registrations = std::mem::take(&mut *self.registrations.lock());
        for dispose in registrations.into_iter().rev() {
            dispose().await;
        }
        if let Some(session) = self.session.lock().await.take() {
            let response = self
                .client
                .delete(self.endpoint.clone())
                .header("mcp-session-id", session)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .map_err(|_| error("MCP session close failed"))?;
            if !response.status().is_success() && !matches!(response.status().as_u16(), 404 | 405) {
                return Err(error(format!(
                    "MCP session close returned HTTP {}",
                    response.status()
                )));
            }
        }
        Ok(())
    }

    async fn request(&self, method: &'static str, params: Value) -> Result<Value, McpClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(error("MCP connection is closed"));
        }
        let _gate = self.request_gate.lock().await;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            Some(id),
        )
        .await
    }

    async fn send(&self, frame: Value, id: Option<u64>) -> Result<Value, McpClientError> {
        let mut request = self.client.post(self.endpoint.clone()).json(&frame);
        if let Some(session) = self.session.lock().await.clone() {
            request = request.header("mcp-session-id", session);
        }
        let mut response = request.send().await.map_err(|_| {
            error("MCP HTTP request failed; check the endpoint, network and credentials")
        })?;
        if !response.status().is_success() {
            return Err(error(format!("MCP returned HTTP {}", response.status())));
        }
        if let Some(session) = response.headers().get("mcp-session-id") {
            *self.session.lock().await = Some(
                session
                    .to_str()
                    .map_err(|_| error("invalid MCP session id"))?
                    .to_string(),
            );
        }
        if id.is_none() {
            return Ok(Value::Null);
        }
        let is_sse = response
            .headers()
            .get("content-type")
            .and_then(|s| s.to_str().ok())
            .is_some_and(|s| s.starts_with("text/event-stream"));
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| error("MCP response stream failed"))?
        {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > 8 * 1024 * 1024 {
                return Err(error("MCP response exceeds 8 MiB"));
            }
            if is_sse {
                while let Some(end) = bytes
                    .windows(2)
                    .position(|s| s == b"\n\n")
                    .map(|i| (i, 2))
                    .or_else(|| {
                        bytes
                            .windows(4)
                            .position(|s| s == b"\r\n\r\n")
                            .map(|i| (i, 4))
                    })
                {
                    let event: Vec<_> = bytes.drain(..end.0 + end.1).collect();
                    let text = String::from_utf8_lossy(&event);
                    let data = text
                        .lines()
                        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if data.is_empty() {
                        continue;
                    }
                    let value: Value =
                        serde_json::from_str(&data).map_err(|_| error("invalid MCP SSE JSON"))?;
                    if value.get("id").and_then(Value::as_u64) == id {
                        return result(value);
                    }
                }
            }
        }
        if is_sse {
            return Err(error("MCP SSE ended before its response"));
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| error("invalid MCP JSON response"))?;
        if value.get("id").and_then(Value::as_u64) != id {
            return Err(error("MCP response id mismatch"));
        }
        result(value)
    }
}

fn result(value: Value) -> Result<Value, McpClientError> {
    if let Some(failure) = value.get("error") {
        return Err(error(format!(
            "MCP protocol error: {}",
            failure
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("request failed")
        )));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| error("MCP response is missing result"))
}

impl McpTransport for RemoteHttpClient {
    fn request(&self, method: &'static str, params: Value) -> RequestFuture<'_> {
        Box::pin(self.request(method, params))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn streamable_http_registers_calls_and_unregisters_real_tools() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..9 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                reader.read_line(&mut first).await.unwrap();
                let mut length = 0;
                let mut authorized = false;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    let lower = line.to_lowercase();
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap()
                    }
                    if lower.starts_with("authorization: bearer test-token") {
                        authorized = true
                    }
                }
                assert!(authorized);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).await.unwrap();
                let (status, content_type, body) = if first.starts_with("DELETE") {
                    (200, "application/json", String::new())
                } else {
                    let request: Value = serde_json::from_slice(&body).unwrap();
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    match request["method"].as_str().unwrap() {
                        "initialize"=>(200,"application/json",json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"serverInfo":{"name":"test","version":"1"}}}).to_string()),
                        "notifications/initialized"=>(202,"application/json",String::new()),
                        "tools/list"=>(200,"text/event-stream",format!(": heartbeat\r\n\r\ndata: {}\r\n\r\n",json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"echo","description":"Echo input","inputSchema":{"type":"object","properties":{}}}]}}))),
                        "tools/call"=>(200,"application/json",json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"ok"}]}}).to_string()),
                        method=>panic!("unexpected method {method}")
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nMcp-Session-Id: test-session\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                reader
                    .get_mut()
                    .write_all(response.as_bytes())
                    .await
                    .unwrap();
            }
        });
        let ctx = Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        let tools = dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
        let client = RemoteHttpClient::connect(
            &ctx,
            RemoteHttpConfig {
                server_name: "remote-test".into(),
                endpoint: format!("http://{address}/mcp"),
                headers: BTreeMap::from([("Authorization".into(), "Bearer test-token".into())]),
                request_timeout: Duration::from_secs(3),
            },
        )
        .await
        .unwrap();
        assert_eq!(client.tool_count(), 1);
        let name = crate::public_tool_name("remote-test", "echo");
        assert!(tools.get(&name, None).is_some());
        let result = client
            .request("tools/call", json!({"name":"echo","arguments":{}}))
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "ok");
        client.close().await.unwrap();
        assert!(tools.get(&name, None).is_none());
        assert!(client.request("tools/call", json!({})).await.is_err());
        let probe = RemoteHttpClient::connect_inner(
            &ctx,
            RemoteHttpConfig {
                server_name: "remote-test".into(),
                endpoint: format!("http://{address}/mcp"),
                headers: BTreeMap::from([("Authorization".into(), "Bearer test-token".into())]),
                request_timeout: Duration::from_secs(3),
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(probe.tool_count(), 1);
        assert!(
            tools.get(&name, None).is_none(),
            "a disabled-server probe must never register tools"
        );
        probe.close().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn remote_plaintext_and_embedded_credentials_are_rejected() {
        for endpoint in [
            "http://example.com/mcp",
            "https://user:password@example.com/mcp",
            "https://example.com/mcp#fragment",
        ] {
            assert!(
                RemoteHttpClient::connect(
                    &Context::root(),
                    RemoteHttpConfig {
                        server_name: "test".into(),
                        endpoint: endpoint.into(),
                        headers: BTreeMap::new(),
                        request_timeout: Duration::from_secs(1)
                    }
                )
                .await
                .is_err()
            );
        }
    }
}
