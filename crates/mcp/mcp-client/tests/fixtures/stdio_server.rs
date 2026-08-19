use std::io::{self, BufRead, Write};
use std::net::TcpListener;
use std::time::Duration;

use serde_json::{Value, json};

fn send(value: Value) {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &value).expect("serialize fixture response");
    writeln!(stdout).expect("newline fixture response");
    stdout.flush().expect("flush fixture response");
}

fn main() {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "happy".to_string());
    if mode == "reconnect" {
        run_reconnect_fixture();
        return;
    }
    assert_eq!(mode, "happy", "unsupported fixture mode");
    eprintln!(
        r#"fixture diagnostic: protocol-looking {{"jsonrpc":"2.0","id":999,"result":{{"wrong":true}}}}"#
    );

    let stdin = io::stdin();
    let mut initialized = false;
    let mut call_sequence = 0_u64;
    for line in stdin.lock().lines() {
        let line = line.expect("read fixture request");
        let message: Value = serde_json::from_str(&line).expect("fixture received JSON line");
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                assert!(!initialized, "initialize sent twice");
                let id = message["id"].clone();
                send(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "rust-fixture", "version": "1"}
                    }
                }));
            }
            Some("notifications/initialized") => {
                assert!(
                    message.get("id").is_none(),
                    "initialized must be a notification"
                );
                initialized = true;
            }
            Some("tools/list") => {
                assert!(
                    initialized,
                    "tools/list sent before initialized notification"
                );
                let id = message["id"].clone();
                send(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [{
                            "name": "inspect",
                            "description": "Inspect one value",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"value": {"type": "string"}},
                                "required": ["value"],
                                "additionalProperties": false
                            },
                            "outputSchema": {
                                "type": "object",
                                "properties": {
                                    "echo": {"type": "string"},
                                    "sequence": {"type": "integer"}
                                },
                                "required": ["echo", "sequence"],
                                "additionalProperties": false
                            }
                        }]
                    }
                }));
            }
            Some("tools/call") => {
                assert_eq!(
                    message["params"]["name"], "inspect",
                    "wire must use raw MCP name"
                );
                let value = message["params"]["arguments"]["value"]
                    .as_str()
                    .expect("fixture call value");
                call_sequence += 1;
                let id = message["id"].clone();
                send(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [
                            {"type": "text", "text": format!("inspected {value}")},
                            {"type": "image", "mimeType": "image/png", "data": "AA=="}
                        ],
                        "structuredContent": {"echo": value, "sequence": call_sequence}
                    }
                }));
            }
            Some(other) => panic!("unexpected fixture method: {other}"),
            None => panic!("fixture received message without method: {message}"),
        }
    }
}

fn run_reconnect_fixture() {
    let state_path = std::env::var("MCP_RECONNECT_STATE").expect("reconnect state path");
    let generation = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0)
        + 1;
    std::fs::write(&state_path, generation.to_string()).expect("publish generation");
    let trace_path = std::env::var("MCP_RECONNECT_TRACE").expect("reconnect trace path");
    append_trace(
        &trace_path,
        &format!("start:{generation}:{}", std::process::id()),
    );
    let _exclusive_listener = std::env::var("MCP_RECONNECT_PORT").ok().map(|port| {
        TcpListener::bind(format!("127.0.0.1:{port}"))
            .expect("previous MCP generation must release its exclusive port")
    });
    let hang_first = std::env::var_os("MCP_RECONNECT_HANG_FIRST").is_some();
    if generation == 2
        && let Some(release) = std::env::var_os("MCP_RECONNECT_SECOND_RELEASE")
    {
        while !std::path::Path::new(&release).exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let exit_after_call = std::env::var_os("MCP_RECONNECT_EXIT_AFTER_CALL").is_some();
    let stdin = io::stdin();
    let mut initialized = false;
    for line in stdin.lock().lines() {
        let message: Value =
            serde_json::from_str(&line.expect("request line")).expect("request JSON");
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => send(json!({
                "jsonrpc": "2.0", "id": message["id"],
                "result": {
                    "protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                    "serverInfo": {"name": "reconnect-fixture", "version": generation.to_string()}
                }
            })),
            Some("notifications/initialized") => initialized = true,
            Some("tools/list") => {
                assert!(initialized);
                send(json!({
                    "jsonrpc": "2.0", "id": message["id"],
                    "result": {"tools": [{
                        "name": "inspect", "description": "Inspect through a reconnecting generation",
                        "inputSchema": {"type": "object", "properties": {"value": {"type": "string"}}, "required": ["value"], "additionalProperties": false}
                    }]}
                }));
            }
            Some("tools/call") if generation == 1 && hang_first => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            Some("tools/call") if generation == 1 => {
                append_trace(
                    &trace_path,
                    &format!("end:{generation}:{}", std::process::id()),
                );
                std::process::exit(23);
            }
            Some("tools/call") => {
                let value = message["params"]["arguments"]["value"].as_str().unwrap();
                send(json!({
                    "jsonrpc": "2.0", "id": message["id"],
                    "result": {"content": [{"type": "text", "text": format!("generation {generation} inspected {value}")}]}
                }));
                if exit_after_call {
                    break;
                }
            }
            Some(other) => panic!("unexpected reconnect fixture method: {other}"),
            None => panic!("message without method"),
        }
    }
    append_trace(
        &trace_path,
        &format!("end:{generation}:{}", std::process::id()),
    );
}

fn append_trace(path: &str, line: &str) {
    use std::fs::OpenOptions;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("trace file");
    writeln!(file, "{line}").expect("trace line");
    file.flush().expect("trace flush");
}
