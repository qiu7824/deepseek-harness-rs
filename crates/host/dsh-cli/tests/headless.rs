use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn configure_deepseek(home: &std::path::Path, base_url: &str) {
    std::fs::create_dir_all(home).expect("create isolated headless home");
    std::fs::write(
        home.join("settings.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "llm-deepseek": { "baseURL": base_url }
        }))
        .expect("encode headless settings"),
    )
    .expect("write isolated headless settings");
}

fn temp_home() -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "dsh-headless-e2e-{}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("temp home");
    path
}

#[test]
fn headless_cli_calls_deepseek_flushes_and_prints_the_last_assistant_text() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake DeepSeek");
    let address = listener.local_addr().expect("fake address");
    let (request_tx, request_rx) = mpsc::channel();
    listener
        .set_nonblocking(true)
        .expect("nonblocking fake listener");
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok((socket, address)) => {
                    socket
                        .set_nonblocking(false)
                        .expect("blocking accepted socket");
                    break (socket, address);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "dsh headless never connected to fake DeepSeek"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept DeepSeek request: {error}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut buffer = [0_u8; 1024];
            let read = socket.read(&mut buffer).expect("read request");
            assert!(read > 0, "client closed before request headers");
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
            let read = socket.read(&mut buffer).expect("read request body");
            assert!(read > 0, "client closed before request body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        request_tx
            .send(String::from_utf8(bytes).expect("utf8 request"))
            .expect("capture request");

        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"pong\"}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).expect("write SSE");
        socket.flush().expect("flush SSE");
    });

    let home = temp_home();
    configure_deepseek(&home, &format!("http://{address}"));
    std::fs::write(home.join("AGENTS.md"), "HEADLESS_GLOBAL_INSTRUCTION")
        .expect("write global instruction fixture");
    let workspace = temp_home();
    std::fs::create_dir(workspace.join(".git")).expect("create instruction root marker");
    std::fs::write(
        workspace.join("AGENTS.md"),
        "HEADLESS_WORKSPACE_INSTRUCTION",
    )
    .expect("write headless instruction fixture");
    let mut child = Command::new(env!("CARGO_BIN_EXE_dsh"))
        .args(["--profile", "headless", "reply with pong"])
        .current_dir(&workspace)
        .env("DSH_HOME", &home)
        .env("DSH_DEEPSEEK_BASE_URL", format!("http://{address}"))
        // Explicit non-sensitive fixture key for wire-level verification.
        .env("DEEPSEEK_API_KEY", "dsh-test-key")
        .env("DSH_DEEPSEEK_MODEL", "deepseek-chat")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run dsh headless");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut request = None;
    loop {
        if request.is_none() {
            request = request_rx.try_recv().ok();
        }
        if child.try_wait().expect("poll dsh headless").is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().expect("kill timed-out dsh headless");
            let output = child.wait_with_output().expect("collect timed-out output");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let _ = server.join();
            panic!(
                "dsh headless exceeded 15s (request_received={}): stdout={stdout:?} stderr={stderr:?}",
                request.is_some()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child
        .wait_with_output()
        .expect("collect dsh headless output");
    server.join().expect("fake server");
    let request = request
        .or_else(|| request_rx.recv_timeout(Duration::from_secs(1)).ok())
        .expect("captured request");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");

    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(stdout.trim(), "pong");
    assert!(request.starts_with("POST /chat/completions HTTP/1.1"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer dsh-test-key")
    );
    assert!(request.contains("reply with pong"));
    assert!(request.contains("HEADLESS_WORKSPACE_INSTRUCTION"));
    assert!(request.contains("HEADLESS_GLOBAL_INSTRUCTION"));
    assert!(request.contains("所有用户可见的推理摘要"));
    assert!(request.contains("Current DSH file policy:"));
    assert!(request.contains("Approval policy: ask."));
    assert!(request.contains("available_skills"));
    assert!(request.contains("dsh-badge"));
    assert!(home.join("profiles/headless/package.json").exists());

    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(workspace);
}

fn accept_request(listener: &TcpListener, deadline: std::time::Instant) -> std::net::TcpStream {
    loop {
        match listener.accept() {
            Ok((socket, _)) => {
                socket
                    .set_nonblocking(false)
                    .expect("blocking accepted socket");
                return socket;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "dsh headless did not make the expected DeepSeek request"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept DeepSeek request: {error}"),
        }
    }
}

fn drain_request(socket: &mut std::net::TcpStream) -> String {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buffer = [0_u8; 1024];
        let read = socket.read(&mut buffer).expect("read request");
        assert!(read > 0, "client closed before request headers");
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
        let read = socket.read(&mut buffer).expect("read request body");
        assert!(read > 0, "client closed before request body");
        bytes.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(bytes).expect("utf8 request")
}

#[test]
fn headless_cli_does_not_hide_the_latest_error_behind_earlier_assistant_text() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake DeepSeek");
    let address = listener.local_addr().expect("fake address");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut first = accept_request(&listener, deadline);
        let _ = drain_request(&mut first);
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"stale\",\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"get_goal\",\"arguments\":\"{}\"}}]}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        first
            .write_all(response.as_bytes())
            .expect("first response");
        first.flush().expect("flush first response");

        let mut second = accept_request(&listener, deadline);
        let _ = drain_request(&mut second);
        let body = r#"{"error":{"message":"expired fixture key"}}"#;
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nX-Request-Id: request-two\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        second
            .write_all(response.as_bytes())
            .expect("second response");
        second.flush().expect("flush second response");
    });

    let home = temp_home();
    configure_deepseek(&home, &format!("http://{address}"));
    let output = Command::new(env!("CARGO_BIN_EXE_dsh"))
        .args(["--profile", "headless", "use the goal tool"])
        .env("DSH_HOME", &home)
        .env("DSH_DEEPSEEK_BASE_URL", format!("http://{address}"))
        .env("DEEPSEEK_API_KEY", "dsh-error-key")
        .output()
        .expect("run two-step dsh headless");
    server.join().expect("fake server");

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        !output.status.success(),
        "stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(!stdout.contains("stale"), "{stdout}");
    assert!(stderr.contains("[AUTH]"), "{stderr}");
    assert!(stderr.contains("expired fixture key"), "{stderr}");
    assert!(!stderr.contains("dsh-error-key"), "{stderr}");

    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn headless_interrupt_cancels_flushes_and_shuts_down() {
    let isolated_temp = temp_home().join("temp");
    std::fs::create_dir_all(&isolated_temp).expect("isolated temp");
    let mut child = Command::new(std::env::current_exe().expect("headless test executable"))
        .args([
            "--exact",
            "headless_interrupt_child_cancels_flushes_and_shuts_down",
            "--nocapture",
        ])
        .env("DSH_HEADLESS_INTERRUPT_CHILD", "1")
        .env("TEMP", &isolated_temp)
        .env("TMP", &isolated_temp)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn isolated interrupt child");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait().expect("poll interrupt child").is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().expect("kill timed-out interrupt child");
            let output = child.wait_with_output().expect("collect interrupt child");
            panic!(
                "interrupt child exceeded 15s: stdout={:?} stderr={:?}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("collect interrupt child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "interrupt child failed: stdout={stdout:?} stderr={stderr:?}"
    );
    let _ = std::fs::remove_dir_all(isolated_temp.parent().expect("isolated temp has parent"));
}

#[test]
fn headless_interrupt_child_cancels_flushes_and_shuts_down() {
    if std::env::var_os("DSH_HEADLESS_INTERRUPT_CHILD").is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("interrupt child runtime");
    runtime.block_on(async {
        let before: std::collections::HashSet<_> = std::fs::read_dir(std::env::temp_dir())
            .expect("read isolated temp baseline")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("dsh-host-"))
            .map(|entry| entry.path())
            .collect();
        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", "dsh-interrupt-key");
        }
        let home = temp_home().join("headless-home");
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dsh_host_cli::run_profile_with_interrupt(
                dsh_host_cli::RunProfileRequest {
                    profile: "headless".to_string(),
                    patches: Vec::new(),
                    args: vec!["interrupt before model admission".to_string()],
                    home,
                    telemetry_env: None,
                    install_anchor: None,
                },
                Some(Box::pin(async { Ok(()) })),
            ),
        )
        .await
        .expect("an already-delivered interrupt must settle the full Host lifecycle");
        match result {
            Ok(handle) => {
                let _ = handle.shutdown().await;
                panic!("headless interrupt was ignored");
            }
            Err(error) => {
                assert!(error.contains("interrupted"), "{error}");
                assert!(!error.contains("dsh-interrupt-key"), "{error}");
            }
        }
        let leaked: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .expect("read isolated temp")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("dsh-host-"))
            .map(|entry| entry.path())
            .filter(|path| !before.contains(path))
            .collect();
        assert!(
            leaked.is_empty(),
            "interrupt leaked Host data roots: {leaked:?}"
        );
    });
}

#[test]
fn headless_cli_fails_loud_when_the_deepseek_key_is_missing() {
    let home = temp_home();
    let output = Command::new(env!("CARGO_BIN_EXE_dsh"))
        .args(["--profile", "headless", "reply with pong"])
        .env("DSH_HOME", &home)
        .env("DSH_DEEPSEEK_BASE_URL", "http://127.0.0.1:9")
        .env_remove("DEEPSEEK_API_KEY")
        .output()
        .expect("run dsh headless without credentials");

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(!output.status.success());
    assert!(stdout.trim().is_empty(), "{stdout}");
    assert!(stderr.contains("MISSING_CREDENTIAL"), "{stderr}");
    assert!(stderr.contains("DEEPSEEK_API_KEY"), "{stderr}");

    let _ = std::fs::remove_dir_all(home);
}
