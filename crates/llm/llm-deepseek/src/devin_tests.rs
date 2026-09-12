use super::*;
use crate::devin;
use crate::devin_wire::{Encoder, Message};
use dsh_llm::{ContentBlock, ModelMessageSource, create_assistant_message};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn options() -> GenerateOptions {
    GenerateOptions {
        provider: "devin".into(),
        model: "swe-2".into(),
        reasoning_effort: None,
        max_tokens: Some(2048),
        messages: vec![],
        system: None,
        tools: None,
        temperature: None,
        stop: None,
        signal: None,
        session_id: Some("test-session".into()),
        purpose: None,
        agent_loop_request: false,
    }
}
fn connection(base: String) -> ResolvedDeepSeekOptions {
    resolve_adapter_options(&DeepSeekConfig {
        api: Some(devin::API.into()),
        base_url: Some(base),
        ..Default::default()
    })
    .unwrap()
}
fn frame(flags: u8, bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![flags];
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn catalog_uses_server_identifiers_and_excludes_disabled_private_and_router_models() {
    let mut root = Encoder::default();
    for (id, disabled, private, router) in [
        ("swe-2-high", false, false, false),
        ("disabled", true, false, false),
        ("private", false, true, false),
        ("adaptive", false, false, true),
    ] {
        let mut row = Encoder::default();
        row.text(1, id);
        row.text(22, id);
        row.number(4, u64::from(disabled));
        row.number(18, 262144);
        let mut info = Encoder::default();
        info.number(2, u64::from(private));
        info.number(25, u64::from(router));
        info.number(13, 16384);
        row.bytes(23, &info.0);
        root.bytes(1, &row.0);
    }
    let models = devin::catalog_from_bytes(&root.0).unwrap();
    assert_eq!(models["data"].as_array().unwrap().len(), 1);
    assert_eq!(models["data"][0]["id"], "swe-2-high");
    assert_eq!(models["data"][0]["context_window"], 262144);
    assert_eq!(models["data"][0]["max_output_tokens"], 16384);
    assert_eq!(models["data"][0]["api"], devin::API);
}

#[test]
fn native_request_preserves_system_tools_results_and_inline_images() {
    let chat = json!({"model":"swe-2","max_tokens":1024,"messages":[
        {"role":"system","content":"Keep the file boundary."},
        {"role":"user","content":[{"type":"text","text":"Inspect"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AQID"}}]},
        {"role":"assistant","content":"","reasoning_content":"Check first","tool_calls":[{"id":"call-a","function":{"name":"read","arguments":"{\"path\":\"a\"}"}}]},
        {"role":"tool","tool_call_id":"call-a","is_error":true,"content":"not found"}],
        "tools":[{"type":"function","function":{"name":"read","description":"Read a file","parameters":{"type":"object"}}}]});
    let bytes = devin::chat_request(&chat, "secret-token", "user-jwt", "cascade").unwrap();
    let request = Message::parse(&bytes).unwrap();
    assert_eq!(request.text(2).unwrap(), "Keep the file boundary.");
    assert_eq!(request.text(21).unwrap(), "swe-2");
    let metadata = Message::parse(request.bytes(1).unwrap().unwrap()).unwrap();
    assert_eq!(
        metadata.text(3).unwrap(),
        "devin-session-token$secret-token"
    );
    assert_eq!(metadata.text(21).unwrap(), "user-jwt");
    assert_eq!(metadata.text(29).unwrap(), "");
    let messages = request.repeated(3).unwrap();
    assert_eq!(messages.len(), 3);
    let user = Message::parse(messages[0]).unwrap();
    assert_eq!(user.number(2).unwrap(), 1);
    let images = user.repeated(10).unwrap();
    assert_eq!(Message::parse(images[0]).unwrap().text(1).unwrap(), "AQID");
    let assistant = Message::parse(messages[1]).unwrap();
    assert_eq!(assistant.number(2).unwrap(), 2);
    assert!(assistant.text(1).unwrap().starts_with("bot-"));
    let result = Message::parse(messages[2]).unwrap();
    assert_eq!(result.number(2).unwrap(), 4);
    assert_eq!(result.text(7).unwrap(), "call-a");
    assert_eq!(result.number(9).unwrap(), 1);
}

#[test]
fn native_replay_preserves_tool_failure_flags() {
    let mut options = options();
    options.messages.push(dsh_llm::create_tool_result_message(
        dsh_llm::ToolResultMessageInput {
            call_id: dsh_llm::call_id("failed-read"),
            content: vec![ContentBlock::Text {
                text: "file missing".into(),
            }],
            is_error: true,
        },
    ));
    let mut chat = json!({"model":"swe-2","messages":[{"role":"tool","tool_call_id":"failed-read","content":"file missing"}]});
    devin::prepare_replay(&mut chat, &options, "scope");
    let bytes = devin::chat_request(&chat, "token", "jwt", "cascade").unwrap();
    let request = Message::parse(&bytes).unwrap();
    let messages = request.repeated(3).unwrap();
    let result = Message::parse(messages[0]).unwrap();
    assert_eq!(result.text(7).unwrap(), "failed-read");
    assert_eq!(result.number(9).unwrap(), 1);
}

#[test]
fn signed_replay_is_bound_to_account_model_session_and_endpoint() {
    let mut options = options();
    let mut connection = connection(devin::BASE_URL.into());
    connection.account_scope = Some("account-a".into());
    let scope = devin::scope(&options, &connection, "token");
    options.messages.push(create_assistant_message(vec![ContentBlock::Text{text:"answer".into()}],ModelMessageSource{provider:"devin".into(),model:"swe-2".into(),replay_state:Some(json!({"protocol":devin::API,"scopeHash":scope,"cascadeId":"native-cascade","signature":"signed"}))}));
    let mut chat = json!({"messages":[{"role":"assistant","content":"answer"}]});
    assert_eq!(
        devin::prepare_replay(&mut chat, &options, &scope),
        "native-cascade"
    );
    assert_eq!(chat["messages"][0]["devin_state"]["signature"], "signed");
    connection.account_scope = Some("account-b".into());
    let changed = devin::scope(&options, &connection, "token");
    devin::prepare_replay(&mut chat, &options, &changed);
    assert!(chat["messages"][0].get("devin_state").is_none());
    assert!(authorized_service(devin::BASE_URL, "https://attacker.invalid").is_err());
    assert!(authorized_service(devin::BASE_URL, "http://server.codeium.com").is_err());
    assert!(
        authorized_service(
            devin::BASE_URL,
            "https://server.codeium.com@attacker.invalid"
        )
        .is_err()
    );
    assert!(authorized_service(devin::BASE_URL, "https://regional.codeium.com").is_ok());
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() < 1024 * 1024);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let header = String::from_utf8(bytes[..end].to_vec()).unwrap();
            let length = header
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return (header, bytes[end + 4..end + 4 + length].to_vec());
            }
        }
    }
}

#[tokio::test]
async fn native_http_stream_handles_split_frames_cumulative_tool_json_and_usage() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (header, body) = read_request(&mut socket).await;
        assert!(header.starts_with(&format!("POST {} ", devin::AUTH_PATH)));
        assert!(!header.contains("test-token"));
        let request = Message::parse(&body).unwrap();
        assert_eq!(
            Message::parse(request.bytes(1).unwrap().unwrap())
                .unwrap()
                .text(3)
                .unwrap(),
            "devin-session-token$test-token"
        );
        let mut auth = Encoder::default();
        auth.text(1, "user-jwt");
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/proto\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",auth.0.len()).as_bytes()).await.unwrap();
        socket.write_all(&auth.0).await.unwrap();
        drop(socket);
        let (mut socket, _) = listener.accept().await.unwrap();
        let (header, body) = read_request(&mut socket).await;
        assert!(header.starts_with(&format!("POST {} ", devin::CHAT_PATH)));
        assert!(!header.contains("test-token"));
        assert_eq!(body[0], 1);
        assert!(
            header
                .to_ascii_lowercase()
                .contains("connect-content-encoding: gzip")
        );
        assert_eq!(
            u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize,
            body.len() - 5
        );
        let decoded = unary_payload(&body).unwrap();
        let request = Message::parse(&decoded).unwrap();
        assert_eq!(request.text(21).unwrap(), "swe-2");
        let mut first = Encoder::default();
        first.text(1, "response-a");
        first.text(9, "Inspect");
        first.text(10, "signed-");
        let mut call = Encoder::default();
        call.text(1, "call-a");
        call.text(2, "read");
        call.text(3, "{\"path\":");
        first.bytes(6, &call.0);
        let mut final_ = Encoder::default();
        final_.text(1, "response-a");
        final_.text(3, "done");
        final_.text(10, "state");
        final_.number(5, 10);
        let mut call = Encoder::default();
        call.text(1, "call-a");
        call.text(3, "{\"path\":\"file.txt\"}");
        final_.bytes(6, &call.0);
        let mut usage = Encoder::default();
        usage.number(2, 10);
        usage.number(3, 6);
        usage.number(4, 3);
        usage.number(5, 20);
        final_.bytes(7, &usage.0);
        let data = [
            frame(1, &gzip(&first.0)),
            frame(0, &final_.0),
            frame(2, b"{}"),
        ]
        .concat();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/connect+proto\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",data.len()).as_bytes()).await.unwrap();
        for part in data.chunks(3) {
            socket.write_all(part).await.unwrap()
        }
    });
    let options = options();
    let connection = connection(format!("http://{address}"));
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let chat =
        json!({"model":"swe-2","max_tokens":2048,"messages":[{"role":"user","content":"check"}]});
    tokio::time::timeout(
        Duration::from_secs(5),
        request(&chat, &options, &connection, "test-token", &tx, None),
    )
    .await
    .unwrap()
    .unwrap();
    drop(tx);
    let mut chunks = Vec::new();
    while let Some(chunk) = rx.recv().await {
        chunks.push(chunk)
    }
    server.await.unwrap();
    assert!(chunks.iter().any(|chunk|matches!(chunk,StreamChunk::BlockEnd{block:ContentBlock::ToolCall{arguments,..},..} if arguments=="{\"path\":\"file.txt\"}")));
    assert_eq!(
        chunks
            .iter()
            .filter(|chunk| matches!(chunk, StreamChunk::Usage { .. }))
            .count(),
        1
    );
    assert!(chunks.iter().any(|chunk|matches!(chunk,StreamChunk::Usage{usage} if usage.input_tokens==10&&usage.output_tokens==6&&usage.cache_read_tokens==Some(20)&&usage.cache_write_tokens==Some(3))));
    let state = chunks
        .iter()
        .find_map(|chunk| {
            if let StreamChunk::Finish {
                replay_state: Some(state),
                ..
            } = chunk
            {
                Some(state)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(state["signature"], "signed-state");
    assert!(!state.to_string().contains("test-token"));
    assert!(!state.to_string().contains("user-jwt"));
}

#[test]
fn provider_errors_never_echo_session_credentials() {
    let error = safe_error(
        reqwest::StatusCode::UNAUTHORIZED,
        b"{\"message\":\"bad secret-token user-jwt\"}",
        &["secret-token", "user-jwt"],
    );
    assert!(!error.message.contains("secret-token"));
    assert!(!error.message.contains("user-jwt"));
    let token = "s".repeat(3000);
    let body = json!({"message":format!("invalid {token}")}).to_string();
    let error = safe_error(
        reqwest::StatusCode::UNAUTHORIZED,
        body.as_bytes(),
        &[&token],
    );
    assert!(
        !error.message.contains(&"s".repeat(32)),
        "redaction must precede display truncation"
    );
}

#[test]
fn gzip_expansion_cannot_exceed_the_frame_limit() {
    let payload = gzip(&vec![0u8; MAX_FRAME + 1]);
    assert!(uncompress(&payload).unwrap_err().contains("exceeds"));
}

#[tokio::test]
async fn cancellation_interrupts_an_unfinished_authentication_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sent, received) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/proto\r\nContent-Length: 1024\r\n\r\n\x0a").await.unwrap();
        let _ = sent.send(());
        std::future::pending::<()>().await;
    });
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = cancelled.clone();
    let (sender, _receiver) = tokio::sync::mpsc::channel(32);
    let worker = tokio::spawn(async move {
        let options = options();
        let connection = connection(format!("http://{address}"));
        request(
            &json!({"model":"swe-2","messages":[]}),
            &options,
            &connection,
            "token",
            &sender,
            Some(Arc::new(move || {
                signal.load(std::sync::atomic::Ordering::SeqCst)
            })),
        )
        .await
    });
    received.await.unwrap();
    cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    let result = tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(result.code, "CANCELLED");
    server.abort();
    let _ = server.await;
}
