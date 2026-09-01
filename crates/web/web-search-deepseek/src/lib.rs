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
pub type RequestRecorder = Arc<dyn Fn(&serde_json::Value) + Send + Sync>;

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
    options: Options,
    client: reqwest::Client,
}

impl DeepSeekSearchProvider {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("DeepSeek web search client"),
        }
    }

    async fn api_key(&self) -> Result<String, WebError> {
        if let Some(value) = &self.options.api_key
            && !value.trim().is_empty()
        {
            return Ok(value.clone());
        }
        if let Some(resolve) = &self.options.resolve_api_key
            && let Some(value) = resolve()
                .await
                .map_err(|error| WebError::new("WEB_PROVIDER_CREDENTIAL_MISSING", error))?
            && !value.trim().is_empty()
        {
            return Ok(value);
        }
        Err(WebError::new(
            "WEB_PROVIDER_CREDENTIAL_MISSING",
            format!("missing credential {}", self.options.api_key_env),
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
        if sources.is_empty() {
            return Err(WebError::new(
                "WEB_PROVIDER_ERROR",
                "DeepSeek web search returned no sources",
            ));
        }
        Ok(WebSearchResult {
            content: (!text.is_empty()).then(|| text.join("\n\n")),
            sources: sources.into_values().collect(),
            truncated: false,
        })
    }
}

#[async_trait]
impl WebSearchProvider for DeepSeekSearchProvider {
    fn id(&self) -> &str {
        "deepseek-official"
    }

    fn available(&self) -> bool {
        self.options
            .api_key
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
            || self.options.resolve_api_key.is_some()
    }

    async fn search(
        &self,
        request: WebSearchRequest,
        cancelled: Cancelled,
    ) -> Result<WebSearchResult, WebError> {
        if cancelled() {
            return Err(WebError::new("WEB_ABORTED", "web search aborted"));
        }
        let api_key = self.api_key().await?;
        let body = serde_json::json!({
            "model": self.options.model,
            "max_tokens": self.options.max_tokens,
            "messages": [{"role":"user","content":[{"type":"text","text":format!("Perform a web search for the query: {}", request.query)}]}],
            "tools": [{"type":"web_search_20250305","name":"web_search","max_uses":self.options.max_uses}]
        });
        if let Some(record) = &self.options.record_request {
            record(&body);
        }
        let url = format!("{}/messages", self.options.base_url.trim_end_matches('/'));
        let request_future = self
            .client
            .post(&url)
            .header("x-api-key", &api_key)
            .bearer_auth(&api_key)
            .header("anthropic-version", &self.options.api_version)
            .json(&body)
            .send();
        tokio::pin!(request_future);
        let response = loop {
            tokio::select! {
                response = &mut request_future => break response.map_err(|error| {
                    search_endpoint_error(&url, format!("DeepSeek search request failed: {error}"))
                })?,
                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    if cancelled() { return Err(WebError::new("WEB_ABORTED", "web search aborted")); }
                }
            }
        };
        let status = response.status();
        let value = response
            .json::<serde_json::Value>()
            .await
            .map_err(|error| {
                search_endpoint_error(
                    &url,
                    format!("DeepSeek returned an unprocessable response body: {error}"),
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
        Self::project(value).map_err(|error| search_endpoint_error(&url, error.to_string()))
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
