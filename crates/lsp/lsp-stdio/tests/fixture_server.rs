use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

fn main() {
    eprintln!("fixture diagnostic on stderr");
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut open_documents = HashMap::<String, String>::new();
    let mut duplicate_opens = HashSet::<String>::new();
    loop {
        let Some(message) = read_message(&mut input) else {
            return;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = message.get("id").cloned();
        match method {
            "initialize" => send(
                &mut output,
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "capabilities": { "positionEncoding": "utf-16", "textDocumentSync": 1, "definitionProvider": true } }
                }),
                false,
            ),
            "textDocument/definition" => {
                send(
                    &mut output,
                    json!({ "jsonrpc": "2.0", "id": 999, "result": null }),
                    false,
                );
                send(
                    &mut output,
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": [{
                            "uri": "file:///workspace/src/definition.rs",
                            "range": {
                                "start": { "line": 4, "character": 2 },
                                "end": { "line": 4, "character": 8 }
                            }
                        }]
                    }),
                    true,
                );
            }
            "textDocument/references" => send(
                &mut output,
                json!({
                    "jsonrpc": "2.0", "id": id, "result": [
                        {
                            "uri": "file:///workspace/src/main.rs",
                            "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 7 } }
                        },
                        {
                            "uri": "file:///workspace/tests/reference.rs",
                            "range": { "start": { "line": 2, "character": 1 }, "end": { "line": 2, "character": 5 } }
                        }
                    ]
                }),
                false,
            ),
            "textDocument/implementation" => send(
                &mut output,
                json!({
                    "jsonrpc": "2.0", "id": id, "result": [{
                        "uri": "file:///workspace/src/implementation.rs",
                        "range": { "start": { "line": 8, "character": 4 }, "end": { "line": 8, "character": 10 } }
                    }]
                }),
                false,
            ),
            "textDocument/hover" => {
                let uri = message["params"]["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or_default();
                if duplicate_opens.remove(uri) {
                    send(
                        &mut output,
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": "document was opened twice without didClose" } }),
                        false,
                    );
                } else if open_documents
                    .get(uri)
                    .is_some_and(|text| text == "request-error")
                {
                    send(
                        &mut output,
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32001, "message": "fixture request error" } }),
                        false,
                    );
                } else {
                    send(
                        &mut output,
                        json!({
                            "jsonrpc": "2.0", "id": id, "result": {
                                "contents": format!("fn main() [pid={}]", std::process::id()),
                                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 7 } }
                            }
                        }),
                        false,
                    );
                }
            }
            "textDocument/didOpen" => {
                let document = &message["params"]["textDocument"];
                let uri = document["uri"].as_str().unwrap_or_default().to_string();
                let text = document["text"].as_str().unwrap_or_default().to_string();
                if open_documents.insert(uri.clone(), text).is_some() {
                    duplicate_opens.insert(uri);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = message["params"]["textDocument"]["uri"].as_str() {
                    open_documents.remove(uri);
                }
            }
            "shutdown" => send(
                &mut output,
                json!({ "jsonrpc": "2.0", "id": id, "result": null }),
                false,
            ),
            "exit" => return,
            _ => {}
        }
    }
}

fn read_message(input: &mut impl Read) -> Option<Value> {
    let mut header = Vec::new();
    let mut byte = [0_u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        if input.read_exact(&mut byte).is_err() {
            return None;
        }
        header.push(byte[0]);
    }
    let header = String::from_utf8(header).ok()?;
    let length = header
        .split("\r\n")
        .find_map(|line| line.strip_prefix("Content-Length: "))?
        .parse::<usize>()
        .ok()?;
    let mut body = vec![0; length];
    input.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(output: &mut impl Write, message: Value, split: bool) {
    let body = serde_json::to_vec(&message).expect("serialize fixture response");
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend(body);
    if split {
        let midpoint = frame.len() / 2;
        output.write_all(&frame[..midpoint]).expect("first half");
        output.flush().expect("flush first half");
        thread::sleep(Duration::from_millis(20));
        output.write_all(&frame[midpoint..]).expect("second half");
    } else {
        output.write_all(&frame).expect("response");
    }
    output.flush().expect("flush response");
}
