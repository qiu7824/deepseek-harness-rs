use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

fn temp_home(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "dsh-protocol-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("protocol temp home");
    path
}

fn run_protocol(mode: &str, requests: &[Value]) -> std::process::Output {
    let home = temp_home("stdio");
    let mut child = Command::new(env!("CARGO_BIN_EXE_dsh"))
        .arg(mode)
        .env("DSH_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dsh protocol mode");
    {
        let stdin = child.stdin.as_mut().expect("piped stdin");
        for request in requests {
            writeln!(
                stdin,
                "{}",
                serde_json::to_string(request).expect("encode request")
            )
            .expect("write request");
        }
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("protocol process exits");
    let _ = std::fs::remove_dir_all(home);
    output
}

fn json_lines(output: &[u8]) -> Vec<Value> {
    String::from_utf8(output.to_vec())
        .expect("stdout utf8")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("stdout contains only NDJSON"))
        .collect()
}

#[test]
fn dsh_binary_hosts_the_sdk_jsonrpc_initialize_and_shutdown_protocol() {
    let cwd = std::env::current_dir().expect("cwd");
    let output = run_protocol(
        "__dsh-sdk-jsonrpc",
        &[
            json!({
                "jsonrpc": "2.0",
                "id": "init",
                "method": "initialize",
                "params": {
                    "cwd": cwd,
                    "provider": "deepseek-official",
                    "model": "deepseek-chat"
                }
            }),
            json!({ "jsonrpc": "2.0", "id": "stop", "method": "shutdown" }),
        ],
    );
    assert!(
        output.status.success(),
        "status={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = json_lines(&output.stdout);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(
        lines[0]["result"]["serverInfo"]["name"],
        "deepseek-harness-sdk-runtime"
    );
    assert_eq!(
        lines[1],
        json!({ "jsonrpc": "2.0", "id": "stop", "result": {} })
    );
}

#[test]
fn dsh_binary_hosts_the_acp_initialize_and_session_new_protocol() {
    let cwd = std::env::current_dir().expect("cwd");
    let output = run_protocol(
        "__dsh-acp",
        &[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/new",
                "params": { "cwd": cwd, "mcpServers": [] }
            }),
        ],
    );
    assert!(
        output.status.success(),
        "status={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = json_lines(&output.stdout);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(
        lines[0]["result"]["agentInfo"]["name"],
        "deepseek-harness-acp"
    );
    assert!(
        lines[1]["result"]["sessionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{:?}",
        lines[1]
    );
}

#[test]
fn upstream_python_sdk_drives_a_real_turn_through_the_rust_runtime() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake DeepSeek");
    let address = listener.local_addr().expect("fake address");
    listener
        .set_nonblocking(true)
        .expect("nonblocking fixture listener");
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept model request: {error}"),
            }
        };
        socket
            .set_nonblocking(false)
            .expect("accepted model socket must block");
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut buffer = [0_u8; 1024];
            let read = socket.read(&mut buffer).expect("read model request");
            assert!(read > 0, "model request closed before headers");
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
            assert!(read > 0, "model request closed before body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"SDK_OK\"}}]}\r\n\r\n",
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
        "dsh-python-sdk-rust-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(
        root.join("settings.json"),
        serde_json::to_vec_pretty(&json!({
            "llm-deepseek": { "baseURL": format!("http://{address}") }
        }))
        .expect("encode SDK settings"),
    )
    .expect("write SDK settings");
    let script = root.join("run_sdk.py");
    std::fs::write(
        &script,
        r#"import json, os
from deepseek_harness import DeepSeekHarness
with DeepSeekHarness(
    launch_args_override=(os.environ["DSH_TEST_BIN"], "__dsh-sdk-jsonrpc"),
    cwd=os.environ["DSH_TEST_CWD"],
    provider="deepseek-official",
    model="deepseek-chat",
    env={
        "DSH_HOME": os.environ["DSH_TEST_HOME"],
        "DEEPSEEK_API_KEY": "sdk-fixture-key",
    },
    request_timeout_seconds=15,
) as harness:
    result = harness.run("reply SDK_OK", session_id="sdk-main")
print(json.dumps({"text": result.final_response, "reason": result.finish_reason}))
"#,
    )
    .expect("write SDK script");
    let sdk_src = std::path::PathBuf::from(r"D:\HermesTemp\deepseek-harness\python\sdk\src");
    let output = Command::new("python")
        .arg(&script)
        .env("PYTHONPATH", sdk_src)
        .env("DSH_TEST_BIN", env!("CARGO_BIN_EXE_dsh"))
        .env("DSH_TEST_CWD", &root)
        .env("DSH_TEST_HOME", &root)
        .output()
        .expect("run upstream Python SDK");
    let _ = server.join();
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("SDK output JSON");
    assert_eq!(payload, json!({ "text": "SDK_OK", "reason": "completed" }));
}
