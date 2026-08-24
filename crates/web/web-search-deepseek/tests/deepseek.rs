use dsh_web::{WebSearchProvider, WebSearchRequest};
use dsh_web_search_deepseek::{DeepSeekSearchProvider, Options};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TEST_KEY: &str = "test-key-placeholder";

async fn fixture(status: u16, response: &'static str, hold: bool) -> (String, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(String::new()));
    let seen = captured.clone();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = vec![0; 16384];
        let n = stream.read(&mut bytes).await.unwrap();
        *seen.lock().unwrap() = String::from_utf8_lossy(&bytes[..n]).into_owned();
        if hold {
            std::future::pending::<()>().await;
        }
        let wire = format!(
            "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
            response.len()
        );
        stream.write_all(wire.as_bytes()).await.unwrap();
    });
    (format!("http://{addr}/anthropic/v1"), captured)
}

fn options(base_url: String) -> Options {
    Options {
        api_key: Some(TEST_KEY.into()),
        resolve_api_key: None,
        api_key_env: "DEEPSEEK_API_KEY".into(),
        base_url,
        model: "deepseek-chat".into(),
        api_version: "2023-06-01".into(),
        max_tokens: 4096,
        max_uses: 5,
        record_request: None,
    }
}

#[tokio::test]
async fn posts_exact_native_search_envelope_and_projects_sources() {
    let body = r#"{"content":[{"type":"text","citations":[{"url":"https://a.test","cited_text":"excerpt"}]},{"type":"web_search_tool_result","content":[{"type":"web_search_result","url":"https://a.test","title":"A","page_age":"2026-02-02"},{"type":"web_search_result","url":"https://a.test","title":"duplicate"}]}]}"#;
    let (base, captured) = fixture(200, body, false).await;
    let provider = DeepSeekSearchProvider::new(options(base));
    let result = provider
        .search(
            WebSearchRequest {
                query: "hello".into(),
                max_results: None,
            },
            Arc::new(|| false),
        )
        .await
        .unwrap();
    let request = captured.lock().unwrap().clone().to_ascii_lowercase();
    assert!(request.starts_with("post /anthropic/v1/messages http/1.1"));
    assert!(request.contains("x-api-key: test-key-placeholder"));
    assert!(request.contains("authorization: bearer test-key-placeholder"));
    assert!(request.contains("anthropic-version: 2023-06-01"));
    assert!(request.contains(r#""text":"perform a web search for the query: hello""#));
    assert!(request.contains(r#""type":"web_search_20250305""#));
    assert_eq!(result.sources.len(), 1);
    assert_eq!(result.sources[0].snippet.as_deref(), Some("excerpt"));
    assert_eq!(
        result.sources[0].published_at.as_deref(),
        Some("2026-02-02")
    );
}

#[tokio::test]
async fn maps_http_shape_and_cancellation_errors() {
    let (base, _) = fixture(429, r#"{"error":{"message":"rate limited"}}"#, false).await;
    let error = DeepSeekSearchProvider::new(options(base))
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            Arc::new(|| false),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), "WEB_PROVIDER_ERROR");
    assert_eq!(error.to_string(), "rate limited");

    let (base, _) = fixture(200, "{}", true).await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let search = tokio::spawn(async move {
        DeepSeekSearchProvider::new(options(base))
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None,
                },
                Arc::new(move || flag.load(Ordering::SeqCst)),
            )
            .await
    });
    tokio::task::yield_now().await;
    cancelled.store(true, Ordering::SeqCst);
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), search)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code(), "WEB_ABORTED");
}

#[tokio::test]
async fn strict_response_and_missing_credential_are_typed_errors() {
    let provider = DeepSeekSearchProvider::new(Options {
        api_key: None,
        resolve_api_key: None,
        ..options("https://example.test/v1".into())
    });
    assert_eq!(
        provider
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None
                },
                Arc::new(|| false)
            )
            .await
            .unwrap_err()
            .code(),
        "WEB_PROVIDER_CREDENTIAL_MISSING"
    );
    let (base, _) = fixture(
        200,
        r#"{"content":[{"type":"text","text":"prose only"}]}"#,
        false,
    )
    .await;
    assert_eq!(
        DeepSeekSearchProvider::new(options(base))
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None
                },
                Arc::new(|| false)
            )
            .await
            .unwrap_err()
            .code(),
        "WEB_PROVIDER_ERROR"
    );
}
