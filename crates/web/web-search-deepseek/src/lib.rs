//! DeepSeek native web-search provider over the Anthropic-compatible Messages API.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dsh_web::{
    Cancelled, WebError, WebSearchProvider, WebSearchRequest, WebSearchResult, WebSearchSource,
};
use futures::future::BoxFuture;

pub type ApiKeyResolver =
    Arc<dyn Fn() -> BoxFuture<'static, Result<Option<String>, String>> + Send + Sync>;
pub type RequestRecorder = Arc<dyn Fn(&serde_json::Value) -> Result<(), String> + Send + Sync>;
pub type OptionsResolver = Arc<dyn Fn() -> Result<Options, String> + Send + Sync>;

#[derive(Clone)]
pub struct Options {
    pub api_key: Option<String>,
    pub resolve_api_key: Option<ApiKeyResolver>,
    pub api_key_env: String,
    pub base_url: String,
    pub model: String,
    pub api_version: String,
    pub max_tokens: u64,
    pub max_uses: u64,
    pub record_request: Option<RequestRecorder>,
}

pub struct DeepSeekSearchProvider {
    resolve_options: OptionsResolver,
    client: reqwest::Client,
}

impl DeepSeekSearchProvider {
    pub fn new(options: Options) -> Self {
        Self::with_options(Arc::new(move || Ok(options.clone())))
    }

    pub fn with_options(resolve_options: OptionsResolver) -> Self {
        Self {
            resolve_options,
            client: dsh_http_proxy::builder()
                .expect("valid outbound proxy policy")
                .timeout(Duration::from_secs(120))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("DeepSeek web search client"),
        }
    }

    async fn api_key(options: &Options) -> Result<String, WebError> {
        if let Some(value) = &options.api_key
            && !value.trim().is_empty()
        {
            return Ok(value.clone());
        }
        if let Some(resolve) = &options.resolve_api_key
            && let Some(value) = resolve()
                .await
                .map_err(|error| WebError::new("WEB_PROVIDER_CREDENTIAL_MISSING", error))?
            && !value.trim().is_empty()
        {
            return Ok(value);
        }
        Err(WebError::new(
            "WEB_PROVIDER_CREDENTIAL_MISSING",
            format!("missing credential {}", options.api_key_env),
        ))
    }

    fn project(value: serde_json::Value) -> Result<WebSearchResult, WebError> {
        let blocks = value
            .get("content")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                WebError::new(
                    "WEB_PROVIDER_ERROR",
                    "DeepSeek web search response omitted content",
                )
            })?;
        let mut sources: BTreeMap<String, WebSearchSource> = BTreeMap::new();
        let mut citations: BTreeMap<String, String> = BTreeMap::new();
        let mut text = Vec::new();
        let mut search_blocks = 0;
        for block in blocks {
            match block.get("type").and_then(serde_json::Value::as_str) {
                Some("text") => {
                    if let Some(body) = block
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .filter(|text| !text.is_empty())
                    {
                        text.push(body.to_string());
                    }
                    if let Some(items) =
                        block.get("citations").and_then(serde_json::Value::as_array)
                    {
                        for item in items {
                            if let (Some(url), Some(snippet)) = (
                                item.get("url").and_then(serde_json::Value::as_str),
                                item.get("cited_text").and_then(serde_json::Value::as_str),
                            ) {
                                citations
                                    .entry(url.to_string())
                                    .or_insert_with(|| snippet.to_string());
                            }
                        }
                    }
                }
                Some("web_search_tool_result") => {
                    search_blocks += 1;
                    if let Some(code) = block
                        .pointer("/content/error_code")
                        .and_then(serde_json::Value::as_str)
                    {
                        return Err(WebError::new(
                            "WEB_SEARCH_PROVIDER_ERROR",
                            format!(
                                "Search service reported {code}; this is not an empty search result"
                            ),
                        ));
                    }
                    if !block
                        .get("content")
                        .is_some_and(serde_json::Value::is_array)
                    {
                        return Err(WebError::new(
                            "WEB_PROVIDER_ERROR",
                            "Search service returned malformed web_search_tool_result content",
                        ));
                    }
                    if let Some(items) = block.get("content").and_then(serde_json::Value::as_array)
                    {
                        for item in items {
                            if item.get("type").and_then(serde_json::Value::as_str)
                                != Some("web_search_result")
                            {
                                continue;
                            }
                            let Some(url) = item
                                .get("url")
                                .and_then(serde_json::Value::as_str)
                                .filter(|url| !url.is_empty())
                            else {
                                continue;
                            };
                            sources
                                .entry(url.to_string())
                                .or_insert_with(|| WebSearchSource {
                                    url: url.to_string(),
                                    title: item
                                        .get("title")
                                        .and_then(serde_json::Value::as_str)
                                        .map(str::to_string),
                                    snippet: None,
                                    published_at: item
                                        .get("page_age")
                                        .and_then(serde_json::Value::as_str)
                                        .map(str::to_string),
                                });
                        }
                    }
                }
                _ => {}
            }
        }
        for (url, snippet) in citations {
            if let Some(source) = sources.get_mut(&url) {
                source.snippet = Some(snippet);
            }
        }
        if search_blocks == 0 {
            let block_types = blocks
                .iter()
                .filter_map(|block| block.get("type").and_then(serde_json::Value::as_str))
                .take(8)
                .collect::<Vec<_>>();
            return Err(WebError::new(
                "WEB_SEARCH_NOT_TRIGGERED",
                format!(
                    "Search response contained no native web_search_tool_result blocks (content types: {}); search execution was not confirmed, so do not report this as no matching pages",
                    block_types.join(", ")
                ),
            ));
        }
        let incomplete = matches!(
            value.get("stop_reason").and_then(serde_json::Value::as_str),
            Some("pause_turn" | "max_tokens")
        );
        if sources.is_empty() && incomplete {
            return Err(WebError::new(
                "WEB_SEARCH_INCOMPLETE",
                "Search response ended before usable results were available",
            ));
        }
        Ok(WebSearchResult {
            content: (!sources.is_empty() && !text.is_empty()).then(|| text.join("\n\n")),
            sources: sources.into_values().collect(),
            truncated: incomplete,
        })
    }
}

#[async_trait]
impl WebSearchProvider for DeepSeekSearchProvider {
    fn id(&self) -> &str {
        "deepseek-official"
    }

    fn available(&self) -> bool {
        (self.resolve_options)().is_ok_and(|options| {
            options
                .api_key
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
                || options.resolve_api_key.is_some()
        })
    }

    async fn search(
        &self,
        request: WebSearchRequest,
        cancelled: Cancelled,
    ) -> Result<WebSearchResult, WebError> {
        if cancelled() {
            return Err(WebError::new("WEB_ABORTED", "web search aborted"));
        }
        let options = (self.resolve_options)()
            .map_err(|error| WebError::new("WEB_PROVIDER_CONFIG", error))?;
        let operation = async {
            let api_key = Self::api_key(&options).await?;
            let body = serde_json::json!({
                "model": options.model,
                "max_tokens": options.max_tokens,
                "messages": [{"role":"user","content":[{"type":"text","text":format!("Perform a web search for the query: {}", request.query)}]}],
                "tools": [{"type":"web_search_20250305","name":"web_search","max_uses":options.max_uses}]
            });
            let url = format!("{}/messages", options.base_url.trim_end_matches('/'));
            if let Some(record) = &options.record_request {
                record(&serde_json::json!({"endpoint":url,"apiVersion":options.api_version,"body":body}))
                .map_err(|error| WebError::new("WEB_REQUEST_RECORD_FAILED", error))?;
            }
            let response = self
                .client
                .post(&url)
                .header("x-api-key", &api_key)
                .bearer_auth(&api_key)
                .header("anthropic-version", &options.api_version)
                .json(&body)
                .send()
                .await
                .map_err(|error| {
                    search_endpoint_error(&url, format!("DeepSeek search request failed: {error}"))
                })?;
            let status = response.status();
            let value = response
                .json::<serde_json::Value>()
                .await
                .map_err(|error| {
                    search_endpoint_error(
                        &url,
                        format!(
                            "DeepSeek returned an unprocessable response body (HTTP {}): {error}",
                            status.as_u16()
                        ),
                    )
                })?;
            if !status.is_success() {
                let message = value
                    .pointer("/error/message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("DeepSeek web search request failed");
                return Err(search_endpoint_error(
                    &url,
                    format!("DeepSeek API error (HTTP {}): {message}", status.as_u16()),
                ));
            }
            Self::project(value).map_err(|error| {
                let guidance = search_endpoint_error(&url, error.to_string());
                WebError::new(error.code(), guidance.to_string())
            })
        };
        tokio::pin!(operation);
        loop {
            tokio::select! {
                result = &mut operation => return result,
                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    if cancelled() { return Err(WebError::new("WEB_ABORTED", "web search aborted")); }
                }
            }
        }
    }
}

fn search_endpoint_error(endpoint: &str, message: impl AsRef<str>) -> WebError {
    WebError::new(
        "WEB_PROVIDER_ERROR",
        format!(
            "{}\n\nThe web search request used endpoint {:?}. Search endpoint configuration is separate from chat. If that endpoint is not intended, guide the user to Settings > Plugins > Plugin configuration > Web search, where they can change and save Endpoint. If that settings page is unavailable, the user can set DEEPSEEK_SEARCH_BASE_URL or configure web-search-deepseek.baseURL to a trusted Anthropic-compatible Messages API base. Only the user should choose or change the endpoint.",
            message.as_ref(),
            endpoint
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(base_url: String, key: &str, model: &str) -> Options {
        Options {
            api_key: Some(key.into()),
            resolve_api_key: None,
            api_key_env: "FIXTURE_KEY".into(),
            base_url,
            model: model.into(),
            api_version: "2023-06-01".into(),
            max_tokens: 64,
            max_uses: 1,
            record_request: None,
        }
    }

    async fn fixture() -> (
        String,
        tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let app=axum::Router::new().route("/messages",axum::routing::post(move |headers:axum::http::HeaderMap,axum::Json(body):axum::Json<serde_json::Value>| {
            let sender=sender.clone();
            async move {
                sender.send(serde_json::json!({"key":headers["x-api-key"].to_str().unwrap(),"body":body})).unwrap();
                axum::Json(serde_json::json!({"content":[{"type":"web_search_tool_result","content":[]}]}))
            }
        }));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), receiver, task)
    }

    #[tokio::test]
    async fn each_search_snapshots_endpoint_model_and_key_before_awaiting_credentials() {
        use std::sync::Mutex;
        let (first_url, mut first_requests, first_server) = fixture().await;
        let (next_url, mut next_requests, next_server) = fixture().await;
        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let recorder: RequestRecorder = {
            let recorded = recorded.clone();
            Arc::new(move |request| {
                recorded.lock().unwrap().push(request.clone());
                Ok(())
            })
        };
        let mut initial = options(first_url.clone(), "unused", "first-model");
        initial.api_key = None;
        initial.record_request = Some(recorder.clone());
        initial.resolve_api_key = Some({
            let started = started.clone();
            let release = release.clone();
            Arc::new(move || {
                let started = started.clone();
                let release = release.clone();
                Box::pin(async move {
                    started.add_permits(1);
                    release.acquire().await.unwrap().forget();
                    Ok(Some("first-fixture-secret".into()))
                })
            })
        });
        let current = Arc::new(Mutex::new(initial));
        let provider = Arc::new(DeepSeekSearchProvider::with_options({
            let current = current.clone();
            Arc::new(move || Ok(current.lock().unwrap().clone()))
        }));
        let first = {
            let provider = provider.clone();
            tokio::spawn(async move {
                provider
                    .search(
                        WebSearchRequest {
                            query: "fixture".into(),
                            max_results: None,
                        },
                        Arc::new(|| false),
                    )
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), started.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        let mut next = options(next_url.clone(), "next-fixture-secret", "next-model");
        next.record_request = Some(recorder);
        *current.lock().unwrap() = next;
        release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let packet = tokio::time::timeout(Duration::from_secs(2), first_requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(packet["key"], "first-fixture-secret");
        assert_eq!(packet["body"]["model"], "first-model");
        assert!(next_requests.try_recv().is_err());
        tokio::time::timeout(
            Duration::from_secs(2),
            provider.search(
                WebSearchRequest {
                    query: "fixture".into(),
                    max_results: None,
                },
                Arc::new(|| false),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let packet = tokio::time::timeout(Duration::from_secs(2), next_requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(packet["key"], "next-fixture-secret");
        assert_eq!(packet["body"]["model"], "next-model");
        let records = recorded.lock().unwrap();
        assert_eq!(records[0]["endpoint"], format!("{first_url}/messages"));
        assert_eq!(records[1]["endpoint"], format!("{next_url}/messages"));
        assert!(
            !serde_json::to_string(&*records)
                .unwrap()
                .contains("fixture-secret")
        );
        first_server.abort();
        next_server.abort();
    }

    #[tokio::test]
    async fn cancellation_can_interrupt_credential_resolution() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut config = options("http://127.0.0.1:1".into(), "unused", "fixture");
        config.api_key = None;
        config.resolve_api_key = Some(Arc::new(|| Box::pin(futures::future::pending())));
        let provider = DeepSeekSearchProvider::new(config);
        let stopped = Arc::new(AtomicBool::new(false));
        let cancelled: Cancelled = {
            let stopped = stopped.clone();
            Arc::new(move || stopped.load(Ordering::SeqCst))
        };
        let task = tokio::spawn(async move {
            provider
                .search(
                    WebSearchRequest {
                        query: "fixture".into(),
                        max_results: None,
                    },
                    cancelled,
                )
                .await
        });
        tokio::task::yield_now().await;
        stopped.store(true, Ordering::SeqCst);
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code(), "WEB_ABORTED");
    }

    #[tokio::test]
    async fn failed_request_recording_prevents_dispatch() {
        let (url, mut requests, server) = fixture().await;
        let mut config = options(url, "fixture-key", "fixture");
        config.record_request = Some(Arc::new(|_| Err("fixture log unavailable".into())));
        let error = DeepSeekSearchProvider::new(config)
            .search(
                WebSearchRequest {
                    query: "fixture".into(),
                    max_results: None,
                },
                Arc::new(|| false),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), "WEB_REQUEST_RECORD_FAILED");
        assert!(requests.try_recv().is_err());
        server.abort();
    }

    #[test]
    fn native_zero_results_are_not_a_provider_failure() {
        let result = DeepSeekSearchProvider::project(serde_json::json!({"stop_reason":"end_turn","content":[{"type":"web_search_tool_result","content":[]},{"type":"text","text":"No matching pages"}]})).unwrap();
        assert!(result.sources.is_empty());
        assert!(result.content.is_none());
    }

    #[test]
    fn missing_execution_and_server_errors_remain_distinct() {
        let missing = DeepSeekSearchProvider::project(
            serde_json::json!({"content":[{"type":"text","text":"I searched the web"}]}),
        )
        .unwrap_err();
        assert_eq!(missing.code(), "WEB_SEARCH_NOT_TRIGGERED");
        let failed = DeepSeekSearchProvider::project(serde_json::json!({"content":[{"type":"web_search_tool_result","content":{"type":"web_search_tool_result_error","error_code":"max_uses_exceeded"}}]})).unwrap_err();
        assert_eq!(failed.code(), "WEB_SEARCH_PROVIDER_ERROR");
        assert!(failed.to_string().contains("max_uses_exceeded"));
        let paused = DeepSeekSearchProvider::project(serde_json::json!({"stop_reason":"pause_turn","content":[{"type":"web_search_tool_result","content":[]}]})).unwrap_err();
        assert_eq!(paused.code(), "WEB_SEARCH_INCOMPLETE");
    }

    #[test]
    fn source_urls_keep_their_citation_snippets() {
        let result=DeepSeekSearchProvider::project(serde_json::json!({"content":[{"type":"web_search_tool_result","content":[{"type":"web_search_result","url":"https://example.test/","title":"Example"}]},{"type":"text","text":"summary","citations":[{"url":"https://example.test/","cited_text":"verified excerpt"}]}]})).unwrap();
        assert_eq!(result.sources.len(), 1);
        assert_eq!(
            result.sources[0].snippet.as_deref(),
            Some("verified excerpt")
        );
    }

    #[test]
    fn endpoint_error_names_the_actual_endpoint_and_recovery_surface() {
        let endpoint = "https://search.example.invalid/anthropic/v1/messages";
        let error =
            search_endpoint_error(endpoint, "DeepSeek search request failed: connection reset");

        assert_eq!(error.code(), "WEB_PROVIDER_ERROR");
        assert!(error.to_string().contains(endpoint));
        assert!(
            error
                .to_string()
                .contains("Settings > Plugins > Plugin configuration > Web search")
        );
        assert!(
            error
                .to_string()
                .contains("Only the user should choose or change the endpoint")
        );
    }
}
