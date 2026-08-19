use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use cordis::{Context, Disposer};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{
    McpClientError, McpTransport, PROTOCOL_VERSION, RequestFuture, error, register_tools,
    validate_server_name,
};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct StreamableHttpConfig {
    pub server_name: String,
    pub endpoint: String,
    pub request_timeout: Duration,
    pub close_timeout: Duration,
}

#[derive(Clone, Debug)]
struct Endpoint {
    host: String,
    port: u16,
    authority: String,
    target: String,
}

/// One streamable HTTP session. The dependency-free transport accepts plain
/// HTTP only on loopback and fails closed for remote or TLS endpoints.
pub struct StreamableHttpClient {
    endpoint: Endpoint,
    session_id: tokio::sync::Mutex<Option<String>>,
    gate: tokio::sync::Mutex<()>,
    next_id: AtomicU64,
    request_timeout: Duration,
    close_timeout: Duration,
    registrations: parking_lot::Mutex<Vec<Disposer>>,
    closed: AtomicBool,
}

impl StreamableHttpClient {
    pub async fn connect(
        ctx: &Context,
        config: StreamableHttpConfig,
    ) -> Result<Arc<Self>, McpClientError> {
        validate_server_name(&config.server_name)?;
        if config.request_timeout.is_zero() || config.close_timeout.is_zero() {
            return Err(error("MCP timeouts must be positive"));
        }
        let client = Arc::new(Self {
            endpoint: parse_endpoint(&config.endpoint)?,
            session_id: tokio::sync::Mutex::new(None),
            gate: tokio::sync::Mutex::new(()),
            next_id: AtomicU64::new(1),
            request_timeout: config.request_timeout,
            close_timeout: config.close_timeout,
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
                        "clientInfo": {"name": "dsh-mcp-client", "version": "0.1.0"}
                    }),
                )
                .await?;
            if !initialized.is_object() {
                return Err(error("MCP initialize response must be an object"));
            }
            client.notify("notifications/initialized", None).await?;
            let listed = client.request("tools/list", json!({})).await?;
            let transport: Arc<dyn McpTransport> = client.clone();
            let registrations = register_tools(ctx, transport, &config.server_name, listed).await?;
            *client.registrations.lock() = registrations;
            Ok(())
        }
        .await;
        if let Err(failure) = startup {
            let _ = client.close().await;
            return Err(failure);
        }
        Ok(client)
    }

    pub async fn close(&self) -> Result<(), McpClientError> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let registrations = std::mem::take(&mut *self.registrations.lock());
        for dispose in registrations.into_iter().rev() {
            dispose().await;
        }
        let session = self.session_id.lock().await.clone();
        if session.is_none() {
            return Ok(());
        }
        tokio::time::timeout(self.close_timeout, self.send_http("DELETE", None, session))
            .await
            .map_err(|_| error("MCP HTTP session close timed out"))??
            .require_success("session close")?;
        Ok(())
    }

    async fn notify(
        &self,
        method: &'static str,
        params: Option<Value>,
    ) -> Result<(), McpClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(error("MCP connection is closed"));
        }
        let _gate = self.gate.lock().await;
        let frame = match params {
            Some(params) => json!({"jsonrpc": "2.0", "method": method, "params": params}),
            None => json!({"jsonrpc": "2.0", "method": method}),
        };
        let session = self.session_id.lock().await.clone();
        tokio::time::timeout(
            self.request_timeout,
            self.send_http("POST", Some(&frame), session),
        )
        .await
        .map_err(|_| error(format!("MCP request {method:?} timed out")))??
        .require_success(method)?;
        Ok(())
    }

    async fn request(&self, method: &'static str, params: Value) -> Result<Value, McpClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(error("MCP connection is closed"));
        }
        let _gate = self.gate.lock().await;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let session = self.session_id.lock().await.clone();
        let response = tokio::time::timeout(
            self.request_timeout,
            self.send_http("POST", Some(&frame), session),
        )
        .await
        .map_err(|_| error(format!("MCP request {method:?} timed out")))??
        .require_success(method)?;
        if let Some(new_session) = response.headers.get("mcp-session-id") {
            validate_session_id(new_session)?;
            *self.session_id.lock().await = Some(new_session.clone());
        }
        let response_frame = parse_jsonrpc(&response, id)?;
        if let Some(failure) = response_frame.get("error") {
            return Err(error(format!("MCP request {method:?} failed: {failure}")));
        }
        response_frame
            .get("result")
            .cloned()
            .ok_or_else(|| error(format!("MCP request {method:?} returned no result")))
    }

    async fn send_http(
        &self,
        method: &str,
        body: Option<&Value>,
        session: Option<String>,
    ) -> Result<Response, McpClientError> {
        let mut stream = TcpStream::connect((self.endpoint.host.as_str(), self.endpoint.port))
            .await
            .map_err(|failure| error(format!("cannot connect to MCP HTTP endpoint: {failure}")))?;
        let encoded = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|failure| error(format!("MCP JSON encode failed: {failure}")))?
            .unwrap_or_default();
        let mut request = format!(
            "{method} {} HTTP/1.1\r\nHost: {}\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {PROTOCOL_VERSION}\r\nConnection: close\r\n",
            self.endpoint.target, self.endpoint.authority
        );
        if let Some(session) = session {
            request.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
        }
        if body.is_some() {
            request.push_str("Content-Type: application/json\r\n");
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n", encoded.len()));
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|failure| error(format!("MCP HTTP request write failed: {failure}")))?;
        stream
            .write_all(&encoded)
            .await
            .map_err(|failure| error(format!("MCP HTTP request body write failed: {failure}")))?;
        stream
            .flush()
            .await
            .map_err(|failure| error(format!("MCP HTTP request flush failed: {failure}")))?;
        read_response(&mut stream).await
    }
}

impl McpTransport for StreamableHttpClient {
    fn request(&self, method: &'static str, params: Value) -> RequestFuture<'_> {
        Box::pin(StreamableHttpClient::request(self, method, params))
    }
}

#[derive(Debug)]
struct Response {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Response {
    fn require_success(self, operation: &str) -> Result<Self, McpClientError> {
        if (200..300).contains(&self.status) {
            return Ok(self);
        }
        let detail = String::from_utf8_lossy(&self.body);
        let suffix = if detail.trim().is_empty() {
            String::new()
        } else {
            format!(": {}", detail.trim())
        };
        Err(error(format!(
            "MCP HTTP request {operation:?} failed with status {}{suffix}",
            self.status
        )))
    }
}

fn parse_endpoint(endpoint: &str) -> Result<Endpoint, McpClientError> {
    let remainder = endpoint.strip_prefix("http://").ok_or_else(|| {
        error("MCP streamable HTTP endpoint must use http:// (TLS is unavailable)")
    })?;
    if remainder.contains('#') {
        return Err(error("MCP HTTP endpoint must not contain a fragment"));
    }
    let boundary = remainder.find(['/', '?']).unwrap_or(remainder.len());
    let authority = &remainder[..boundary];
    if authority.is_empty() || authority.contains('@') {
        return Err(error("MCP HTTP endpoint has an invalid authority"));
    }
    let target = match remainder.get(boundary..) {
        Some(value) if value.starts_with('/') => value.to_string(),
        Some(value) if value.starts_with('?') => format!("/{value}"),
        _ => "/".to_string(),
    };
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let close = bracketed
            .find(']')
            .ok_or_else(|| error("MCP HTTP endpoint has an invalid IPv6 address"))?;
        let host = &bracketed[..close];
        let suffix = &bracketed[close + 1..];
        let port = if suffix.is_empty() {
            80
        } else {
            suffix
                .strip_prefix(':')
                .ok_or_else(|| error("MCP HTTP endpoint has an invalid authority"))?
                .parse::<u16>()
                .map_err(|_| error("MCP HTTP endpoint has an invalid port"))?
        };
        (host.to_string(), port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            return Err(error("MCP HTTP IPv6 endpoints must use brackets"));
        }
        (
            host.to_string(),
            port.parse::<u16>()
                .map_err(|_| error("MCP HTTP endpoint has an invalid port"))?,
        )
    } else {
        (authority.to_string(), 80)
    };
    if host.is_empty() || port == 0 {
        return Err(error("MCP HTTP endpoint has an invalid host or port"));
    }
    if !is_loopback_host(&host) {
        return Err(error(
            "plaintext MCP HTTP endpoints are restricted to loopback hosts",
        ));
    }
    Ok(Endpoint {
        host,
        port,
        authority: authority.to_string(),
        target,
    })
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_session_id(session: &str) -> Result<(), McpClientError> {
    if session.is_empty() || !session.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(error("MCP HTTP server returned an invalid session id"));
    }
    Ok(())
}

async fn read_response(stream: &mut TcpStream) -> Result<Response, McpClientError> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if bytes.len() >= 64 * 1024 {
            return Err(error("MCP HTTP response headers exceeded 64 KiB"));
        }
        let mut chunk = [0_u8; 4096];
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|failure| error(format!("MCP HTTP response read failed: {failure}")))?;
        if count == 0 {
            return Err(error("MCP HTTP response ended before its headers"));
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let head = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| error("MCP HTTP response headers were not UTF-8"))?;
    let mut lines = head[..head.len() - 4].split("\r\n");
    let mut status = lines
        .next()
        .ok_or_else(|| error("MCP HTTP response has no status line"))?
        .split_whitespace();
    let version = status.next().unwrap_or_default();
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(error("MCP HTTP response has an unsupported HTTP version"));
    }
    let status = status
        .next()
        .ok_or_else(|| error("MCP HTTP response has no status code"))?
        .parse::<u16>()
        .map_err(|_| error("MCP HTTP response has an invalid status code"))?;
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| error("MCP HTTP response has an invalid header"))?;
        headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
    }
    let mut body = bytes.split_off(header_end);
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        body = read_chunked(stream, body).await?;
    } else if let Some(length) = headers.get("content-length") {
        let length = length
            .parse::<usize>()
            .map_err(|_| error("MCP HTTP response has an invalid content length"))?;
        if length > MAX_RESPONSE_BYTES {
            return Err(error("MCP HTTP response body exceeded 8 MiB"));
        }
        while body.len() < length {
            read_more(stream, &mut body).await?;
        }
        body.truncate(length);
    } else {
        loop {
            let mut chunk = [0_u8; 4096];
            let count = stream
                .read(&mut chunk)
                .await
                .map_err(|failure| error(format!("MCP HTTP response read failed: {failure}")))?;
            if count == 0 {
                break;
            }
            if body.len() + count > MAX_RESPONSE_BYTES {
                return Err(error("MCP HTTP response body exceeded 8 MiB"));
            }
            body.extend_from_slice(&chunk[..count]);
        }
    }
    Ok(Response {
        status,
        headers,
        body,
    })
}

async fn read_more(stream: &mut TcpStream, body: &mut Vec<u8>) -> Result<(), McpClientError> {
    let mut chunk = [0_u8; 4096];
    let count = stream
        .read(&mut chunk)
        .await
        .map_err(|failure| error(format!("MCP HTTP response read failed: {failure}")))?;
    if count == 0 {
        return Err(error("MCP HTTP response body ended early"));
    }
    if body.len() + count > MAX_RESPONSE_BYTES {
        return Err(error("MCP HTTP response body exceeded 8 MiB"));
    }
    body.extend_from_slice(&chunk[..count]);
    Ok(())
}

async fn read_chunked(
    stream: &mut TcpStream,
    mut encoded: Vec<u8>,
) -> Result<Vec<u8>, McpClientError> {
    loop {
        if let Some(decoded) = try_decode_chunked(&encoded)? {
            return Ok(decoded);
        }
        read_more(stream, &mut encoded).await?;
    }
}

fn try_decode_chunked(bytes: &[u8]) -> Result<Option<Vec<u8>>, McpClientError> {
    let mut decoded = Vec::new();
    let mut offset = 0;
    loop {
        let Some(relative) = bytes[offset..]
            .windows(2)
            .position(|window| window == b"\r\n")
        else {
            return Ok(None);
        };
        let line_end = offset + relative;
        let data_start = line_end
            .checked_add(2)
            .ok_or_else(|| error("MCP HTTP chunk boundary overflowed"))?;
        let size_text = std::str::from_utf8(
            bytes[offset..line_end]
                .split(|byte| *byte == b';')
                .next()
                .unwrap_or_default(),
        )
        .map_err(|_| error("MCP HTTP chunk size was not ASCII"))?;
        let size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|_| error("MCP HTTP response has an invalid chunk size"))?;
        if size == 0 {
            let terminator_end = data_start
                .checked_add(2)
                .ok_or_else(|| error("MCP HTTP chunk trailer boundary overflowed"))?;
            return if bytes.len() >= terminator_end {
                Ok(Some(decoded))
            } else {
                Ok(None)
            };
        }
        let data_end = data_start
            .checked_add(size)
            .ok_or_else(|| error("MCP HTTP chunk size overflowed"))?;
        let frame_end = data_end
            .checked_add(2)
            .ok_or_else(|| error("MCP HTTP chunk trailer boundary overflowed"))?;
        if bytes.len() < frame_end {
            return Ok(None);
        }
        if &bytes[data_end..frame_end] != b"\r\n" {
            return Err(error("MCP HTTP response has malformed chunk framing"));
        }
        let decoded_end = decoded
            .len()
            .checked_add(size)
            .ok_or_else(|| error("MCP HTTP decoded body size overflowed"))?;
        if decoded_end > MAX_RESPONSE_BYTES {
            return Err(error("MCP HTTP response body exceeded 8 MiB"));
        }
        decoded.extend_from_slice(&bytes[data_start..data_end]);
        offset = frame_end;
    }
}

fn parse_jsonrpc(response: &Response, id: u64) -> Result<Value, McpClientError> {
    let content_type = response
        .headers
        .get("content-type")
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
        .ok_or_else(|| error("MCP HTTP response has no content type"))?;
    let frames = match content_type.as_str() {
        "application/json" => vec![
            serde_json::from_slice(&response.body)
                .map_err(|failure| error(format!("invalid MCP HTTP JSON-RPC frame: {failure}")))?,
        ],
        "text/event-stream" => parse_sse(&response.body)?,
        _ => {
            return Err(error(format!(
                "MCP HTTP response has unsupported content type {content_type:?}"
            )));
        }
    };
    frames
        .into_iter()
        .find(|frame| frame.get("id").and_then(Value::as_u64) == Some(id))
        .ok_or_else(|| {
            error(format!(
                "MCP HTTP response did not contain JSON-RPC id {id}"
            ))
        })
}

fn parse_sse(body: &[u8]) -> Result<Vec<Value>, McpClientError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| error("MCP HTTP event stream was not UTF-8"))?
        .replace("\r\n", "\n");
    let mut frames = Vec::new();
    for event in text.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
            .collect::<Vec<_>>()
            .join("\n");
        if !data.is_empty() {
            frames.push(serde_json::from_str(&data).map_err(|failure| {
                error(format!("invalid MCP HTTP SSE JSON-RPC frame: {failure}"))
            })?);
        }
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_remote_plaintext_endpoints_before_network_io() {
        let failure = parse_endpoint("http://192.0.2.1/mcp")
            .expect_err("remote plaintext MCP must fail closed");
        assert!(failure.to_string().contains("loopback"), "{failure}");
    }

    #[test]
    fn rejects_chunk_lengths_whose_trailer_boundary_overflows() {
        let hex_digits = usize::BITS as usize / 4;
        let data_start = hex_digits + 2;
        let size = usize::MAX - data_start;
        let encoded = format!("{size:x}\r\n").into_bytes();
        assert_eq!(encoded.len(), data_start);
        let failure = try_decode_chunked(&encoded)
            .expect_err("overflowing chunk trailer boundary must reject");
        assert!(failure.to_string().contains("overflow"), "{failure}");
    }
}
