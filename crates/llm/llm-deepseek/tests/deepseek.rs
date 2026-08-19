use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use dsh_llm::{
    ContentBlock, FinishReason, GenerateOptions, LlmRuntime, MessageSource, Role, StreamChunk,
    TokenUsage, create_message,
};
use dsh_llm_deepseek::{
    DeepSeekAdapter, DeepSeekAdapterOptions, DeepSeekConfig, PROVIDER, apply,
    resolve_adapter_options,
};
use futures::{FutureExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

#[derive(Debug)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

struct FakeServer {
    base_url: String,
    request: oneshot::Receiver<RecordedRequest>,
}

async fn read_request(socket: &mut TcpStream) -> RecordedRequest {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buffer = [0_u8; 1024];
        let read = socket.read(&mut buffer).await.expect("read request");
        assert!(read > 0, "client closed before request headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(offset) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break offset + 4;
        }
    };
    let head = std::str::from_utf8(&bytes[..header_end]).expect("ASCII request head");
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().expect("request line").split_whitespace();
    let method = request_line.next().expect("method").to_string();
    let path = request_line.next().expect("path").to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let content_length = headers
        .get("content-length")
        .expect("content-length")
        .parse::<usize>()
        .expect("numeric content-length");
    while bytes.len() - header_end < content_length {
        let mut buffer = [0_u8; 1024];
        let read = socket.read(&mut buffer).await.expect("read body");
        assert!(read > 0, "client closed before request body");
        bytes.extend_from_slice(&buffer[..read]);
    }
    let body = serde_json::from_slice(&bytes[header_end..header_end + content_length])
        .expect("JSON request body");
    RecordedRequest {
        method,
        path,
        headers,
        body,
    }
}

async fn spawn_server(
    status: &str,
    response_headers: &[(&str, &str)],
    parts: Vec<Vec<u8>>,
) -> FakeServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address = listener.local_addr().expect("address");
    let (request_tx, request) = oneshot::channel();
    let status = status.to_string();
    let response_headers: Vec<(String, String)> = response_headers
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let captured = read_request(&mut socket).await;
        request_tx.send(captured).ok();
        let content_length: usize = parts.iter().map(Vec::len).sum();
        let mut head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {content_length}\r\nConnection: close\r\n"
        );
        for (name, value) in response_headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        socket
            .write_all(head.as_bytes())
            .await
            .expect("response head");
        for part in parts {
            socket.write_all(&part).await.expect("response part");
            tokio::task::yield_now().await;
        }
        socket.shutdown().await.expect("shutdown");
    });
    FakeServer {
        base_url: format!("http://{address}"),
        request,
    }
}

fn request_options() -> GenerateOptions {
    GenerateOptions {
        provider: PROVIDER.to_string(),
        model: "deepseek-v4-flash".to_string(),
        reasoning_effort: None,
        messages: vec![create_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        )],
        system: Some("Be concise.".to_string()),
        tools: None,
        temperature: None,
        max_tokens: Some(64),
        stop: None,
        signal: None,
        session_id: None,
        purpose: None,
        agent_loop_request: false,
    }
}

#[allow(clippy::result_large_err)] // Resolver intentionally shares the core LlmError seam.
fn adapter(base_url: String, key: Option<&str>) -> Arc<DeepSeekAdapter> {
    let resolved = resolve_adapter_options(&DeepSeekConfig {
        base_url: Some(base_url),
        ..DeepSeekConfig::default()
    })
    .expect("valid options");
    let key = key.map(str::to_string);
    Arc::new(DeepSeekAdapter::new(DeepSeekAdapterOptions {
        options: Arc::new(move || Ok(resolved.clone())),
        resolve_api_key: Arc::new(move |_snapshot| {
            let key = key.clone();
            async move { Ok(key) }.boxed()
        }),
    }))
}

async fn drive(_server: &FakeServer, adapter: Arc<DeepSeekAdapter>) -> Vec<StreamChunk> {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(&ctx, &runtime, adapter).expect("install DeepSeek route");
    tokio::time::timeout(
        Duration::from_secs(2),
        runtime.stream(request_options()).collect(),
    )
    .await
    .expect("adapter stream settled")
}

#[tokio::test]
async fn dropping_pending_stream_closes_the_loopback_connection() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind pending loopback");
    let address = listener.local_addr().expect("pending address");
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept pending stream");
        let _request = read_request(&mut socket).await;
        let _ = accepted_tx.send(());
        let mut byte = [0_u8; 1];
        tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
            .await
            .expect("dropping model stream closes TCP promptly")
            .expect("read pending socket")
    });

    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        adapter(format!("http://{address}"), Some("test-secret")),
    )
    .expect("install route");
    let mut stream = runtime.stream(request_options());
    let next = tokio::spawn(async move { stream.next().await });
    accepted_rx.await.expect("adapter request reached loopback");
    next.abort();
    let _ = next.await;
    assert_eq!(
        server.await.expect("server task"),
        0,
        "pending TCP stream remained open"
    );
}

#[tokio::test]
async fn runtime_posts_to_loopback_and_translates_streamed_text_usage_and_finish() {
    let mut first = b": keepalive\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"h".to_vec();
    first.push(0xc3); // split the UTF-8 encoding of `é` across transport reads
    let mut second = vec![0xa9];
    second.extend_from_slice(
        b"\"}}]}\r\n\r\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2,\"prompt_cache_hit_tokens\":3}}\r\n\r\ndata: [DONE]\r\n\r\n",
    );
    let server = spawn_server(
        "200 OK",
        &[("Content-Type", "text/event-stream")],
        vec![first, second],
    )
    .await;

    let chunks = drive(
        &server,
        adapter(server.base_url.clone(), Some("test-secret")),
    )
    .await;
    let request = tokio::time::timeout(Duration::from_secs(1), server.request)
        .await
        .expect("loopback server did not receive a request")
        .expect("request sender dropped");

    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/chat/completions");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer test-secret")
    );
    assert_eq!(request.body["stream"], true);
    assert_eq!(
        request.body["stream_options"],
        json!({"include_usage": true})
    );
    assert_eq!(
        chunks,
        vec![
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".to_string()
            },
            StreamChunk::TextDelta {
                index: 0,
                text: "hé".to_string()
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::Text {
                    text: "hé".to_string()
                },
            },
            StreamChunk::Usage {
                usage: TokenUsage {
                    input_tokens: 4,
                    output_tokens: 2,
                    cache_read_tokens: Some(3),
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None
            },
        ]
    );
}

#[tokio::test]
async fn rejects_a_success_stream_that_exceeds_the_total_response_budget() {
    let oversized = format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\ndata: [DONE]\n\n",
        "x".repeat(8 * 1024 * 1024)
    );
    let server = spawn_server(
        "200 OK",
        &[("Content-Type", "text/event-stream")],
        vec![oversized.into_bytes()],
    )
    .await;

    let chunks = drive(
        &server,
        adapter(server.base_url.clone(), Some("test-secret")),
    )
    .await;
    let finish = chunks.last().expect("terminal finish");
    let StreamChunk::Finish {
        reason: FinishReason::Error { failure },
        ..
    } = finish
    else {
        panic!("expected response-budget error finish, got {finish:?}");
    };
    assert_eq!(failure.code, "RESPONSE_TOO_LARGE");
}

#[tokio::test]
async fn maps_auth_http_failures_to_the_stable_provider_code() {
    let server = spawn_server(
        "401 Unauthorized",
        &[("Content-Type", "application/json")],
        vec![br#"{"error":{"message":"bad key"}}"#.to_vec()],
    )
    .await;

    let chunks = drive(
        &server,
        adapter(server.base_url.clone(), Some("not-the-real-secret")),
    )
    .await;

    let finish = chunks.last().expect("terminal finish");
    let StreamChunk::Finish {
        reason: FinishReason::Error { failure },
        ..
    } = finish
    else {
        panic!("expected error finish, got {finish:?}");
    };
    assert_eq!(failure.code, "AUTH");
    assert_eq!(failure.status, Some(401));
    assert!(!failure.message.contains("not-the-real-secret"));
}
