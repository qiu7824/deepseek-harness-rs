#[cfg(windows)]
mod windows {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::Command;
    use std::time::Duration;

    fn temp_root() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dsh-sandbox-headless-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn accept(listener: &TcpListener) -> TcpStream {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
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
                        "DeepSeek request timeout"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept request: {error}"),
            }
        }
    }

    fn request(socket: &mut TcpStream) -> String {
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut buffer = [0_u8; 2048];
            let read = socket.read(&mut buffer).expect("read request");
            assert!(read > 0, "request closed early");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break offset + 4;
            }
        };
        let head = std::str::from_utf8(&bytes[..header_end]).expect("headers utf8");
        let length = head
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content length"))
                })
            })
            .expect("content-length");
        while bytes.len() - header_end < length {
            let mut buffer = [0_u8; 2048];
            let read = socket.read(&mut buffer).expect("read body");
            assert!(read > 0, "body closed early");
            bytes.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8(bytes).expect("request utf8")
    }

    fn respond(socket: &mut TcpStream, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).expect("write SSE");
        socket.flush().expect("flush SSE");
    }

    #[test]
    fn default_headless_pwsh_is_workspace_write_in_the_real_dsh_process() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake DeepSeek");
        let address = listener.local_addr().expect("fake address");
        listener.set_nonblocking(true).expect("nonblocking");
        let root = temp_root();
        let workspace = root.join("workspace");
        let home = root.join("home");
        let inside = workspace.join("inside.txt");
        let outside = root.join("outside.txt");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let escape = |path: &std::path::Path| path.to_string_lossy().replace('\'', "''");
        let command = format!(
            "$i=$false;$o=$false;try{{Set-Content -LiteralPath '{}' x -ErrorAction Stop;$i=$true}}catch{{}};try{{Set-Content -LiteralPath '{}' x -ErrorAction Stop;$o=$true}}catch{{}};Write-Output \"workspace-write:inside=$($i.ToString().ToLower()),outside=$($o.ToString().ToLower())\"",
            escape(&inside),
            escape(&outside)
        );
        let server = std::thread::spawn(move || {
            let mut first = accept(&listener);
            let _ = request(&mut first);
            let arguments = serde_json::json!({
                "command": command,
                "description": "verify workspace-write sandbox"
            })
            .to_string();
            let body = format!(
                "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"sandbox-call\",\"function\":{{\"name\":\"pwsh\",\"arguments\":{}}}}}]}}}}]}}\r\n\r\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}],\"usage\":{{\"prompt_tokens\":3,\"completion_tokens\":1}}}}\r\n\r\ndata: [DONE]\r\n\r\n",
                serde_json::to_string(&arguments).expect("arguments json")
            );
            respond(&mut first, &body);
            let mut second = accept(&listener);
            let request = request(&mut second);
            assert!(
                request.contains("workspace-write:inside=true,outside=false"),
                "{request}"
            );
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"sandbox complete\"}}]}\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\r\n\r\n",
                "data: [DONE]\r\n\r\n"
            );
            respond(&mut second, body);
        });

        let output = Command::new(env!("CARGO_BIN_EXE_dsh"))
            .args(["--profile", "headless", "verify the sandbox"])
            .current_dir(&workspace)
            .env("DSH_HOME", &home)
            .env("DSH_DEEPSEEK_BASE_URL", format!("http://{address}"))
            .env("DEEPSEEK_API_KEY", "dsh-sandbox-key")
            .env("DSH_DEEPSEEK_MODEL", "deepseek-chat")
            .env_remove("DSH_SANDBOX_WINDOWS_RUNNER")
            .output()
            .expect("run headless profile");
        server.join().expect("fake server");
        let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        let inside_exists = inside.exists();
        let outside_exists = outside.exists();
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            output.status.success(),
            "stdout={stdout:?} stderr={stderr:?}"
        );
        assert_eq!(stdout.trim(), "sandbox complete");
        assert!(
            inside_exists,
            "workspace-write sandbox did not write inside workspace"
        );
        assert!(!outside_exists, "workspace-write sandbox escaped workspace");
        assert!(!stderr.contains("dsh-sandbox-key"), "{stderr}");
    }
}
