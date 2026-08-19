use std::fs::OpenOptions;
use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn send(output: &mut impl Write, value: Value) {
    serde_json::to_writer(&mut *output, &value).expect("encode fixture frame");
    output.write_all(b"\n").expect("write fixture newline");
    output.flush().expect("flush fixture frame");
}

fn record(path_key: &str, value: &str) {
    let Some(path) = std::env::var_os(path_key) else {
        return;
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open fixture record");
    writeln!(file, "{value}").expect("write fixture record");
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args != ["app-server", "--stdio"] {
        eprintln!("unexpected fixture argv: {args:?}");
        std::process::exit(64);
    }
    record("CODEX_FIXTURE_PID_FILE", &std::process::id().to_string());
    eprintln!("CODEX_FIXTURE_STDERR_SENTINEL");

    let mode = std::env::var("CODEX_FIXTURE_MODE").unwrap_or_else(|_| "complete".to_string());
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.expect("read fixture frame");
        if line.trim().is_empty() {
            continue;
        }
        let frame: Value = serde_json::from_str(&line).expect("parse fixture frame");
        let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
        match method {
            "initialize" => send(
                &mut stdout,
                json!({ "jsonrpc": "2.0", "id": frame["id"], "result": { "userAgent": "codex-cli fixture" } }),
            ),
            "initialized" => {}
            "thread/start" => send(
                &mut stdout,
                json!({
                    "jsonrpc": "2.0",
                    "id": frame["id"],
                    "result": { "thread": { "id": "thread-fixture", "ephemeral": true } }
                }),
            ),
            "turn/start" => {
                let text = frame["params"]["input"][0]["text"]
                    .as_str()
                    .expect("fixture input text");
                record("CODEX_FIXTURE_TRACE_FILE", &format!("turn:{text}"));
                send(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "id": frame["id"],
                        "result": { "turn": { "id": "turn-fixture" } }
                    }),
                );
                if mode == "exit-9" {
                    std::process::exit(9);
                }
                if mode == "complete" {
                    send(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "item/completed",
                            "params": {
                                "threadId": "thread-fixture",
                                "turnId": "turn-fixture",
                                "item": {
                                    "type": "agentMessage",
                                    "text": format!("fixture answered: {text}"),
                                    "phase": "final_answer"
                                }
                            }
                        }),
                    );
                    send(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "turn/completed",
                            "params": {
                                "threadId": "thread-fixture",
                                "turn": { "id": "turn-fixture", "status": "completed", "error": null }
                            }
                        }),
                    );
                }
            }
            "turn/interrupt" => {
                record("CODEX_FIXTURE_TRACE_FILE", "interrupt");
                send(
                    &mut stdout,
                    json!({ "jsonrpc": "2.0", "id": frame["id"], "result": {} }),
                );
            }
            _ => {
                if frame.get("id").is_some() {
                    send(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "id": frame["id"],
                            "error": { "code": -32601, "message": "fixture method not found" }
                        }),
                    );
                }
            }
        }
    }
}
