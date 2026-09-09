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
    adapter_for_api(base_url, None)
}

fn adapter_for_api(base_url: String, api: Option<String>) -> Arc<DeepSeekAdapter> {
    let resolved = resolve_adapter_options(&DeepSeekConfig {
        base_url: Some(base_url),
        api,
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
async fn responses_completion_settles_before_body_eof() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"finished\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"
        );
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}", body.len() + 100, body).as_bytes()).await.unwrap();
        // Keep the unfinished HTTP body open after the authoritative terminal
        // event. The consumer must settle without waiting for transport EOF.
        let mut byte = [0];
        let _ = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut byte)).await;
    });
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        adapter_for_api(format!("http://{address}"), Some("openai-responses".into())),
    )
    .unwrap();
    let chunks: Vec<_> =
        tokio::time::timeout(Duration::from_secs(2), runtime.stream(options()).collect())
            .await
            .expect("terminal event must settle immediately");
    let finishes: Vec<_> = chunks
        .iter()
        .filter_map(|chunk| {
            if let StreamChunk::Finish { reason, .. } = chunk {
                Some(reason)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(finishes.len(), 1);
    assert!(matches!(finishes[0], FinishReason::Stop));
    server.abort();
}

#[tokio::test]
async fn responses_cancel_interrupts_a_quiet_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, ready_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000\r\n\r\n").await.unwrap();
        let _ = ready.send(());
        let mut byte = [0];
        let _ = socket.read(&mut byte).await;
    });
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        adapter_for_api(format!("http://{address}"), Some("openai-responses".into())),
    )
    .unwrap();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = flag.clone();
    let mut request = options();
    request.signal = Some(Arc::new(move || {
        signal.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let stream = runtime.stream(request);
    let collection = tokio::spawn(async move { stream.collect::<Vec<_>>().await });
    ready_rx.await.unwrap();
    flag.store(true, std::sync::atomic::Ordering::SeqCst);
    let chunks = tokio::time::timeout(Duration::from_secs(2), collection)
        .await
        .expect("cancellation must not wait for idle timeout")
        .unwrap();
    assert!(chunks.iter().any(|chunk| matches!(chunk, StreamChunk::Finish { reason: FinishReason::Error { failure }, .. } if failure.code == "CANCELLED")));
    server.abort();
}

#[tokio::test]
async fn responses_transport_failure_preserves_done_only_text_and_has_one_error_finish() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        let body = "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"m0\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Received complete paragraph\"}]}}\n\n";
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len() + 128).as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        adapter_for_api(format!("http://{address}"), Some("openai-responses".into())),
    )
    .unwrap();
    let chunks: Vec<_> =
        tokio::time::timeout(Duration::from_secs(2), runtime.stream(options()).collect())
            .await
            .expect("failed stream must settle");
    server.await.unwrap();
    assert!(chunks.iter().any(|chunk| matches!(chunk, StreamChunk::BlockEnd {block:ContentBlock::Text{text}, ..} if text=="Received complete paragraph")));
    let finishes = chunks
        .iter()
        .filter_map(|chunk| {
            if let StreamChunk::Finish {
                reason,
                replay_state,
            } = chunk
            {
                Some((reason, replay_state))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(finishes.len(), 1);
    assert!(matches!(finishes[0].0, FinishReason::Error {failure} if failure.code=="TRANSPORT"));
    assert_eq!(finishes[0].1.as_ref().unwrap()["responseStatus"], "failed");
}

#[tokio::test]
async fn responses_token_limit_settles_before_eof_and_does_not_finalize_a_partial_call() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        let body = [
            serde_json::json!({"type":"response.output_item.added","output_index":0,"item":{"id":"f0","type":"function_call","status":"in_progress","call_id":"c0","name":"write","arguments":""}}),
            serde_json::json!({"type":"response.function_call_arguments.delta","item_id":"f0","delta":"{\"path\":"}),
            serde_json::json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"id":"m1","type":"message","role":"assistant","content":[{"type":"output_text","text":"Saved prefix"}]}],"usage":{"input_tokens":2,"output_tokens":32}}}),
        ].into_iter().map(|event|format!("data: {event}\n\n")).collect::<String>();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len() + 128).as_bytes()).await.unwrap();
        let mut byte = [0];
        let _ = socket.read(&mut byte).await;
    });
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        adapter_for_api(format!("http://{address}"), Some("openai-responses".into())),
    )
    .unwrap();
    let chunks: Vec<_> =
        tokio::time::timeout(Duration::from_secs(2), runtime.stream(options()).collect())
            .await
            .expect("incomplete terminal event must settle before EOF");
    server.abort();
    assert!(chunks.iter().any(|chunk| matches!(chunk, StreamChunk::BlockEnd {block:ContentBlock::Text{text},..} if text=="Saved prefix")));
    assert!(!chunks.iter().any(|chunk| matches!(
        chunk,
        StreamChunk::BlockEnd {
            block: ContentBlock::ToolCall { .. },
            ..
        }
    )));
    assert!(
        chunks
            .iter()
            .any(|chunk| matches!(chunk, StreamChunk::Usage {usage} if usage.output_tokens==32))
    );
    let finishes = chunks
        .iter()
        .filter_map(|chunk| {
            if let StreamChunk::Finish {
                reason,
                replay_state,
            } = chunk
            {
                Some((reason, replay_state))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(finishes.len(), 1);
    assert_eq!(finishes[0].0, &FinishReason::MaxTokens);
    assert_eq!(finishes[0].1.as_ref().unwrap()["truncatedToolCalls"], true);
    assert_eq!(
        finishes[0].1.as_ref().unwrap()["items"],
        serde_json::json!([])
    );
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

#[tokio::test]
async fn anthropic_message_stop_settles_before_eof_and_keeps_actual_model() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        let body = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"message_actual\",\"model\":\"actual-model\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        );
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",body.len()+100).as_bytes()).await.unwrap();
        let mut byte = [0];
        let _ = socket.read(&mut byte).await;
    });
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        adapter_for_api(
            format!("http://{address}"),
            Some("anthropic-messages".into()),
        ),
    )
    .unwrap();
    let chunks: Vec<_> =
        tokio::time::timeout(Duration::from_secs(2), runtime.stream(options()).collect())
            .await
            .expect("message_stop must settle without waiting for EOF");
    let finishes: Vec<_> = chunks
        .iter()
        .filter_map(|chunk| {
            if let StreamChunk::Finish {
                reason,
                replay_state,
            } = chunk
            {
                Some((reason, replay_state))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(finishes.len(), 1);
    assert!(matches!(finishes[0].0, FinishReason::Stop));
    let state = finishes[0].1.as_ref().unwrap();
    assert_eq!(state["requestedModel"], "test-model");
    assert_eq!(state["responseModel"], "actual-model");
    assert_eq!(state["responseId"], "message_actual");
    server.abort();
}

#[tokio::test]
async fn anthropic_cancel_interrupts_a_quiet_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, ready_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000\r\n\r\n").await.unwrap();
        let _ = ready.send(());
        let mut byte = [0];
        let _ = socket.read(&mut byte).await;
    });
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        adapter_for_api(
            format!("http://{address}"),
            Some("anthropic-messages".into()),
        ),
    )
    .unwrap();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = flag.clone();
    let mut request = options();
    request.signal = Some(Arc::new(move || {
        signal.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let stream = runtime.stream(request);
    let collection = tokio::spawn(async move { stream.collect::<Vec<_>>().await });
    ready_rx.await.unwrap();
    flag.store(true, std::sync::atomic::Ordering::SeqCst);
    let chunks = tokio::time::timeout(Duration::from_secs(2), collection)
        .await
        .expect("Anthropic cancellation must not wait for idle timeout")
        .unwrap();
    assert!(chunks.iter().any(|chunk|matches!(chunk,StreamChunk::Finish{reason:FinishReason::Error{failure},..}if failure.code=="CANCELLED")));
    server.abort();
}

fn protocol_prefix(api: &str) -> Vec<u8> {
    let events = match api {
        "openai-responses" => vec![
            serde_json::json!({"type":"response.output_text.delta","item_id":"m0","output_index":0,"delta":"Saved prefix"}),
        ],
        "anthropic-messages" => vec![
            serde_json::json!({"type":"message_start","message":{"id":"m0","model":"test-model","usage":{"input_tokens":1,"output_tokens":1}}}),
            serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Saved prefix"}}),
        ],
        _ => vec![serde_json::json!({"choices":[{"delta":{"content":"Saved prefix"}}]})],
    };
    events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>()
        .into_bytes()
}

fn protocol_terminal(api: &str) -> &'static [u8] {
    match api {
        "openai-responses" => b"data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n",
        "anthropic-messages" => b"data: {\"type\":\"content_block_stop\",\"index\":0}\n\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n",
        _ => b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    }
}

async fn collect_exact_sse(api: &str, body: Vec<u8>) -> Vec<StreamChunk> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        let mut response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).into_bytes();
        response.extend(body);
        socket.write_all(&response).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        adapter_for_api(format!("http://{address}"), Some(api.into())),
    )
    .unwrap();
    let chunks = tokio::time::timeout(Duration::from_secs(2), runtime.stream(options()).collect())
        .await
        .expect("SSE fixture must settle");
    server.await.unwrap();
    chunks
}

#[tokio::test]
async fn all_protocols_preserve_valid_frames_before_same_body_encoding_or_json_failure() {
    for api in [
        "openai-completions",
        "openai-responses",
        "anthropic-messages",
    ] {
        for suffix in [
            b"data: \xff\n\n".as_slice(),
            b"data: \xe4\xb8".as_slice(),
            b"data: {broken}\n\n".as_slice(),
        ] {
            let mut body = protocol_prefix(api);
            body.extend_from_slice(suffix);
            let chunks = collect_exact_sse(api, body).await;
            let mut assembled = dsh_llm::BlockAssembler::new();
            for chunk in &chunks {
                assembled.push(chunk);
            }
            assert!(
                assembled
                    .interrupted_blocks()
                    .iter()
                    .any(|block| matches!(block,ContentBlock::Text{text}if text=="Saved prefix")),
                "api={api}, chunks={chunks:?}"
            );
            assert!(
                matches!(assembled.finish(),FinishReason::Error{failure} if failure.code=="MALFORMED_RESPONSE"),
                "api={api}, chunks={chunks:?}"
            );
            assert_eq!(
                chunks
                    .iter()
                    .filter(|chunk| matches!(chunk, StreamChunk::Finish { .. }))
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn all_protocols_honor_the_terminal_before_unrelated_malformed_tail_bytes() {
    for api in [
        "openai-completions",
        "openai-responses",
        "anthropic-messages",
    ] {
        let mut body = protocol_prefix(api);
        body.extend_from_slice(protocol_terminal(api));
        body.extend_from_slice(b"data: \xff\n\n");
        let chunks = collect_exact_sse(api, body).await;
        let mut assembled = dsh_llm::BlockAssembler::new();
        for chunk in &chunks {
            assembled.push(chunk);
        }
        assert_eq!(
            assembled.finish(),
            FinishReason::Stop,
            "api={api}, chunks={chunks:?}"
        );
        assert_eq!(
            chunks
                .iter()
                .filter(|chunk| matches!(chunk, StreamChunk::Finish { .. }))
                .count(),
            1
        );
        assert!(
            assembled
                .blocks()
                .iter()
                .any(|block| matches!(block,ContentBlock::Text{text}if text=="Saved prefix"))
        );
    }
}

#[tokio::test]
async fn all_protocols_flush_an_unterminated_valid_final_event_at_eof() {
    for api in [
        "openai-completions",
        "openai-responses",
        "anthropic-messages",
    ] {
        let mut body = protocol_prefix(api);
        body.extend_from_slice(protocol_terminal(api));
        while body.last() == Some(&b'\n') {
            body.pop();
        }
        let chunks = collect_exact_sse(api, body).await;
        let mut assembled = dsh_llm::BlockAssembler::new();
        for chunk in &chunks {
            assembled.push(chunk);
        }
        assert_eq!(
            assembled.finish(),
            FinishReason::Stop,
            "api={api}, chunks={chunks:?}"
        );
        assert_eq!(
            chunks
                .iter()
                .filter(|chunk| matches!(chunk, StreamChunk::Finish { .. }))
                .count(),
            1
        );
    }
}
