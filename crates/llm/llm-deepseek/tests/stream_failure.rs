use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use dsh_llm::{
    ContentBlock, FinishReason, GenerateOptions, LlmRuntime, MessageSource, Role, StreamChunk,
    create_message,
};
use dsh_llm_deepseek::{
    DeepSeekAdapter, DeepSeekAdapterOptions, DeepSeekConfig, PROVIDER, ReasoningWireFormat, apply,
    resolve_adapter_options,
};
use futures::{FutureExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_request(socket: &mut TcpStream) {
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
    let headers: HashMap<String, String> = head
        .split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let mut buffer = [0_u8; 1024];
        let read = socket.read(&mut buffer).await.expect("read request body");
        assert!(read > 0, "client closed before request body");
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn options() -> GenerateOptions {
    GenerateOptions {
        provider: PROVIDER.to_string(),
        model: "test-model".to_string(),
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
        system: None,
        tools: None,
        temperature: None,
        max_tokens: Some(32),
        stop: None,
        signal: None,
        session_id: None,
        purpose: None,
        agent_loop_request: false,
    }
}

fn adapter(base_url: String) -> Arc<DeepSeekAdapter> {
    let resolved = resolve_adapter_options(&DeepSeekConfig {
        base_url: Some(base_url),
        ..DeepSeekConfig::default()
    })
    .expect("valid adapter options");
    Arc::new(DeepSeekAdapter::new(DeepSeekAdapterOptions {
        options: Arc::new(move || Ok(resolved.clone())),
        resolve_api_key: Arc::new(|_| async { Ok(Some("test-secret".to_string())) }.boxed()),
        resolve_attachments: None,
        provider_name: Some("GPT".to_string()),
        reasoning_wire_format: ReasoningWireFormat::OpenAi,
    }))
}

#[tokio::test]
async fn truncated_response_body_emits_one_terminal_error_finish() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback provider");
    let address = listener.local_addr().expect("loopback address");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept request");
        read_request(&mut socket).await;
        let partial = b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    partial.len() + 128
                )
                .as_bytes(),
            )
            .await
            .expect("write response head");
        socket
            .write_all(partial)
            .await
            .expect("write partial response body");
        socket.shutdown().await.expect("truncate response");
    });

    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(&ctx, &runtime, adapter(format!("http://{address}"))).expect("install adapter");
    let chunks: Vec<_> =
        tokio::time::timeout(Duration::from_secs(2), runtime.stream(options()).collect())
            .await
            .expect("truncated stream must settle");
    server.await.expect("provider task");

    let finishes: Vec<_> = chunks
        .iter()
        .filter_map(|chunk| match chunk {
            StreamChunk::Finish { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(finishes.len(), 1, "chunks={chunks:#?}");
    let FinishReason::Error { failure } = finishes[0] else {
        panic!("truncated body must not be accepted as success: {finishes:#?}");
    };
    assert_eq!(failure.code, "TRANSPORT");
    assert!(failure.message.contains("HTTP response body failed"));
}
