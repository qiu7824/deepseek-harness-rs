use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

#[test]
fn request_is_one_json_line_with_the_same_id() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jsonrpc-runtime-fixture"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fixture");

    let mut stdin = child.stdin.take().expect("piped stdin");
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":"req-7","method":"echo","params":{{"text":"hello"}}}}"#
    )
    .expect("write request");
    drop(stdin);

    let output = child.wait_with_output().expect("wait for fixture");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let lines: Vec<_> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "expected one NDJSON response, got {stdout:?}"
    );
    let frame: Value = serde_json::from_str(lines[0]).expect("stdout line is JSON");
    assert_eq!(
        frame,
        json!({"jsonrpc": "2.0", "id": "req-7", "result": {"text": "hello"}})
    );
}
