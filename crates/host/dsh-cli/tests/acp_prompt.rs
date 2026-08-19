use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

fn spawn_acp(base_url: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_dsh"))
        .arg("__dsh-acp")
        .env("DSH_DEEPSEEK_BASE_URL", base_url)
        .env("DEEPSEEK_API_KEY", "acp-fixture-key")
        .env("DSH_DEEPSEEK_MODEL", "deepseek-chat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ACP runtime")
}

fn send(child: &mut Child, frame: Value) {
    let input = child.stdin.as_mut().expect("piped ACP stdin");
    writeln!(
        input,
        "{}",
        serde_json::to_string(&frame).expect("encode ACP frame")
    )
    .expect("write ACP frame");
    input.flush().expect("flush ACP frame");
}

fn read_frame(reader: &mut BufReader<std::process::ChildStdout>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read ACP frame");
    assert!(!line.is_empty(), "ACP stdout ended before response");
    serde_json::from_str(line.trim()).expect("ACP stdout contains only NDJSON")
}

fn start_session(
    child: &mut Child,
    reader: &mut BufReader<std::process::ChildStdout>,
    cwd: &std::path::Path,
) -> String {
    send(
        child,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
    );
    let initialized = read_frame(reader);
    assert_eq!(initialized["id"], 1);
    send(
        child,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": cwd, "mcpServers": [] }
        }),
    );
    let created = read_frame(reader);
    created["result"]["sessionId"]
        .as_str()
        .expect("ACP session id")
        .to_string()
}

#[test]
fn acp_prompt_streams_committed_text_and_settles_at_whole_agent_idle() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake DeepSeek");
    let address = listener.local_addr().expect("fake address");
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept model request");
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("model read timeout");
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut buffer = [0_u8; 1024];
            let read = socket.read(&mut buffer).expect("read model request");
            assert!(read > 0, "model request ended before headers");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(offset) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break offset + 4;
            }
        };
        let head = std::str::from_utf8(&bytes[..header_end]).expect("request head");
        let content_length = head
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content length"))
                })
            })
            .expect("content-length");
        while bytes.len() - header_end < content_length {
            let mut buffer = [0_u8; 1024];
            let read = socket.read(&mut buffer).expect("read model body");
            assert!(read > 0, "model request ended before body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ACP_OK\"}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket
            .write_all(response.as_bytes())
            .expect("write model response");
    });

    let root = std::env::temp_dir().join(format!(
        "dsh-acp-prompt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("ACP workspace");
    let mut child = spawn_acp(&format!("http://{address}"));
    let stdout = child.stdout.take().expect("piped ACP stdout");
    let mut reader = BufReader::new(stdout);
    let session_id = start_session(&mut child, &mut reader, &root);
    send(
        &mut child,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "reply ACP_OK" }]
            }
        }),
    );

    let mut committed = Vec::new();
    let response = loop {
        let frame = read_frame(&mut reader);
        if frame["method"] == "session/update"
            && let Some(text) = frame["params"]["update"]["content"]["text"].as_str()
        {
            committed.push(text.to_string());
        }
        if frame["id"] == 3 {
            break frame;
        }
    };
    assert_eq!(committed, vec!["ACP_OK"]);
    assert_eq!(response["result"]["stopReason"], "end_turn");

    drop(child.stdin.take());
    let output = child.wait_with_output().expect("ACP runtime exits on EOF");
    server.join().expect("model fixture");
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn acp_cancel_interrupts_an_inflight_prompt_and_settles_cancelled() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind pending model");
    let address = listener.local_addr().expect("pending model address");
    let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept pending model request");
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("pending read timeout");
        let mut buffer = [0_u8; 4096];
        let read = socket
            .read(&mut buffer)
            .expect("read pending model request");
        assert!(read > 0);
        accepted_tx.send(()).expect("model request accepted");
        let mut tail = [0_u8; 64];
        loop {
            match socket.read(&mut tail) {
                Ok(0) => return,
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    return;
                }
                Err(error) => panic!("pending model socket did not close: {error}"),
            }
        }
    });

    let root = std::env::temp_dir().join(format!(
        "dsh-acp-cancel-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("ACP cancel workspace");
    let mut child = spawn_acp(&format!("http://{address}"));
    let stdout = child.stdout.take().expect("piped ACP stdout");
    let mut reader = BufReader::new(stdout);
    let session_id = start_session(&mut child, &mut reader, &root);
    send(
        &mut child,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "wait forever" }]
            }
        }),
    );
    accepted_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("prompt reached pending model");
    send(
        &mut child,
        json!({
            "jsonrpc": "2.0", "method": "session/cancel",
            "params": { "sessionId": session_id }
        }),
    );

    let (frame_tx, frame_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let frame = read_frame(&mut reader);
            if frame["id"] == 3 {
                let _ = frame_tx.send(frame);
                return;
            }
        }
    });
    let response = frame_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("cancelled prompt settles promptly");
    assert_eq!(response["result"]["stopReason"], "cancelled");

    drop(child.stdin.take());
    let output = child.wait_with_output().expect("ACP cancel runtime exits");
    server.join().expect("pending model closes after cancel");
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}
