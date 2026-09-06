use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use dsh_tool_computer_use_command::{
    AbortPredicate, AdapterRequest, ComputerUseAdapter, NativeBrowserAdapter, NativeBrowserConfig,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn active_signal() -> AbortPredicate {
    Arc::new(|| false)
}

async fn request(
    adapter: &NativeBrowserAdapter,
    arguments: Value,
) -> dsh_tool_computer_use_command::AdapterOutput {
    let request = AdapterRequest::from_arguments(&arguments)
        .expect("valid test action")
        .with_owner_id("owner-a");
    adapter
        .execute(request, active_signal())
        .await
        .expect("browser action succeeds")
}

async fn fixture_server() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut request = vec![0_u8; 8192];
                let read = stream.read(&mut request).await.unwrap_or(0);
                let first_line = String::from_utf8_lossy(&request[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let route = first_line.split_whitespace().nth(1).unwrap_or("/");
                let initial_title = if route.starts_with("/second") {
                    "second"
                } else {
                    "ready"
                };
                let body = format!(
                    r#"<!doctype html><html><head><meta charset="utf-8"><title>{initial_title}</title>
                    <style>html,body{{margin:0}}body{{height:3000px}}button{{position:absolute;left:20px;top:20px;width:180px;height:50px}}input{{position:absolute;left:20px;top:100px;width:300px;height:40px}}</style></head>
                    <body><button id="action" onclick="document.title='clicked'">Click target</button>
                    <input id="entry" oninput="document.title='typed:'+this.value">
                    <div style="position:absolute;top:2600px">bottom</div></body></html>"#
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (format!("http://{address}"), task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a locally installed Edge/Chrome and DSH_CDP_TEST_ROOT"]
async fn edge_controls_isolated_pages_and_returns_real_screenshots() {
    let test_root = PathBuf::from(
        std::env::var_os("DSH_CDP_TEST_ROOT")
            .expect("DSH_CDP_TEST_ROOT must name the isolated test directory"),
    )
    .join(format!("edge-e2e-{}", uuid::Uuid::new_v4()));
    let executable = std::env::var_os("DSH_BROWSER_EXECUTABLE").map(PathBuf::from);
    let adapter = NativeBrowserAdapter::new(NativeBrowserConfig {
        executable,
        data_root: test_root.clone(),
        headless: true,
        max_sessions: 2,
        launch_timeout: Duration::from_secs(20),
        action_timeout: Duration::from_secs(10),
        viewport_width: 1280,
        viewport_height: 720,
    })
    .expect("construct native browser adapter");
    let (fixture, server) = fixture_server().await;

    let navigated = request(
        &adapter,
        json!({"action":"navigate","sessionId":"first","url":format!("{fixture}/first"),"waitMs":300}),
    )
    .await;
    assert_eq!(navigated.value["state"]["title"], "ready");
    assert_eq!(navigated.value["state"]["readyState"], "complete");
    assert!(adapter.has_owner_activity("owner-a"));
    assert!(!adapter.has_owner_activity("owner-b"));
    let png = navigated.screenshot.expect("navigate screenshot").data;
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(
        png.len() > 1_000,
        "screenshot should contain rendered pixels"
    );

    let clicked = request(
        &adapter,
        json!({"action":"click","sessionId":"first","x":60,"y":45,"waitMs":100}),
    )
    .await;
    assert_eq!(clicked.value["state"]["title"], "clicked");

    let typed = request(
        &adapter,
        json!({"action":"type","sessionId":"first","x":60,"y":120,"text":"hello","waitMs":100}),
    )
    .await;
    assert_eq!(typed.value["state"]["activeElement"]["id"], "entry");
    assert_eq!(typed.value["state"]["activeElement"]["value"], "hello");

    let scrolled = request(
        &adapter,
        json!({"action":"scroll","sessionId":"first","x":100,"y":300,"deltaY":700,"waitMs":150}),
    )
    .await;
    assert!(
        scrolled.value["state"]["scrollY"].as_f64().unwrap_or(0.0) > 0.0,
        "wheel action must move the page"
    );

    let second = request(
        &adapter,
        json!({"action":"navigate","sessionId":"second","url":format!("{fixture}/second"),"waitMs":300}),
    )
    .await;
    assert_eq!(second.value["state"]["title"], "second");
    let first_state = request(
        &adapter,
        json!({"action":"status","sessionId":"first","includeScreenshot":false}),
    )
    .await;
    assert_eq!(
        first_state.value["state"]["activeElement"]["value"],
        "hello"
    );
    assert_ne!(
        first_state.value["state"]["url"],
        second.value["state"]["url"]
    );

    let sessions = request(&adapter, json!({"action":"list_sessions"})).await;
    assert_eq!(sessions.value["sessions"], json!(["first", "second"]));
    let other_owner = adapter
        .execute(
            AdapterRequest::from_arguments(&json!({"action":"list_sessions"}))
                .unwrap()
                .with_owner_id("owner-b"),
            active_signal(),
        )
        .await
        .unwrap();
    assert_eq!(other_owner.value["sessions"], json!([]));
    let isolated = adapter
        .execute(
            AdapterRequest::from_arguments(&json!({"action":"status","sessionId":"first"}))
                .unwrap()
                .with_owner_id("owner-b"),
            active_signal(),
        )
        .await
        .unwrap_err();
    assert_eq!(isolated.code, "COMPUTER_USE_SESSION_NOT_FOUND");

    let cancelled = adapter
        .execute(
            AdapterRequest::from_arguments(&json!({"action":"capture","sessionId":"first"}))
                .unwrap()
                .with_owner_id("owner-a"),
            Arc::new(|| true),
        )
        .await
        .unwrap_err();
    assert_eq!(cancelled.code, "COMPUTER_USE_ABORTED");

    request(&adapter, json!({"action":"close","sessionId":"second"})).await;
    let owner_b = adapter
        .execute(
            AdapterRequest::from_arguments(&json!({
                "action":"navigate",
                "sessionId":"first",
                "url":format!("{fixture}/second"),
                "waitMs":300
            }))
            .unwrap()
            .with_owner_id("owner-b"),
            active_signal(),
        )
        .await
        .unwrap();
    assert_eq!(owner_b.value["state"]["title"], "second");
    assert!(adapter.has_owner_activity("owner-a"));
    assert!(adapter.has_owner_activity("owner-b"));

    adapter.close_owner("owner-a").await.unwrap();
    assert!(!adapter.has_owner_activity("owner-a"));
    assert!(adapter.has_owner_activity("owner-b"));
    let retired_owner = adapter
        .execute(
            AdapterRequest::from_arguments(&json!({"action":"status","sessionId":"first"}))
                .unwrap()
                .with_owner_id("owner-a"),
            active_signal(),
        )
        .await
        .unwrap_err();
    assert_eq!(retired_owner.code, "COMPUTER_USE_SESSION_NOT_FOUND");
    let surviving_owner = adapter
        .execute(
            AdapterRequest::from_arguments(&json!({"action":"status","sessionId":"first"}))
                .unwrap()
                .with_owner_id("owner-b"),
            active_signal(),
        )
        .await
        .unwrap();
    assert_eq!(surviving_owner.value["state"]["title"], "second");

    adapter.shutdown().await.unwrap();
    assert!(!adapter.has_owner_activity("owner-b"));
    let shutdown_owner = adapter
        .execute(
            AdapterRequest::from_arguments(&json!({"action":"status","sessionId":"first"}))
                .unwrap()
                .with_owner_id("owner-b"),
            active_signal(),
        )
        .await
        .unwrap_err();
    assert_eq!(shutdown_owner.code, "COMPUTER_USE_SESSION_NOT_FOUND");
    server.abort();
    let _ = tokio::fs::remove_dir_all(&test_root).await;
}
