use dsh_llm::{
    ContentBlock, GenerateOptions, LlmAdapter, MessageSource, Role, call_id, create_message,
};
use dsh_llm_deepseek::{
    DEEPSEEK_MESSAGES_API, DeepSeekAdapter, DeepSeekAdapterOptions, DeepSeekConfig,
    DeepSeekFilesClient, ReasoningWireFormat, resolve_adapter_options,
};
use futures::StreamExt;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn request(socket: &mut TcpStream) -> (String, Vec<u8>) {
    let mut bytes = Vec::new();
    let end = loop {
        let mut buffer = [0; 2048];
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() < 2 * 1024 * 1024);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let length: usize = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .unwrap()
        .1
        .trim()
        .parse()
        .unwrap();
    while bytes.len() < end + length {
        let mut buffer = [0; 2048];
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    (head.to_ascii_lowercase(), bytes[end..end + length].to_vec())
}
async fn reply(socket: &mut TcpStream, content_type: &str, body: &str) {
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
}
fn adapter(url: String) -> DeepSeekAdapter {
    let config = resolve_adapter_options(&DeepSeekConfig {
        api: Some(DEEPSEEK_MESSAGES_API.into()),
        base_url: Some(url),
        ..Default::default()
    })
    .unwrap();
    DeepSeekAdapter::new(DeepSeekAdapterOptions {
        options: Arc::new(move || Ok(config.clone())),
        resolve_api_key: Arc::new(|_| Box::pin(async { Ok(Some("fixture-key".into())) })),
        resolve_attachments: None,
        provider_name: None,
        reasoning_wire_format: ReasoningWireFormat::DeepSeek,
    })
}
fn options() -> GenerateOptions {
    GenerateOptions {
        provider: "deepseek-official".into(),
        model: "deepseek-flash".into(),
        reasoning_effort: None,
        messages: vec![],
        system: None,
        tools: None,
        temperature: None,
        max_tokens: Some(128),
        stop: None,
        signal: None,
        session_id: Some("fixture-session".into()),
        purpose: None,
        agent_loop_request: false,
        telemetry: None,
    }
}

struct GeneratedUpload {
    remaining: u64,
    max_read: Arc<std::sync::atomic::AtomicUsize>,
}
impl tokio::io::AsyncRead for GeneratedUpload {
    fn poll_read(mut self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>, output: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        let count = output.remaining().min(self.remaining as usize);
        self.max_read.fetch_max(count, std::sync::atomic::Ordering::SeqCst);
        output.initialize_unfilled_to(count)[..count].fill(0x5a);
        output.advance(count); self.remaining -= count as u64;
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn files_upload_stream_accepts_the_full_128_mib_boundary_with_bounded_reads() {
    const SIZE: u64 = 128 * 1024 * 1024;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        let (length, mut received) = loop {
            let mut buffer = [0; 8192]; let n = socket.read(&mut buffer).await.unwrap(); assert!(n > 0);
            header.extend_from_slice(&buffer[..n]);
            if let Some(at) = header.windows(4).position(|w| w == b"\r\n\r\n") {
                let text = String::from_utf8_lossy(&header[..at]);
                let length: usize = text.lines().find_map(|line| line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case("content-length")).map(|(_, n)| n.trim().parse().unwrap())).unwrap();
                break (length, header.len()-at-4);
            }
            assert!(header.len() < 32 * 1024);
        };
        assert!(length as u64 > SIZE && (length as u64) < SIZE + 4096);
        while received < length { let mut buffer = [0; 64 * 1024]; let n = socket.read(&mut buffer).await.unwrap(); assert!(n > 0); received += n; }
        reply(&mut socket, "application/json", &format!("{{\"id\":\"file-stream\",\"type\":\"file\",\"filename\":\"image.png\",\"mime_type\":\"image/png\",\"size_bytes\":{SIZE},\"created_at\":\"2026-09-22T00:00:00Z\",\"downloadable\":true}}" )).await;
    });
    let max_read = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let client = DeepSeekFilesClient::messages(&format!("http://{address}/anthropic"), "fixture-key", Duration::from_secs(30));
    let uploaded = client.upload_stream(Box::pin(GeneratedUpload { remaining: SIZE, max_read: max_read.clone() }), SIZE, "image/png", "image.png", 604800).await.unwrap();
    server.await.unwrap();
    assert_eq!(uploaded.bytes, SIZE);
    assert!(max_read.load(std::sync::atomic::Ordering::SeqCst) <= 64 * 1024);
    let untouched = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    assert!(client.upload_stream(Box::pin(GeneratedUpload { remaining: SIZE+1, max_read: untouched.clone() }), SIZE+1, "image/png", "image.png", 604800).await.is_err());
    assert_eq!(untouched.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn request_replays_bad_history_and_returns_signed_thinking_tools_and_usage() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (head, bytes) = request(&mut socket).await;
        assert!(head.starts_with("post /anthropic/v1/messages "));
        assert!(head.contains("x-api-key: fixture-key"));
        assert!(head.contains("anthropic-version: 2023-06-01"));
        assert!(!head.contains("authorization:"));
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["input"], json!({}));
        assert_eq!(body["messages"][1]["content"][0]["is_error"], true);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["output_config"]["effort"], "high");
        let events = [
            json!({"type":"message_start","message":{"id":"response-one","model":"deepseek-flash","usage":{"input_tokens":20,"cache_read_input_tokens":50}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"checking"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signature-one"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"new-call","name":"read","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"中文.txt\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":12}}),
            json!({"type":"message_stop"}),
        ];
        let stream = events
            .into_iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        reply(&mut socket, "text/event-stream", &stream).await;
    });
    let mut request = options();
    request.messages = vec![
        create_message(
            Role::Assistant,
            vec![ContentBlock::ToolCall {
                id: call_id("old-call"),
                name: "read".into(),
                arguments: "{broken".into(),
            }],
            MessageSource::Model {
                provider: request.provider.clone(),
                model: request.model.clone(),
                replay_state: None,
            },
        ),
        create_message(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_call_id: call_id("old-call"),
                content: vec![],
                is_error: Some(true),
            }],
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ),
    ];
    let chunks = tokio::time::timeout(
        Duration::from_secs(5),
        adapter(format!("http://{address}/anthropic"))
            .stream(&request)
            .collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    let mut assembled = dsh_llm::BlockAssembler::new();
    for chunk in &chunks {
        assembled.push(chunk);
    }
    assert_eq!(
        assembled.finish(),
        dsh_llm::FinishReason::ToolCalls,
        "{chunks:?}"
    );
    assert_eq!(assembled.usage().unwrap().output_tokens, 12);
    assert_eq!(assembled.usage().unwrap().cache_read_tokens, Some(50));
    assert_eq!(
        assembled.replay_state().unwrap()["protocol"],
        DEEPSEEK_MESSAGES_API
    );
    assert_eq!(
        assembled.replay_state().unwrap()["content"][0]["signature"],
        "signature-one"
    );
    assert!(assembled.blocks().iter().any(|block|matches!(block,ContentBlock::ToolCall{arguments,..} if arguments.contains("中文.txt"))));
    server.await.unwrap();
}

#[tokio::test]
async fn messages_files_use_native_metadata_expiry_headers_and_delete_contract() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for operation in 0..3 {
            let (mut socket, _) = listener.accept().await.unwrap();
            // GET/DELETE have no Content-Length, read headers directly.
            if operation == 0 {
                let (head, body) = request(&mut socket).await;
                assert!(head.starts_with("post /anthropic/v1/files "));
                assert!(head.contains("x-api-key: fixture-key"));
                assert!(head.contains("anthropic-beta: files-api-2025-04-14"));
                assert!(!head.contains("authorization:"));
                let body = String::from_utf8_lossy(&body);
                assert!(body.contains("expires_after[seconds]"));
                assert!(!body.contains("name=\"purpose\""));
            } else {
                let mut head = Vec::new();
                loop {
                    let mut b = [0; 1024];
                    let count = socket.read(&mut b).await.unwrap();
                    assert!(count > 0);
                    head.extend_from_slice(&b[..count]);
                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8(head).unwrap().to_ascii_lowercase();
                assert!(head.starts_with(if operation == 1 {
                    "get /anthropic/v1/files/file_one "
                } else {
                    "delete /anthropic/v1/files/file_one "
                }));
            }
            let value = if operation == 2 {
                json!({"id":"file_one","type":"file_deleted"})
            } else {
                json!({"id":"file_one","type":"file","mime_type":"image/png","size_bytes":3,"created_at":"2026-09-23T00:00:00Z","filename":"image.png"})
            };
            reply(&mut socket, "application/json", &value.to_string()).await;
        }
    });
    let client = DeepSeekFilesClient::messages(
        &format!("http://{address}/anthropic/v1"),
        "fixture-key",
        Duration::from_secs(5),
    );
    let uploaded = client
        .upload(vec![1, 2, 3], "image/png", "image.png", 3600)
        .await
        .unwrap();
    assert_eq!(uploaded.expires_at, Some(uploaded.created_at + 3600));
    let retrieved = client.retrieve(&uploaded.id).await.unwrap();
    assert_eq!(retrieved.bytes, 3);
    assert_eq!(retrieved.expires_at, None);
    client.delete(&uploaded.id).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn empty_files_metadata_retains_operation_and_http_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        reply(&mut socket, "application/json", "").await;
    });
    let client = DeepSeekFilesClient::messages(
        &format!("http://{address}"),
        "fixture-key",
        Duration::from_secs(5),
    );
    let error = client
        .upload(vec![1, 2, 3], "image/png", "image.png", 3600)
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(200));
    assert_eq!(error.code, dsh_llm_deepseek::FilesErrorCode::Protocol);
    assert!(error.message.contains("upload") && error.message.contains("HTTP 200"));
    server.await.unwrap();
}
