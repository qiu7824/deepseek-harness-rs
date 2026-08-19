use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn temp_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "dsh-web-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("child")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn raw_http(port: u16, request: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect web host");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).expect("read response");
    let text = String::from_utf8(bytes).expect("response utf8");
    let (head, body) = text.split_once("\r\n\r\n").expect("response head");
    let status = head
        .split_whitespace()
        .nth(1)
        .expect("status")
        .parse::<u16>()
        .expect("numeric status");
    (status, body.to_string())
}

#[test]
fn web_cli_serves_spa_boot_data_and_host_describe_from_an_arbitrary_cwd() {
    let root = temp_root();
    let home = root.join("home");
    let mut child = ChildGuard(Some(
        Command::new(env!("CARGO_BIN_EXE_dsh"))
            .args(["web", "--port", "0"])
            .current_dir(&root)
            .env("DSH_HOME", &home)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn dsh web"),
    ));
    let stdout = child.child_mut().stdout.take().expect("web stdout");
    let (line_tx, line_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        let _ = line_tx.send(lines.next().transpose());
    });
    let line = line_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("web readiness timeout")
        .expect("read readiness")
        .expect("web exited without readiness");
    let url = line
        .strip_prefix("dsh web: http://127.0.0.1:")
        .unwrap_or_else(|| panic!("unexpected readiness line: {line:?}"));
    let port = url.parse::<u16>().expect("readiness port");

    let (root_status, html) = raw_http(
        port,
        &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(root_status, 200);
    assert!(html.contains("DeepSeek Harness"), "{html}");
    assert!(html.contains("window.__DSH_BOOT__"), "{html}");
    assert!(html.contains("\"rev\":"), "{html}");
    assert!(html.contains("\"entries\":[{"), "{html}");
    assert!(html.contains("/plugins/client-runtime.js"), "{html}");

    let body = serde_json::json!({
        "type": "client-request",
        "rpcId": "web-process-e2e",
        "method": "host.describe",
        "payload": {}
    })
    .to_string();
    let (api_status, response) = raw_http(
        port,
        &format!(
            "POST /api/host.describe HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );
    assert_eq!(api_status, 200);
    let response: serde_json::Value = serde_json::from_str(&response).expect("host.describe JSON");
    assert_eq!(response["result"]["ok"], true);
    assert_eq!(response["result"]["value"]["provider"], "deepseek-official");

    let _ = std::fs::remove_dir_all(root);
}
