use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use dsh_attachment::{
    AttachmentAbort, AttachmentError, AttachmentStore, ImageAttachmentLimits, ImageAttachmentRef,
    ImageMediaType, RequestImageAttachment, RequestImagePolicy, SaveImageAttachment,
    StoredImageAttachment, attachment_id, image_variant_id,
};
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

struct StaticImageStore {
    limits: ImageAttachmentLimits,
    image: StoredImageAttachment,
}

#[async_trait::async_trait]
impl AttachmentStore for StaticImageStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.limits
    }

    async fn validate_image(&self, _input: &SaveImageAttachment) -> Result<(), AttachmentError> {
        Ok(())
    }

    async fn save_image(
        &self,
        _input: &SaveImageAttachment,
    ) -> Result<ImageAttachmentRef, AttachmentError> {
        Ok(self.image.reference.clone())
    }

    async fn read_image(
        &self,
        _reference: &ImageAttachmentRef,
        _signal: Option<&AttachmentAbort>,
    ) -> Result<StoredImageAttachment, AttachmentError> {
        Ok(self.image.clone())
    }

    async fn read_image_request(
        &self,
        reference: &ImageAttachmentRef,
        _policy: &RequestImagePolicy,
        _signal: Option<&AttachmentAbort>,
    ) -> Result<RequestImageAttachment, AttachmentError> {
        Ok(RequestImageAttachment {
            attachment_id: reference.attachment_id.clone(),
            variant_id: image_variant_id(format!("sha256:{}", "a".repeat(64))),
            media_type: ImageMediaType::Png,
            data: self.image.data.clone(),
            width: 1,
            height: 1,
        })
    }
}

struct FilesImageStore(StaticImageStore);

#[async_trait::async_trait]
impl AttachmentStore for FilesImageStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        self.0.image_limits()
    }
    async fn validate_image(&self, input: &SaveImageAttachment) -> Result<(), AttachmentError> {
        self.0.validate_image(input).await
    }
    async fn save_image(
        &self,
        input: &SaveImageAttachment,
    ) -> Result<ImageAttachmentRef, AttachmentError> {
        self.0.save_image(input).await
    }
    async fn read_image(
        &self,
        reference: &ImageAttachmentRef,
        signal: Option<&AttachmentAbort>,
    ) -> Result<StoredImageAttachment, AttachmentError> {
        self.0.read_image(reference, signal).await
    }
    async fn read_image_request(
        &self,
        reference: &ImageAttachmentRef,
        _policy: &RequestImagePolicy,
        _signal: Option<&AttachmentAbort>,
    ) -> Result<RequestImageAttachment, AttachmentError> {
        Ok(RequestImageAttachment {
            attachment_id: reference.attachment_id.clone(),
            variant_id: image_variant_id(format!("sha256:{}", "b".repeat(64))),
            media_type: ImageMediaType::Webp,
            data: vec![1, 2, 3],
            width: 1,
            height: 1,
        })
    }
}

async fn read_raw_request(socket: &mut TcpStream) -> (String, HashMap<String, String>, Vec<u8>) {
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
    let request_line = lines.next().expect("request line").to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let content_length = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let mut buffer = [0_u8; 1024];
        let read = socket.read(&mut buffer).await.expect("read body");
        assert!(read > 0, "client closed before request body");
        bytes.extend_from_slice(&buffer[..read]);
    }
    (
        request_line,
        headers,
        bytes[header_end..header_end + content_length].to_vec(),
    )
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
    let raw_body = &bytes[header_end..header_end + content_length];
    let body = serde_json::from_slice(raw_body).unwrap_or_else(|error| {
        panic!(
            "JSON request body for {path}: {error}; prefix={:?}",
            String::from_utf8_lossy(&raw_body[..raw_body.len().min(64)])
        )
    });
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
        resolve_attachments: None,
        provider_name: None,
        include_thinking_fields: true,
    }))
}

fn image_store() -> Arc<dyn AttachmentStore> {
    let reference = ImageAttachmentRef {
        attachment_id: attachment_id("test-image"),
        media_type: ImageMediaType::Png,
        bytes: 8,
        width: 1,
        height: 1,
        name: None,
    };
    Arc::new(StaticImageStore {
        limits: ImageAttachmentLimits {
            max_image_bytes: 1024,
            max_images_per_message: 4,
            max_message_image_bytes: 4096,
            max_image_pixels: 1024,
            media_types: vec![ImageMediaType::Png],
        },
        image: StoredImageAttachment {
            reference,
            data: b"test-png".to_vec(),
        },
    })
}

#[allow(clippy::result_large_err)] // Test resolver preserves the public LlmError seam.
fn image_adapter(base_url: String) -> Arc<DeepSeekAdapter> {
    let resolved = resolve_adapter_options(&DeepSeekConfig {
        base_url: Some(base_url),
        ..DeepSeekConfig::default()
    })
    .expect("valid options");
    let store = image_store();
    Arc::new(DeepSeekAdapter::new(DeepSeekAdapterOptions {
        options: Arc::new(move || Ok(resolved.clone())),
        resolve_api_key: Arc::new(move |_| async { Ok(Some("test-secret".to_string())) }.boxed()),
        resolve_attachments: Some(Arc::new(move || Some(store.clone()))),
        provider_name: None,
        include_thinking_fields: true,
    }))
}

fn files_image_adapter(base_url: String, index_path: std::path::PathBuf) -> Arc<DeepSeekAdapter> {
    let resolved = resolve_adapter_options(&DeepSeekConfig {
        base_url: Some(base_url),
        files_index_path: Some(index_path),
        files_api_timeout_ms: Some(2_000),
        ..DeepSeekConfig::default()
    })
    .expect("valid options");
    let store: Arc<dyn AttachmentStore> = Arc::new(FilesImageStore(StaticImageStore {
        limits: ImageAttachmentLimits {
            max_image_bytes: 1024,
            max_images_per_message: 4,
            max_message_image_bytes: 4096,
            max_image_pixels: 1024,
            media_types: vec![ImageMediaType::Png],
        },
        image: StoredImageAttachment {
            reference: ImageAttachmentRef {
                attachment_id: attachment_id("test-image"),
                media_type: ImageMediaType::Png,
                bytes: 8,
                width: 1,
                height: 1,
                name: None,
            },
            data: b"test-png".to_vec(),
        },
    }));
    Arc::new(DeepSeekAdapter::new(DeepSeekAdapterOptions {
        options: Arc::new(move || Ok(resolved.clone())),
        resolve_api_key: Arc::new(move |_| async { Ok(Some("test-secret".to_string())) }.boxed()),
        resolve_attachments: Some(Arc::new(move || Some(store.clone()))),
        provider_name: None,
        include_thinking_fields: true,
    }))
}

fn image_request_options() -> GenerateOptions {
    let mut options = request_options();
    options.messages[0].content.push(ContentBlock::Image {
        attachment: dsh_llm::ImageAttachmentRef {
            attachment_id: "test-image".to_string(),
            media_type: Some("image/png".to_string()),
            bytes: Some(8),
            width: Some(1),
            height: Some(1),
            name: None,
        },
    });
    options
}

async fn assert_expiring_file_is_replaced_and_deleted(delete_status: &'static str) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind expiring-file e2e");
    let address = listener.local_addr().expect("expiring-file address");
    let base_url = format!("http://{address}");
    let index_path = std::env::temp_dir().join(format!(
        "dsh-files-expiring-e2e-{}.json",
        uuid::Uuid::new_v4()
    ));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time after epoch")
        .as_millis() as u64;
    let index = dsh_llm_deepseek::DeepSeekUploadIndex::new(index_path.clone());
    index
        .commit(
            dsh_llm_deepseek::DeepSeekUploadRecord {
                scope: dsh_llm_deepseek::deepseek_file_scope(&base_url, "test-secret"),
                attachment_id: dsh_attachment::attachment_id("test-image"),
                variant_id: dsh_attachment::image_variant_id(format!("sha256:{}", "b".repeat(64))),
                file_id: dsh_llm_deepseek::deepseek_file_id("file-old"),
                bytes: 3,
                created_at: now.saturating_sub(1_000),
                expires_at: now.saturating_add(1_000),
            },
            now,
            0,
        )
        .await
        .expect("seed near-expiry mapping");

    let server = tokio::spawn(async move {
        let (mut upload, _) = listener.accept().await.expect("accept replacement upload");
        let (line, headers, _) = read_raw_request(&mut upload).await;
        assert!(line.starts_with("POST /files "), "{line}");
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer test-secret")
        );
        let uploaded = json!({
            "id": "file-new",
            "object": "file",
            "bytes": 3,
            "created_at": 2,
            "filename": "image.webp",
            "purpose": "user_data",
            "expires_at": 9_999_999_999_u64
        })
        .to_string();
        upload
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    uploaded.len(), uploaded
                )
                .as_bytes(),
            )
            .await
            .expect("replacement upload response");

        let mut saw_delete = false;
        let mut saw_chat = false;
        for _ in 0..2 {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .expect("delete and chat reached expiring-file server")
                .expect("accept delete or chat");
            let (line, headers, body) = read_raw_request(&mut socket).await;
            assert_eq!(
                headers.get("authorization").map(String::as_str),
                Some("Bearer test-secret")
            );
            if line.starts_with("DELETE /files/file-old ") {
                assert!(!saw_delete, "duplicate expired-file DELETE");
                saw_delete = true;
                let response = if delete_status.starts_with("200") {
                    br#"{"id":"file-old","object":"file","deleted":true}"#.as_slice()
                } else {
                    br#"{"error":{"message":"delete unavailable"}}"#.as_slice()
                };
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 {delete_status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            response.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .expect("delete response head");
                socket
                    .write_all(response)
                    .await
                    .expect("delete response body");
            } else if line.starts_with("POST /chat/completions ") {
                assert!(!saw_chat, "duplicate chat request");
                saw_chat = true;
                let request: Value = serde_json::from_slice(&body).expect("chat JSON body");
                let text = request.to_string();
                assert!(text.contains("file-new"), "new file id missing: {text}");
                assert!(!text.contains("file-old"), "old file id leaked: {text}");
                let stream = b"data: {\"choices\":[{\"delta\":{\"content\":\"seen\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            stream.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .expect("chat response head");
                socket.write_all(stream).await.expect("chat response body");
            } else {
                panic!("unexpected request while replacing expired file: {line}");
            }
        }
        assert!(saw_delete, "expired remote file was not deleted");
        assert!(saw_chat, "chat request was not sent");
    });

    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        files_image_adapter(base_url, index_path.clone()),
    )
    .expect("install expiring-file adapter");
    let chunks: Vec<_> = tokio::time::timeout(
        Duration::from_secs(3),
        runtime.stream(image_request_options()).collect(),
    )
    .await
    .expect("expiring-file request settled");
    server.await.expect("expiring-file server");
    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            ..
        }
    )));
    let _ = std::fs::remove_file(index_path);
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
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut options = request_options();
    let cancelled_for_options = cancelled.clone();
    options.signal = Some(Arc::new(move || {
        cancelled_for_options.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let mut stream = runtime.stream(options);
    let next = tokio::spawn(async move { stream.next().await });
    accepted_rx.await.expect("adapter request reached loopback");
    cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(1), next)
        .await
        .expect("cancelled model stream settles")
        .expect("cancelled model stream task");
    assert_eq!(
        server.await.expect("server task"),
        0,
        "pending TCP stream remained open"
    );
}

#[tokio::test]
async fn yields_the_first_reasoning_delta_before_the_provider_finishes() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind streaming loopback");
    let address = listener.local_addr().expect("streaming address");
    let (first_sent_tx, first_sent_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept streaming request");
        let _request = read_request(&mut socket).await;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("streaming response head");
        socket
            .write_all(
                b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking\"}}]}\r\n\r\n",
            )
            .await
            .expect("first reasoning frame");
        let _ = first_sent_tx.send(());
        let _ = release_rx.await;
        socket
            .write_all(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\ndata: [DONE]\r\n\r\n")
            .await
            .expect("terminal frames");
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
    let first = tokio::spawn(async move { stream.next().await });
    first_sent_rx.await.expect("provider sent first frame");
    let first = tokio::time::timeout(Duration::from_millis(500), first)
        .await
        .expect("first chunk arrived before provider finish")
        .expect("first chunk task")
        .expect("first chunk");
    assert!(
        matches!(first, StreamChunk::BlockStart { ref block_type, .. } if block_type == "reasoning")
    );
    release_tx.send(()).expect("release provider finish");
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
async fn accepts_chat_eof_after_explicit_finish_reason_without_done() {
    let server = spawn_server(
        "200 OK",
        &[("Content-Type", "text/event-stream")],
        vec![
            br#"data: {"choices":[{"delta":{"content":"ok"}}]}

data: {"choices":[{"delta":{},"finish_reason":"stop"}]}

"#
            .to_vec(),
        ],
    )
    .await;
    let chunks = drive(
        &server,
        adapter(server.base_url.clone(), Some("test-secret")),
    )
    .await;
    assert!(matches!(
        chunks.last(),
        Some(StreamChunk::Finish {
            reason: FinishReason::Stop,
            ..
        })
    ));
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
async fn image_schema_mismatch_falls_back_to_responses_input_image() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fallback loopback");
    let address = listener.local_addr().expect("fallback address");
    let (requests_tx, mut requests_rx) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let (mut upload, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("files upload reached fallback server")
            .expect("accept files upload");
        let (line, _headers, _body) = read_raw_request(&mut upload).await;
        assert!(line.starts_with("POST /files "), "{line}");
        let files_error = br#"{"error":{"message":"files unavailable"}}"#;
        upload
            .write_all(
                format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    files_error.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        upload.write_all(files_error).await.unwrap();
        upload.shutdown().await.unwrap();

        let (mut chat, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("chat fallback request reached server")
            .expect("accept chat request");
        requests_tx.send(read_request(&mut chat).await).ok();
        let error = br#"{"error":{"message":"Failed to deserialize the JSON body into the target type: messages[1]: unknown variant `image_url`, expected `text`"}}"#;
        chat.write_all(
            format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                error.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        chat.write_all(error).await.unwrap();
        chat.shutdown().await.unwrap();

        let (mut responses, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("responses fallback request reached server")
            .expect("accept responses request");
        requests_tx.send(read_request(&mut responses).await).ok();
        let stream = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"seen\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":1}}}\n\n";
        responses
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    stream.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        responses.write_all(stream).await.unwrap();
        responses.shutdown().await.unwrap();
    });

    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(&ctx, &runtime, image_adapter(format!("http://{address}"))).expect("install image route");
    let chunks: Vec<_> = tokio::time::timeout(
        Duration::from_secs(2),
        runtime.stream(image_request_options()).collect(),
    )
    .await
    .expect("image fallback settled");
    server.await.expect("fallback server");
    let chat = requests_rx.recv().await.expect("chat request");
    let responses = requests_rx.recv().await.expect("responses request");
    assert_eq!(chat.path, "/chat/completions");
    assert_eq!(responses.path, "/responses");
    let content = responses.body["input"][0]["content"]
        .as_array()
        .expect("Responses content array");
    let handle_index = content
        .iter()
        .position(|part| {
            part["type"] == "input_text" && part["text"] == "Image test-image; request image 1x1px."
        })
        .expect("stable request image handle");
    let image_index = content
        .iter()
        .position(|part| part["type"] == "input_image")
        .expect("Responses input_image");
    assert_eq!(image_index, handle_index + 1);
    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            ..
        }
    )));
}

#[tokio::test]
async fn expiring_file_is_reuploaded_and_deleted_remotely() {
    assert_expiring_file_is_replaced_and_deleted("200 OK").await;
}

#[tokio::test]
async fn remote_delete_failure_is_best_effort_and_chat_still_succeeds() {
    assert_expiring_file_is_replaced_and_deleted("500 Internal Server Error").await;
}

#[tokio::test]
async fn image_request_uploads_to_files_and_sends_a_file_id() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind files e2e");
    let address = listener.local_addr().expect("address");
    let index = std::env::temp_dir().join(format!("dsh-files-e2e-{}.json", uuid::Uuid::new_v4()));
    let server = tokio::spawn(async move {
        let (mut upload, _) = listener.accept().await.expect("accept upload");
        let (line, headers, body) = read_raw_request(&mut upload).await;
        assert!(line.starts_with("POST /files "), "{line}");
        assert!(
            headers
                .get("authorization")
                .is_some_and(|value| value == "Bearer test-secret")
        );
        assert!(String::from_utf8_lossy(&body).contains("user_data"));
        let upload_body = json!({"id":"file-e2e","object":"file","bytes":3,"created_at":1,"filename":"image.webp","purpose":"user_data","expires_at":9999999999u64}).to_string();
        upload.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", upload_body.len(), upload_body).as_bytes()).await.expect("upload response");

        let (mut chat, _) = listener.accept().await.expect("accept chat");
        let request = read_request(&mut chat).await;
        assert_eq!(request.path, "/chat/completions");
        let file_part = request.body["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .filter_map(|message| message["content"].as_array())
            .flatten()
            .find(|part| part["type"] == "file")
            .unwrap_or_else(|| panic!("file part missing from {}", request.body));
        assert_eq!(file_part["file_id"], "file-e2e");
        assert!(!request.body.to_string().contains("image_url"));
        let stream = b"data: {\"choices\":[{\"delta\":{\"content\":\"seen\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";
        chat.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", stream.len()).as_bytes()).await.expect("chat head");
        chat.write_all(stream).await.expect("chat stream");
    });

    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        files_image_adapter(format!("http://{address}"), index.clone()),
    )
    .expect("install files adapter");
    let chunks: Vec<_> = tokio::time::timeout(
        Duration::from_secs(3),
        runtime.stream(image_request_options()).collect(),
    )
    .await
    .expect("files e2e timeout");
    server.await.expect("files server");
    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            ..
        }
    )));
    assert!(
        index.is_file(),
        "upload mapping persisted to the isolated index"
    );
    let _ = std::fs::remove_file(index);
}

#[tokio::test]
async fn concurrent_streams_singleflight_same_scope_and_variant_upload() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind concurrent files e2e");
    let address = listener.local_addr().expect("address");
    let index_path = std::env::temp_dir().join(format!(
        "dsh-files-concurrent-e2e-{}.json",
        uuid::Uuid::new_v4()
    ));
    let server = tokio::spawn(async move {
        let (mut upload, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("files upload reached concurrent server")
            .expect("accept files upload");
        let (line, _, _) = read_raw_request(&mut upload).await;
        assert!(line.starts_with("POST /files "), "{line}");

        // Keep the first upload in flight long enough for the independent second stream
        // to contend on the same scope + request-image variant.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let uploaded = json!({
            "id": "file-shared",
            "object": "file",
            "bytes": 3,
            "created_at": 1,
            "filename": "image.webp",
            "purpose": "user_data",
            "expires_at": 9_999_999_999_u64
        })
        .to_string();
        upload
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    uploaded.len(), uploaded
                )
                .as_bytes(),
            )
            .await
            .expect("shared upload response");

        let stream = b"data: {\"choices\":[{\"delta\":{\"content\":\"seen\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let mut chat_file_ids = Vec::new();
        for ordinal in 1..=2 {
            let (mut chat, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap_or_else(|_| panic!("chat request {ordinal} reached concurrent server"))
                .expect("accept concurrent chat request");
            let request = read_request(&mut chat).await;
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, "/chat/completions");
            let file_id = request.body["messages"]
                .as_array()
                .expect("messages")
                .iter()
                .filter_map(|message| message["content"].as_array())
                .flatten()
                .find(|part| part["type"] == "file")
                .and_then(|part| part["file_id"].as_str())
                .unwrap_or_else(|| panic!("file part missing from {}", request.body));
            chat_file_ids.push(file_id.to_string());
            chat.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    stream.len()
                )
                .as_bytes(),
            )
            .await
            .expect("concurrent chat response head");
            chat.write_all(stream)
                .await
                .expect("concurrent chat response stream");
        }
        assert_eq!(chat_file_ids, ["file-shared", "file-shared"]);
    });

    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let adapter = files_image_adapter(format!("http://{address}"), index_path.clone());
    apply(&ctx, &runtime, adapter).expect("install shared files adapter");

    let first = runtime.stream(image_request_options()).collect::<Vec<_>>();
    let second = runtime.stream(image_request_options()).collect::<Vec<_>>();
    let (first_chunks, second_chunks) = tokio::time::timeout(Duration::from_secs(3), async move {
        tokio::join!(first, second)
    })
    .await
    .expect("concurrent files streams settled");
    server.await.expect("concurrent files server");

    for chunks in [&first_chunks, &second_chunks] {
        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                ..
            }
        )));
    }
    assert!(
        index_path.is_file(),
        "single upload mapping persisted to the shared index"
    );
    let _ = std::fs::remove_file(index_path);
}

#[tokio::test]
async fn stale_file_id_is_invalidated_reuploaded_and_retried_once() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stale-file e2e");
    let address = listener.local_addr().expect("address");
    let index_path =
        std::env::temp_dir().join(format!("dsh-files-stale-{}.json", uuid::Uuid::new_v4()));
    let scope = dsh_llm_deepseek::deepseek_file_scope(&format!("http://{address}"), "test-secret");
    let variant = dsh_attachment::image_variant_id(format!("sha256:{}", "b".repeat(64)));
    let index = dsh_llm_deepseek::DeepSeekUploadIndex::new(index_path.clone());
    index
        .commit(
            dsh_llm_deepseek::DeepSeekUploadRecord {
                scope,
                attachment_id: dsh_attachment::attachment_id("test-image"),
                variant_id: variant,
                file_id: dsh_llm_deepseek::deepseek_file_id("file-stale"),
                bytes: 3,
                created_at: 1,
                expires_at: 9_999_999_999_000,
            },
            1_000,
            86_400_000,
        )
        .await
        .expect("seed stale mapping");

    let server = tokio::spawn(async move {
        let (mut first_chat, _) = listener.accept().await.expect("accept stale chat");
        let first = read_request(&mut first_chat).await;
        assert_eq!(first.path, "/chat/completions");
        assert!(first.body.to_string().contains("file-stale"));
        let error = br#"{"error":{"message":"file_id file-stale not found"}}"#;
        first_chat
            .write_all(
                format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    error.len()
                )
                .as_bytes(),
            )
            .await
            .expect("stale error head");
        first_chat.write_all(error).await.expect("stale error body");

        let (mut upload, _) = listener.accept().await.expect("accept replacement upload");
        let (line, _, _) = read_raw_request(&mut upload).await;
        assert!(line.starts_with("POST /files "), "{line}");
        let uploaded = json!({"id":"file-fresh","object":"file","bytes":3,"created_at":2,"filename":"image.webp","purpose":"user_data","expires_at":9999999999u64}).to_string();
        upload
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    uploaded.len(), uploaded
                )
                .as_bytes(),
            )
            .await
            .expect("replacement upload response");

        let (mut second_chat, _) = listener.accept().await.expect("accept retried chat");
        let second = read_request(&mut second_chat).await;
        let text = second.body.to_string();
        assert!(text.contains("file-fresh"), "{text}");
        assert!(!text.contains("file-stale"), "{text}");
        let stream = b"data: {\"choices\":[{\"delta\":{\"content\":\"fresh\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        second_chat
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    stream.len()
                )
                .as_bytes(),
            )
            .await
            .expect("retried chat head");
        second_chat.write_all(stream).await.expect("retried stream");
    });

    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        files_image_adapter(format!("http://{address}"), index_path.clone()),
    )
    .expect("install stale-file adapter");
    let chunks: Vec<_> = tokio::time::timeout(
        Duration::from_secs(3),
        runtime.stream(image_request_options()).collect(),
    )
    .await
    .expect("stale-file retry timeout");
    server.await.expect("stale-file server");
    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            ..
        }
    )));
    let _ = std::fs::remove_file(index_path);
}

#[tokio::test]
async fn files_upload_failure_falls_back_to_an_all_inline_chat_request() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fallback");
    let address = listener.local_addr().expect("address");
    let index =
        std::env::temp_dir().join(format!("dsh-files-fallback-{}.json", uuid::Uuid::new_v4()));
    let server = tokio::spawn(async move {
        let (mut upload, _) = listener.accept().await.expect("accept upload");
        let (line, _, _) = read_raw_request(&mut upload).await;
        assert!(line.starts_with("POST /files "));
        let error = b"{\"error\":{\"message\":\"files unavailable\"}}";
        upload.write_all(format!("HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", error.len()).as_bytes()).await.expect("error head");
        upload.write_all(error).await.expect("error body");

        let (mut chat, _) = listener.accept().await.expect("accept chat");
        let request = read_request(&mut chat).await;
        let text = request.body.to_string();
        assert!(text.contains("image_url"), "{text}");
        assert!(!text.contains("file_id"), "{text}");
        let stream = b"data: {\"choices\":[{\"delta\":{\"content\":\"inline\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        chat.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", stream.len()).as_bytes()).await.expect("chat head");
        chat.write_all(stream).await.expect("chat body");
    });
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    apply(
        &ctx,
        &runtime,
        files_image_adapter(format!("http://{address}"), index.clone()),
    )
    .expect("install");
    let chunks: Vec<_> = tokio::time::timeout(
        Duration::from_secs(3),
        runtime.stream(image_request_options()).collect(),
    )
    .await
    .expect("timeout");
    server.await.expect("server");
    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            ..
        }
    )));
    assert!(
        !index.exists(),
        "failed upload must not publish an index record"
    );
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
