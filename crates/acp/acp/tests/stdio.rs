use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn request(
    child: &mut std::process::Child,
    reader: &mut BufReader<std::process::ChildStdout>,
    frame: Value,
) -> Value {
    let stdin = child.stdin.as_mut().expect("piped stdin");
    writeln!(
        stdin,
        "{}",
        serde_json::to_string(&frame).expect("encode request")
    )
    .expect("write request");
    stdin.flush().expect("flush request");
    let mut line = String::new();
    reader.read_line(&mut line).expect("read response");
    serde_json::from_str(line.trim()).expect("stdout line is JSON")
}

#[test]
fn real_stdio_initializes_and_allocates_a_session_with_pure_json_stdout() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_acp-fixture"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ACP fixture");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut reader = BufReader::new(stdout);

    let initialized = request(
        &mut child,
        &mut reader,
        json!({ "jsonrpc": "2.0", "id": "init", "method": "initialize", "params": {} }),
    );
    assert_eq!(initialized["id"], "init");
    assert_eq!(
        initialized["result"]["agentInfo"]["name"],
        "deepseek-harness-acp"
    );

    let cwd = std::env::current_dir().expect("cwd");
    let created = request(
        &mut child,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "session/new",
            "params": { "cwd": cwd, "mcpServers": [] }
        }),
    );
    assert_eq!(created["id"], 7);
    assert!(
        created["result"]["sessionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{created}"
    );

    drop(child.stdin.take());
    let status = child.wait().expect("fixture exits on EOF");
    assert!(status.success());
}
