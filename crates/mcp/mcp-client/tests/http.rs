use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use dsh_llm::{ContentBlock, call_id};
use dsh_mcp_client::{StreamableHttpClient, StreamableHttpConfig};
use dsh_tools::{ToolExecutionInput, ToolRuntime};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Option<Value>,
}

fn mount_runtime() -> (Context, Arc<ToolRuntime>) {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("system prompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    (ctx, tools)
}

async fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let header_end = loop {
        assert!(bytes.len() < 64 * 1024, "fixture request headers too large");
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).await.expect("read HTTP request");
        assert_ne!(count, 0, "HTTP request ended before headers");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let head = std::str::from_utf8(&bytes[..header_end]).expect("UTF-8 request headers");
    let mut lines = head[..head.len() - 4].split("\r\n");
    let mut request_line = lines.next().expect("request line").split_whitespace();
    let method = request_line.next().expect("HTTP method").to_string();
    let path = request_line.next().expect("HTTP path").to_string();
    assert_eq!(request_line.next(), Some("HTTP/1.1"));
    let headers: HashMap<String, String> = lines
        .map(|line| {
            let (name, value) = line.split_once(':').expect("HTTP header");
            (name.to_ascii_lowercase(), value.trim().to_string())
        })
        .collect();
    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>().expect("content length"))
        .unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).await.expect("read HTTP body");
        assert_ne!(count, 0, "HTTP request ended before body");
        bytes.extend_from_slice(&chunk[..count]);
    }
    let body = if content_length == 0 {
        None
    } else {
        Some(
            serde_json::from_slice(&bytes[header_end..header_end + content_length])
                .expect("JSON request body"),
        )
    };
    Request {
        method,
        path,
        headers,
        body,
    }
}

async fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: Option<&str>,
    session: Option<&str>,
    body: &[u8],
) {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(content_type) = content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if let Some(session) = session {
        head.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .await
        .expect("response head");
    stream.write_all(body).await.expect("response body");
    stream.shutdown().await.expect("close fixture connection");
}

async fn run_fixture(listener: TcpListener) -> Vec<Request> {
    let mut requests = Vec::new();
    for step in 0..5 {
        let (mut stream, _) = listener.accept().await.expect("accept MCP request");
        let request = read_request(&mut stream).await;
        assert_eq!(request.path, "/mcp?fixture=1");
        assert_eq!(
            request.headers.get("accept").map(String::as_str),
            Some("application/json, text/event-stream")
        );
        if step == 0 {
            assert!(!request.headers.contains_key("mcp-session-id"));
        } else {
            assert_eq!(
                request.headers.get("mcp-session-id").map(String::as_str),
                Some("fixture-session")
            );
        }
        match step {
            0 => {
                assert_eq!(request.method, "POST");
                assert_eq!(request.body.as_ref().unwrap()["method"], "initialize");
                let id = request.body.as_ref().unwrap()["id"].clone();
                let body = serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "http-fixture", "version": "1"}
                    }
                }))
                .unwrap();
                respond(
                    &mut stream,
                    "200 OK",
                    Some("application/json"),
                    Some("fixture-session"),
                    &body,
                )
                .await;
            }
            1 => {
                assert_eq!(request.method, "POST");
                let body = request.body.as_ref().unwrap();
                assert_eq!(body["method"], "notifications/initialized");
                assert!(body.get("id").is_none());
                respond(&mut stream, "202 Accepted", None, None, &[]).await;
            }
            2 => {
                assert_eq!(request.method, "POST");
                assert_eq!(request.body.as_ref().unwrap()["method"], "tools/list");
                let id = request.body.as_ref().unwrap()["id"].clone();
                let message = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [{
                            "name": "inspect/http",
                            "description": "Inspect over streamable HTTP",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"value": {"type": "string"}},
                                "required": ["value"],
                                "additionalProperties": false
                            }
                        }]
                    }
                });
                let body = format!("event: message\ndata: {message}\n\n");
                respond(
                    &mut stream,
                    "200 OK",
                    Some("text/event-stream"),
                    None,
                    body.as_bytes(),
                )
                .await;
            }
            3 => {
                assert_eq!(request.method, "POST");
                let message = request.body.as_ref().unwrap();
                assert_eq!(message["method"], "tools/call");
                assert_eq!(message["params"]["name"], "inspect/http");
                assert_eq!(message["params"]["arguments"], json!({"value": "beta"}));
                let body = serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "result": {
                        "content": [{"type": "text", "text": "HTTP inspected beta"}],
                        "structuredContent": {"transport": "http"}
                    }
                }))
                .unwrap();
                respond(&mut stream, "200 OK", Some("application/json"), None, &body).await;
            }
            4 => {
                assert_eq!(request.method, "DELETE");
                assert!(request.body.is_none());
                respond(&mut stream, "204 No Content", None, None, &[]).await;
            }
            _ => unreachable!(),
        }
        requests.push(request);
    }
    requests
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streamable_http_tracer_initializes_registers_executes_and_closes() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind loopback fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(run_fixture(listener));
    let (ctx, tools) = mount_runtime();

    let client = StreamableHttpClient::connect(
        &ctx,
        StreamableHttpConfig {
            server_name: "web".to_string(),
            endpoint: format!("http://{address}/mcp?fixture=1"),
            request_timeout: Duration::from_secs(3),
            close_timeout: Duration::from_secs(2),
        },
    )
    .await
    .expect("real streamable HTTP fixture should connect");

    let schema = tools
        .schemas(None)
        .into_iter()
        .find(|schema| schema.description == "Inspect over streamable HTTP")
        .expect("HTTP MCP tool registered");
    assert!(schema.name.starts_with("mcp__web__inspect_http_"));
    let public_name = schema.name.clone();

    let result = tools
        .execute(ToolExecutionInput {
            call_id: call_id("mcp-http-tracer-1"),
            root_call_id: None,
            name: public_name.clone(),
            arguments: json!({"value": "beta"}),
            agent: None,
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;
    assert!(!result.is_error, "tool result: {:?}", result.error);
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "HTTP inspected beta".to_string()
        }]
    );
    assert_eq!(
        result.value,
        Some(json!({
            "content": [{"type": "text", "text": "HTTP inspected beta"}],
            "structuredContent": {"transport": "http"}
        }))
    );

    client.close().await.expect("bounded HTTP close");
    assert!(tools.get(&public_name, None).is_none());
    let requests = fixture.await.expect("fixture task");
    assert_eq!(requests.len(), 5);
}
