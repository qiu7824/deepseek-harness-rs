use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn fixture() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let worker = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                let boundary = loop {
                    let length = stream.read(&mut buffer).await.unwrap();
                    if length == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&buffer[..length]);
                    assert!(bytes.len() < 64 * 1024);
                    if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&bytes[..boundary]).to_string();
                let length = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, length)| length.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                while bytes.len() < boundary + length {
                    let count = stream.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let path = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let request: Value = serde_json::from_slice(&bytes[boundary..boundary + length])
                    .unwrap_or(Value::Null);
                let status = if path == "/broken" {
                    "503 Service Unavailable"
                } else {
                    "200 OK"
                };
                let result = match request["method"].as_str() {
                    Some("tools/list") => {
                        let names = match path {
                            "/candidate" => vec!["new", "collision"],
                            "/extra" => vec!["collision"],
                            _ => vec!["old"],
                        };
                        json!({"tools":names.into_iter().map(|name|json!({"name":name,"description":"Fixture","inputSchema":{"type":if path=="/invalid"{"string"}else{"object"},"properties":{}}})).collect::<Vec<_>>()})
                    }
                    Some("tools/call") => {
                        json!({"content":[{"type":"text","text":request["params"]["name"]}]})
                    }
                    _ => json!({}),
                };
                let body = json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (address, worker)
}

#[tokio::test]
async fn failed_mcp_startup_or_catalog_activation_preserves_the_working_connection_and_tools() {
    let (base, server) = fixture().await;
    let directory =
        std::env::temp_dir().join(format!("dsh-mcp-replacement-{}", std::process::id()));
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    SkillRegistry::install(&ctx, Default::default()).unwrap();
    let tools = dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
    let manager =
        CapabilityManager::install(&ctx, directory.clone(), directory.to_string_lossy().into())
            .await
            .unwrap();
    let config = |path: &str| json!({"name":"fixture","transport":"http","endpoint":format!("{base}{path}"),"enabled":true});
    let first = manager
        .invoke("capabilities.serverSave", json!({"server":config("/old")}))
        .await
        .unwrap();
    assert_eq!(first["status"], "connected");
    assert_eq!(first["toolCount"], 1);
    let original = match manager
        .state
        .lock()
        .await
        .connections
        .get("fixture")
        .unwrap()
    {
        Connection::Http(client) => client.clone(),
        _ => panic!("HTTP fixture"),
    };
    let original_name = tools.schemas(None)[0].name.clone();
    for path in ["/broken", "/invalid"] {
        let rejected = manager
            .invoke("capabilities.serverSave", json!({"server":config(path)}))
            .await
            .unwrap();
        assert!(rejected["error"].as_str().unwrap().contains("保留原有"));
        assert_eq!(rejected["toolCount"], 1);
        let state = manager.state.lock().await;
        let Connection::Http(current) = state.connections.get("fixture").unwrap() else {
            panic!("HTTP fixture")
        };
        assert!(Arc::ptr_eq(current, &original));
        assert_eq!(tools.schemas(None)[0].name, original_name);
    }
    let extra = RemoteHttpClient::connect(
        &ctx,
        RemoteHttpConfig {
            server_name: "fixture".into(),
            endpoint: format!("{base}/extra"),
            headers: Default::default(),
            request_timeout: Duration::from_secs(2),
        },
    )
    .await
    .unwrap();
    let rejected = manager
        .invoke(
            "capabilities.serverSave",
            json!({"server":config("/candidate")}),
        )
        .await
        .unwrap();
    assert!(rejected["error"].as_str().unwrap().contains("保留原有"));
    assert_eq!(
        tools.schemas(None).len(),
        2,
        "partial candidate registrations were retracted"
    );
    let output = tools
        .execute(dsh_tools::ToolExecutionInput {
            call_id: dsh_llm::call_id("mcp-old-call"),
            root_call_id: None,
            name: original_name,
            arguments: json!({}),
            agent: None,
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;
    assert!(!output.is_error, "previous transport remains callable");
    assert!(
        output
            .content
            .iter()
            .any(|block| matches!(block,dsh_llm::ContentBlock::Text{text} if text=="old"))
    );
    manager
        .invoke(
            "capabilities.serverToggle",
            json!({"name":"fixture","enabled":false}),
        )
        .await
        .unwrap();
    extra.close().await.unwrap();
    assert!(tools.schemas(None).is_empty());
    server.abort();
    drop(manager);
    std::fs::remove_dir_all(directory).unwrap();
}
