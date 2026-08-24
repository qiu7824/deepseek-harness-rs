use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
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

fn find_artifact(root: &Path, needle: &str) -> Option<std::path::PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                pending.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
            }
        } else if path.to_string_lossy().contains(needle)
            || std::fs::read_to_string(&path).is_ok_and(|text| text.contains(needle))
        {
            return Some(path);
        }
    }
    None
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

fn start_web(home: &Path, cwd: &Path) -> (ChildGuard, u16) {
    let mut child = ChildGuard(Some(
        Command::new(env!("CARGO_BIN_EXE_dsh"))
            .args(["web", "--port", "0"])
            .current_dir(cwd)
            .env("DSH_HOME", home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
        .unwrap_or_else(|| {
            let mut stderr = String::new();
            if let Some(stream) = child.child_mut().stderr.take() {
                let mut reader = BufReader::new(stream);
                let _ = reader.read_to_string(&mut stderr);
            }
            panic!("web exited without readiness: {stderr}")
        });
    let port = line
        .strip_prefix("dsh web: http://127.0.0.1:")
        .unwrap_or_else(|| panic!("unexpected readiness line: {line:?}"))
        .parse::<u16>()
        .expect("readiness port");
    let probe = "{\"type\":\"client-request\",\"rpcId\":\"ready\",\"method\":\"host.describe\",\"payload\":{}}";
    let request = format!(
        "POST /api/host.describe HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{probe}",
        probe.len()
    );
    for _ in 0..100 {
        if let Ok((status, _)) = std::panic::catch_unwind(|| raw_http(port, &request)) {
            if status == 200 {
                return (child, port);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("web API route did not become ready on port {port}")
}

fn fake_openai_server(expected_requests: usize) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake OpenAI");
    let address = listener.local_addr().expect("fake address");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..expected_requests {
            let (mut socket, _) = listener.accept().expect("accept OpenAI request");
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("fake read timeout");
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut buffer = [0_u8; 2048];
                let read = socket.read(&mut buffer).expect("read OpenAI request");
                assert!(read > 0, "OpenAI request closed before headers");
                bytes.extend_from_slice(&buffer[..read]);
                if let Some(offset) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    break offset + 4;
                }
            };
            let head = String::from_utf8(bytes[..header_end].to_vec()).expect("request head UTF-8");
            let content_length = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().expect("content length"))
                .unwrap_or(0);
            while bytes.len() - header_end < content_length {
                let mut buffer = [0_u8; 2048];
                let read = socket.read(&mut buffer).expect("read OpenAI body");
                assert!(read > 0, "OpenAI request closed before body");
                bytes.extend_from_slice(&buffer[..read]);
            }
            let recorded = String::from_utf8_lossy(&bytes).into_owned();
            let recorded = recorded
                .lines()
                .map(|line| {
                    if line.to_ascii_lowercase().starts_with("authorization:") {
                        "Authorization: Bearer [REDACTED]".to_string()
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\r\n");
            tx.send(recorded).ok();
            if head.starts_with("POST /v1/files ") {
                let body = r#"{"error":{"message":"files unavailable in fixture"}}"#;
                write!(
                    socket,
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write fake files response");
            } else {
                let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write fake response");
            }
        }
    });
    (format!("http://{address}/v1"), rx)
}

fn rpc(port: u16, method: &str, payload: serde_json::Value) -> serde_json::Value {
    let body = serde_json::json!({
        "type": "client-request",
        "rpcId": format!("web-{method}"),
        "method": method,
        "payload": payload,
    })
    .to_string();
    let (status, response) = raw_http(
        port,
        &format!(
            "POST /api/{method} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );
    assert_eq!(status, 200, "RPC {method} returned {status}: {response}");
    serde_json::from_str(&response).expect("rpc JSON")
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
    let plugin_dir = home.join("profiles").join("web");
    std::fs::create_dir_all(&plugin_dir).expect("plugin profile dir");
    std::fs::write(
        plugin_dir.join("plugins.json"),
        r#"[{"id":"fixture-noop","name":"cordis:noop","disabled":false}]"#,
    )
    .expect("plugin config");
    let (mut child, port) = start_web(&home, &root);

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

    let plugins = rpc(port, "pluginInventory.list", serde_json::json!({}));
    let plugin_entries = plugins["result"]["value"]["entries"]
        .as_array()
        .expect("plugin entries");
    assert!(
        plugin_entries
            .iter()
            .any(|entry| entry["entryId"] == "fixture-noop")
    );
    let disabled_plugin = rpc(
        port,
        "pluginInventory.setEnabled",
        serde_json::json!({"entryId": "fixture-noop", "enabled": false}),
    );
    assert_eq!(disabled_plugin["result"]["ok"], true, "{disabled_plugin}");
    assert_eq!(
        disabled_plugin["result"]["value"]["entry"]["enabled"],
        false
    );
    let persisted_plugins =
        std::fs::read_to_string(plugin_dir.join("plugins.json")).expect("persisted plugin config");
    assert!(
        persisted_plugins.contains("\"disabled\": true"),
        "{persisted_plugins}"
    );

    let described = rpc(port, "settings.describe", serde_json::json!({}));
    let namespaces = described["result"]["value"]["namespaces"]
        .as_array()
        .expect("settings namespaces");
    for namespace in ["locale", "ui-theme", "ui-conversation", "permission"] {
        assert!(
            namespaces.iter().any(|view| view["ns"] == namespace),
            "missing {namespace} settings namespace: {described}"
        );
    }
    for (namespace, field, value) in [
        ("locale", "preference", "en"),
        ("ui-theme", "preference", "dark"),
        ("ui-conversation", "busyEnter", "steer"),
    ] {
        let view = namespaces
            .iter()
            .find(|view| view["ns"] == namespace)
            .expect("settings namespace");
        let mutated = rpc(
            port,
            "settings.mutate",
            serde_json::json!({
                "ns": namespace,
                "ops": [{"op": "set", "path": [field], "value": value}],
                "expectedRevision": view["revision"]
            }),
        );
        assert_eq!(mutated["result"]["ok"], true, "{mutated}");
    }
    let pi_ai = namespaces
        .iter()
        .find(|view| view["ns"] == "llm-pi-ai")
        .expect("llm-pi-ai settings namespace");
    assert!(pi_ai["schema"]["refs"].is_object());
    assert!(pi_ai["schema"].to_string().contains("openai-completions"));

    let revision = pi_ai["revision"].as_i64().expect("revision");
    let (base_url, openai_request) = fake_openai_server(2);
    let credential = rpc(
        port,
        "credentials.set",
        serde_json::json!({"ref": "ACME_API_KEY", "value": "temporary-test-key"}),
    );
    assert_eq!(credential["result"]["ok"], true, "{credential}");
    let updated = rpc(
        port,
        "settings.mutate",
        serde_json::json!({
            "ns": "llm-pi-ai",
            "ops": [{
                "op": "set",
                "path": ["providers", "acme"],
                "value": {
                    "displayName": "Acme",
                    "apiKeyEnv": "ACME_API_KEY",
                    "api": "openai-completions",
                    "baseURL": base_url,
                    "models": [{"id": "acme-chat", "name": "Acme Chat"}]
                }
            }],
            "expectedRevision": revision
        }),
    );
    assert_eq!(updated["result"]["ok"], true, "{updated}");
    let providers = rpc(port, "llm.providers", serde_json::json!({}));
    assert!(
        providers["result"]["value"]["providers"]
            .as_array()
            .expect("providers")
            .iter()
            .any(|provider| provider["provider"] == "acme" && provider["active"] == true),
        "{providers}"
    );
    let models = rpc(port, "llm.models", serde_json::json!({}));
    assert!(models.to_string().contains("acme-chat"), "{models}");

    let created = rpc(
        port,
        "session.create",
        serde_json::json!({"cwd": root.to_string_lossy()}),
    );
    assert_eq!(created["result"]["ok"], true, "{created}");
    let session_id = created["result"]["value"]["sessionId"]
        .as_str()
        .expect("session id");
    let selected = rpc(
        port,
        "session.selectModel",
        serde_json::json!({
            "sessionId": session_id,
            "provider": "acme",
            "model": "acme-chat"
        }),
    );
    assert_eq!(selected["result"]["ok"], true, "{selected}");
    let prompted = rpc(
        port,
        "session.prompt",
        serde_json::json!({
            "sessionId": session_id,
            "mode": "queue",
            "content": [{"type": "text", "text": "reply briefly"}]
        }),
    );
    assert_eq!(prompted["result"]["ok"], true, "{prompted}");
    let request = (0..2)
        .map(|_| {
            openai_request
                .recv_timeout(Duration::from_secs(10))
                .expect("custom Base URL did not receive the expected request")
        })
        .find(|request| request.contains("\"tools\":["))
        .expect("OpenAI-compatible main request");
    assert!(
        request.starts_with("POST /v1/chat/completions HTTP/1.1"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer [redacted]")
    );
    assert!(!request.contains("temporary-test-key"));
    assert!(request.contains("\"model\":\"acme-chat\""), "{request}");
    assert!(!request.contains("\"thinking\""), "{request}");

    let (deepseek_base_url, deepseek_request) = fake_openai_server(3);
    let described = rpc(port, "settings.describe", serde_json::json!({}));
    let deepseek_view = described["result"]["value"]["namespaces"]
        .as_array()
        .expect("namespaces")
        .iter()
        .find(|view| view["ns"] == "llm-deepseek")
        .expect("DeepSeek namespace");
    let configured = rpc(
        port,
        "settings.mutate",
        serde_json::json!({
            "ns": "llm-deepseek",
            "ops": [{"op": "set", "path": ["baseURL"], "value": deepseek_base_url}],
            "expectedRevision": deepseek_view["revision"]
        }),
    );
    assert_eq!(configured["result"]["ok"], true, "{configured}");
    let credential = rpc(
        port,
        "credentials.set",
        serde_json::json!({"ref": "DEEPSEEK_API_KEY", "value": "temporary-deepseek-key"}),
    );
    assert_eq!(credential["result"]["ok"], true, "{credential}");
    let created = rpc(
        port,
        "session.create",
        serde_json::json!({"cwd": root.to_string_lossy()}),
    );
    let deepseek_session = created["result"]["value"]["sessionId"]
        .as_str()
        .expect("DeepSeek session id");
    let selected = rpc(
        port,
        "session.selectModel",
        serde_json::json!({
            "sessionId": deepseek_session,
            "provider": "deepseek-official",
            "model": "deepseek-v4-flash"
        }),
    );
    assert_eq!(selected["result"]["ok"], true, "{selected}");
    let prompted = rpc(
        port,
        "session.prompt",
        serde_json::json!({
            "sessionId": deepseek_session,
            "mode": "queue",
            "content": [
                {"type": "text", "text": "reply briefly about this image"},
                {
                    "type": "image",
                    "mediaType": "image/png",
                    "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
                    "name": "pixel.png"
                }
            ]
        }),
    );
    assert_eq!(prompted["result"]["ok"], true, "{prompted}");
    let deepseek_request = (0..3)
        .map(|_| {
            deepseek_request
                .recv_timeout(Duration::from_secs(10))
                .expect("configured DeepSeek Base URL did not receive the expected request")
        })
        .find(|request| request.contains("\"type\":\"image_url\""))
        .expect("DeepSeek main image request");
    let lower_request = deepseek_request.to_ascii_lowercase();
    let authorization = lower_request
        .lines()
        .find_map(|line| line.strip_prefix("authorization: bearer "))
        .expect("redacted bearer authorization header");
    assert_eq!(authorization.trim(), "[redacted]");
    assert!(!deepseek_request.contains("temporary-deepseek-key"));
    assert!(
        deepseek_request.contains("\"model\":\"deepseek-v4-flash\""),
        "{deepseek_request}"
    );
    assert!(
        deepseek_request.contains("\"type\":\"image_url\""),
        "{deepseek_request}"
    );
    assert!(
        deepseek_request.contains("data:image/webp;base64,UklG"),
        "{deepseek_request}"
    );

    let archived = rpc(
        port,
        "workspace.archiveSession",
        serde_json::json!({"sessionId": deepseek_session}),
    );
    assert_eq!(archived["result"]["ok"], true, "{archived}");
    let deleted = rpc(
        port,
        "workspace.deleteArchivedSession",
        serde_json::json!({"sessionId": deepseek_session}),
    );
    assert_eq!(deleted["result"]["ok"], true, "{deleted}");
    assert_eq!(deleted["result"]["value"]["deleted"], true, "{deleted}");
    let after_delete = rpc(port, "session.list", serde_json::json!({}));
    assert!(
        !after_delete.to_string().contains(deepseek_session),
        "{after_delete}"
    );
    let attached_summary = after_delete["result"]["value"]["items"]
        .as_array()
        .expect("attached session items")
        .iter()
        .find(|item| item["sessionId"] == session_id)
        .expect("attached surviving session")
        .clone();

    let sessions_root = home.join("sessions");
    let durable_deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut stable_sample = None;
    loop {
        if let Some(path) = find_artifact(&sessions_root, session_id) {
            let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            if size > 0 && stable_sample.as_ref() == Some(&(path.clone(), size)) {
                break;
            }
            stable_sample = Some((path, size));
        }
        assert!(
            std::time::Instant::now() < durable_deadline,
            "session write-behind did not publish a stable durable artifact under {}",
            sessions_root.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.child_mut().kill();
    let _ = child.child_mut().wait();
    let cold_headers = {
        let runtime = tokio::runtime::Runtime::new().expect("cold persistence runtime");
        runtime.block_on(async {
            let ctx = cordis::Context::root();
            let persistence = dsh_session_persistence_jsonl::JsonlSessionPersistence::install(
                &ctx,
                dsh_session_persistence_jsonl::JsonlConfig {
                    root: sessions_root.to_string_lossy().into_owned(),
                    pack_chunks: true,
                    compression: dsh_session_persistence_jsonl::JsonlCompression::Zstd,
                    prepared_session_cache_size: 5,
                    write_batch_max_delay_ms: 200,
                },
            )
            .expect("open isolated cold persistence");
            dsh_session_persistence::SessionPersistenceApi::list(persistence.as_ref())
                .await
                .expect("list isolated cold persistence")
        })
    };
    assert!(
        cold_headers
            .iter()
            .any(|header| header.id.as_str() == session_id),
        "isolated JSONL backend did not discover {session_id}: {cold_headers:?}"
    );
    assert!(
        home.is_dir(),
        "persistent DSH_HOME was removed during shutdown"
    );
    assert!(
        plugin_dir.join("plugins.json").is_file(),
        "persistent plugin config was removed during shutdown"
    );
    let (_restarted, restarted_port) = start_web(&home, &root);
    let restored = rpc(restarted_port, "llm.models", serde_json::json!({}));
    assert!(restored.to_string().contains("acme-chat"), "{restored}");
    let restored_plugins = rpc(
        restarted_port,
        "pluginInventory.list",
        serde_json::json!({}),
    );
    let restored_plugin = restored_plugins["result"]["value"]["entries"]
        .as_array()
        .expect("restored plugin entries")
        .iter()
        .find(|entry| entry["entryId"] == "fixture-noop")
        .expect("restored plugin");
    assert_eq!(restored_plugin["enabled"], false, "{restored_plugins}");
    let restored_sessions = rpc(restarted_port, "session.list", serde_json::json!({}));
    if !restored_sessions.to_string().contains(session_id) {
        let history = rpc(
            restarted_port,
            "session.history",
            serde_json::json!({"sessionId": session_id, "maxMessages": 100}),
        );
        panic!(
            "surviving session was not restored from DSH_HOME: {restored_sessions}; history={history}"
        );
    }
    assert!(
        !restored_sessions.to_string().contains(deepseek_session),
        "{restored_sessions}"
    );
    let cold_summary = restored_sessions["result"]["value"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("cold session items: {restored_sessions}"))
        .iter()
        .find(|item| item["sessionId"] == session_id)
        .expect("cold surviving session");
    assert_eq!(cold_summary["updatedAt"], attached_summary["updatedAt"]);
    assert_eq!(cold_summary["blank"], attached_summary["blank"]);
    let restored_settings = rpc(restarted_port, "settings.describe", serde_json::json!({}));
    let restored_namespaces = restored_settings["result"]["value"]["namespaces"]
        .as_array()
        .expect("restored settings namespaces");
    for (namespace, field, value) in [
        ("locale", "preference", "en"),
        ("ui-theme", "preference", "dark"),
        ("ui-conversation", "busyEnter", "steer"),
    ] {
        let view = restored_namespaces
            .iter()
            .find(|view| view["ns"] == namespace)
            .expect("restored namespace");
        assert_eq!(view["value"][field], value, "{restored_settings}");
    }

    let _ = std::fs::remove_dir_all(root);
}
