use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use dsh_llm::{ContentBlock, call_id};
use dsh_mcp_client::{StdioClient, StdioConfig};
use dsh_subprocess::SubprocessRuntime;
use dsh_subprocess_local::LocalSubprocessRuntime;
use dsh_tools::{ToolExecutionInput, ToolRuntime};
use serde_json::json;

fn fixture_path() -> String {
    env!("CARGO_BIN_EXE_mcp-test-fixture").to_string()
}

fn mount_runtime() -> (Context, Arc<ToolRuntime>) {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("system prompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let local = LocalSubprocessRuntime::new();
    let subprocess: Arc<dyn SubprocessRuntime> = local;
    ctx.register_service(subprocess);
    (ctx, tools)
}

fn config(mode: &str) -> StdioConfig {
    StdioConfig {
        server_name: "fixture".to_string(),
        command: fixture_path(),
        args: vec![mode.to_string()],
        env: Vec::new(),
        cwd: env!("CARGO_MANIFEST_DIR").to_string(),
        request_timeout: Duration::from_secs(3),
        close_timeout: Duration::from_secs(2),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_tracer_initializes_registers_executes_and_closes() {
    let (ctx, tools) = mount_runtime();

    let client = StdioClient::connect(&ctx, config("happy"))
        .await
        .expect("real stdio fixture should connect");

    let schema = tools
        .schemas(None)
        .into_iter()
        .find(|schema| schema.name == "mcp__fixture__inspect")
        .expect("MCP tool registered under server-qualified public name");
    assert_eq!(schema.description, "Inspect one value");
    assert_eq!(schema.parameters["type"], "object");
    assert!(
        tools.get("inspect", None).is_none(),
        "raw name must stay off the model registry"
    );

    let result = tools
        .execute(ToolExecutionInput {
            call_id: call_id("mcp-tracer-1"),
            root_call_id: None,
            name: "mcp__fixture__inspect".to_string(),
            arguments: json!({"value": "alpha"}),
            agent: None,
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;

    assert!(!result.is_error, "tool result: {:?}", result.error);
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "inspected alpha\n[image: image/png, content discarded]".to_string(),
        }]
    );
    assert_eq!(
        result.value,
        Some(json!({
            "content": [
                {"type": "text", "text": "inspected alpha"},
                {"type": "image", "mimeType": "image/png", "data": "AA=="}
            ],
            "structuredContent": {"echo": "alpha", "sequence": 1}
        }))
    );
    assert!(
        client
            .stderr_snapshot()
            .contains("fixture diagnostic: protocol-looking"),
        "stderr must be captured separately, not parsed as JSON-RPC stdout"
    );

    client.close().await.expect("bounded close");
    assert!(
        tools.get("mcp__fixture__inspect", None).is_none(),
        "close unregisters the generation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_failure_reconnects_once_after_old_generation_exits() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let root = std::env::temp_dir().join(format!(
            "dsh-mcp-reconnect-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("reconnect root");
        let state = root.join("generation.txt");
        let trace = root.join("trace.txt");
        let (ctx, tools) = mount_runtime();
        let mut reconnect_config = config("reconnect");
        reconnect_config.server_name = "restart".to_string();
        reconnect_config.env = vec![
            (
                "MCP_RECONNECT_STATE".to_string(),
                state.to_string_lossy().into_owned(),
            ),
            (
                "MCP_RECONNECT_TRACE".to_string(),
                trace.to_string_lossy().into_owned(),
            ),
        ];
        let client = StdioClient::connect_reconnecting(&ctx, reconnect_config)
            .await
            .expect("first generation connects");
        let schema = tools
            .schemas(None)
            .into_iter()
            .find(|schema| schema.description.contains("reconnecting generation"))
            .expect("stable reconnect tool");
        let name = schema.name.clone();
        let result = tools
            .execute(ToolExecutionInput {
                call_id: call_id("mcp-reconnect-call"),
                root_call_id: None,
                name: name.clone(),
                arguments: json!({"value": "gamma"}),
                agent: None,
                parent: None,
                signal: Arc::new(|| false),
            })
            .await;
        assert!(!result.is_error, "{:?}", result.error);
        assert_eq!(
            result.content,
            vec![ContentBlock::Text {
                text: "generation 2 inspected gamma".to_string()
            }]
        );
        assert_eq!(
            tools
                .schemas(None)
                .iter()
                .filter(|schema| schema.name == name)
                .count(),
            1
        );
        client.close().await.expect("bounded reconnect close");
        let trace_text = std::fs::read_to_string(&trace).expect("trace");
        let lines: Vec<_> = trace_text.lines().collect();
        assert_eq!(lines.len(), 4, "{trace_text}");
        assert!(lines[0].starts_with("start:1:"), "{trace_text}");
        assert!(lines[1].starts_with("end:1:"), "{trace_text}");
        assert!(lines[2].starts_with("start:2:"), "{trace_text}");
        assert!(lines[3].starts_with("end:2:"), "{trace_text}");
        let _ = std::fs::remove_dir_all(root);
    })
    .await
    .expect("reconnect tracer remains bounded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_timeout_terminates_old_generation_before_reconnect() {
    tokio::time::timeout(Duration::from_secs(6), async {
        let root = std::env::temp_dir().join(format!(
            "dsh-mcp-timeout-reconnect-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("timeout reconnect root");
        let state = root.join("generation.txt");
        let trace = root.join("trace.txt");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("reserve exclusive port")
            .local_addr()
            .expect("exclusive port")
            .port();
        let (ctx, tools) = mount_runtime();
        let mut reconnect_config = config("reconnect");
        reconnect_config.server_name = "timeout_restart".to_string();
        reconnect_config.request_timeout = Duration::from_millis(200);
        reconnect_config.close_timeout = Duration::from_millis(500);
        reconnect_config.env = vec![
            (
                "MCP_RECONNECT_STATE".to_string(),
                state.to_string_lossy().into_owned(),
            ),
            (
                "MCP_RECONNECT_TRACE".to_string(),
                trace.to_string_lossy().into_owned(),
            ),
            ("MCP_RECONNECT_PORT".to_string(), port.to_string()),
            ("MCP_RECONNECT_HANG_FIRST".to_string(), "1".to_string()),
        ];
        let client = StdioClient::connect_reconnecting(&ctx, reconnect_config)
            .await
            .expect("first timeout generation connects");
        let name = tools
            .schemas(None)
            .into_iter()
            .find(|schema| schema.description.contains("reconnecting generation"))
            .expect("timeout reconnect tool")
            .name;
        let result = tools
            .execute(ToolExecutionInput {
                call_id: call_id("mcp-timeout-reconnect-call"),
                root_call_id: None,
                name,
                arguments: json!({"value": "delta"}),
                agent: None,
                parent: None,
                signal: Arc::new(|| false),
            })
            .await;
        assert!(!result.is_error, "{:?}", result.error);
        assert_eq!(
            result.content,
            vec![ContentBlock::Text {
                text: "generation 2 inspected delta".to_string()
            }]
        );
        client.close().await.expect("close timeout reconnect route");
        let _ = std::fs::remove_dir_all(root);
    })
    .await
    .expect("timeout reconnect must terminate the old generation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_waits_for_an_inflight_reconnect_and_cannot_publish_after_close() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let root = std::env::temp_dir().join(format!(
            "dsh-mcp-close-reconnect-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("close reconnect root");
        let state = root.join("generation.txt");
        let trace = root.join("trace.txt");
        let release = root.join("release-second-generation");
        let (ctx, tools) = mount_runtime();
        let mut reconnect_config = config("reconnect");
        reconnect_config.server_name = "close_race".to_string();
        reconnect_config.env = vec![
            (
                "MCP_RECONNECT_STATE".to_string(),
                state.to_string_lossy().into_owned(),
            ),
            (
                "MCP_RECONNECT_TRACE".to_string(),
                trace.to_string_lossy().into_owned(),
            ),
            (
                "MCP_RECONNECT_SECOND_RELEASE".to_string(),
                release.to_string_lossy().into_owned(),
            ),
            ("MCP_RECONNECT_EXIT_AFTER_CALL".to_string(), "1".to_string()),
        ];
        let client = StdioClient::connect_reconnecting(&ctx, reconnect_config)
            .await
            .expect("first close-race generation connects");
        let name = tools
            .schemas(None)
            .into_iter()
            .find(|schema| schema.description.contains("reconnecting generation"))
            .expect("close-race tool")
            .name;
        let tools_for_call = tools.clone();
        let call = tokio::spawn(async move {
            tools_for_call
                .execute(ToolExecutionInput {
                    call_id: call_id("mcp-close-race-call"),
                    root_call_id: None,
                    name,
                    arguments: json!({"value": "epsilon"}),
                    agent: None,
                    parent: None,
                    signal: Arc::new(|| false),
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if std::fs::read_to_string(&trace).is_ok_and(|text| text.contains("start:2:")) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second generation reached its startup barrier");

        let closing_client = client.clone();
        let mut close = tokio::spawn(async move { closing_client.close().await });
        let close_completed_while_reconnect_was_blocked =
            tokio::time::timeout(Duration::from_millis(200), &mut close)
                .await
                .is_ok();
        std::fs::write(&release, b"release").expect("release second generation");
        if !close_completed_while_reconnect_was_blocked {
            close
                .await
                .expect("close task")
                .expect("close after reconnect");
        }
        let result = call.await.expect("tool call task");
        assert!(!result.is_error, "{:?}", result.error);
        assert!(
            !close_completed_while_reconnect_was_blocked,
            "close returned before the in-flight reconnect settled"
        );
        let _ = std::fs::remove_dir_all(root);
    })
    .await
    .expect("close/reconnect race remains bounded");
}
