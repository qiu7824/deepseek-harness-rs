//! Hosted Responses search uses the selected connection's existing credentials.
use super::{provider_auth::AccountAuth, task_models::TaskModels};
use dsh_web::{
    Cancelled, WebError, WebSearchProvider, WebSearchRequest, WebSearchResult, WebSearchSource,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

pub(super) struct RoutedSearch {
    pub ctx: cordis::Context,
    pub settings: Arc<dsh_settings::SettingsProvider>,
    pub agents: Arc<dsh_agent::AgentRegistry>,
    pub auth: Arc<AccountAuth>,
    pub fallback: Arc<dyn WebSearchProvider>,
}
fn error(message: impl Into<String>) -> WebError {
    WebError::new("WEB_HOSTED_SEARCH_ERROR", message)
}

#[async_trait::async_trait]
impl WebSearchProvider for RoutedSearch {
    fn id(&self) -> &str {
        "session-search"
    }
    fn available(&self) -> bool {
        true
    }
    async fn search(
        &self,
        request: WebSearchRequest,
        cancelled: Cancelled,
    ) -> Result<WebSearchResult, WebError> {
        let settings = self
            .settings
            .get(&dsh_settings::settings_namespace("web-search-deepseek").unwrap())
            .and_then(|v| v.to_json())
            .unwrap_or(json!({}));
        if settings["mode"] != "hosted" {
            return self.fallback.search(request, cancelled).await;
        }
        let operation = self.hosted(request, settings["access"] != "cached", &cancelled);
        tokio::pin!(operation);
        loop {
            tokio::select! {value=&mut operation=>return value,_=tokio::time::sleep(Duration::from_millis(20))=>if cancelled(){return Err(WebError::new("WEB_ABORTED","search cancelled"));}}
        }
    }
}
impl RoutedSearch {
    async fn hosted(
        &self,
        request: WebSearchRequest,
        live: bool,
        cancelled: &Cancelled,
    ) -> Result<WebSearchResult, WebError> {
        if cancelled() {
            return Err(WebError::new("WEB_ABORTED", "search cancelled"));
        }
        let agent = self
            .agents
            .current_initiator()
            .map_err(error)?
            .ok_or_else(|| error("Hosted search requires an active session"))?;
        let tasks = self
            .ctx
            .get_typed::<Arc<TaskModels>>("taskModels", false)
            .ok_or_else(|| error("Model routing is unavailable"))?;
        let route = tasks
            .route_for_agent("search", &agent)
            .await
            .map_err(error)?;
        let provider = route["provider"]
            .as_str()
            .ok_or_else(|| error("Select a model connection"))?;
        let model = route["model"]
            .as_str()
            .ok_or_else(|| error("Select a model"))?;
        let (profile, key) = self.auth.image_connection(provider).await.map_err(error)?;
        if key.is_none() && profile["keyless"] != true {
            return Err(error(
                "The selected search connection has no usable credentials",
            ));
        }
        if profile["authProvider"] != "openai-codex"
            && !matches!(
                profile["api"].as_str(),
                Some("openai-responses" | "openai-completions")
            )
        {
            return Err(error(
                "This connection does not support hosted Responses search; select a supported connection or DeepSeek search in plugin settings",
            ));
        }
        let endpoint = endpoint(
            profile["baseURL"]
                .as_str()
                .ok_or_else(|| error("Connection endpoint is missing"))?,
        )?;
        let mut headers = super::validated_discovery_headers(
            provider,
            &serde_json::from_value(profile.get("headers").cloned().unwrap_or(json!({})))
                .map_err(|_| error("Invalid connection headers"))?,
        )
        .map_err(error)?;
        for name in ["authorization", "host", "content-length", "content-type"] {
            headers.remove(name);
        }
        if let Some(key) = &key {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {key}")
                    .parse()
                    .map_err(|_| error("Invalid credentials"))?,
            );
        }
        let resolved = tasks
            .llm
            .resolve_call_config(
                &dsh_llm::LlmCallConfig {
                    provider: provider.into(),
                    model: model.into(),
                    reasoning_effort: route["reasoningEffort"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(dsh_llm::reasoning_effort_id),
                    ..Default::default()
                },
                Some(cancelled),
            )
            .await
            .map_err(|e| error(e.to_string()))?;
        let mut body = search_body(model, &request.query, live);
        if let Some(effort) = resolved.reasoning_effort {
            body["reasoning"] = json!({"effort":effort.as_str()});
        }
        agent
            .session()
            .append(
                "web/hosted-search-request",
                json!({"provider":provider,"endpoint":endpoint,"body":body}),
                None,
            )
            .map_err(error)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| error(e.to_string()))?;
        let mut response = client
            .post(&endpoint)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| error(format!("Hosted search transport failed: {e}")))?;
        let status = response.status();
        let stream = status.is_success()
            && response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("text/event-stream"));
        let mut bytes = Vec::new();
        let mut total = 0usize;
        while let Some(chunk) = response.chunk().await.map_err(|e| error(e.to_string()))? {
            total = total.saturating_add(chunk.len());
            if total > 8 * 1024 * 1024 {
                return Err(error("Hosted search response exceeded 8 MiB"));
            }
            bytes.extend_from_slice(&chunk);
            if stream {
                while let Some((end, width)) = sse_boundary(&bytes) {
                    let packet = std::str::from_utf8(&bytes[..end])
                        .map_err(|_| error("Search returned non-UTF8 data"))?;
                    if let Some(result) = parse_packet(packet) {
                        return result;
                    }
                    bytes.drain(..end + width);
                }
            }
        }
        if !status.is_success() {
            let mut detail = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(key) = key.filter(|k| !k.is_empty()) {
                detail = detail.replace(&key, "[redacted]");
            }
            return Err(error(format!(
                "Hosted search HTTP {status}: {}",
                detail.chars().take(2048).collect::<String>()
            )));
        }
        parse_response(&bytes)
    }
}
fn endpoint(base: &str) -> Result<String, WebError> {
    let url = reqwest::Url::parse(base).map_err(|_| error("Invalid hosted search endpoint"))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(local && url.scheme() == "http"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(error("Hosted search requires an HTTPS connection endpoint"));
    }
    let base = base.trim_end_matches('/');
    Ok(if base.ends_with("/responses") {
        base.into()
    } else {
        format!("{base}/responses")
    })
}
fn search_body(model: &str, query: &str, live: bool) -> Value {
    json!({"model":model,"instructions":"Search the web for the supplied query. Cite the original source URLs. Report when there are no matching sources.","input":[{"role":"user","content":[{"type":"input_text","text":query}]}],"tools":[{"type":"web_search","external_web_access":live}],"tool_choice":{"type":"web_search"},"include":["web_search_call.action.sources"],"stream":true,"store":false})
}
fn parse_response(bytes: &[u8]) -> Result<WebSearchResult, WebError> {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        return map_response(&value);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| error("Search returned non-UTF8 data"))?;
    for packet in text.replace("\r\n", "\n").split("\n\n") {
        if let Some(result) = parse_packet(packet) {
            return result;
        }
    }
    Err(error(
        "Hosted search stream ended without a completed response",
    ))
}
fn sse_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    lf.into_iter().chain(crlf).min_by_key(|(i, _)| *i)
}
fn parse_packet(packet: &str) -> Option<Result<WebSearchResult, WebError>> {
    let data = packet
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    let value: Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(_) => return Some(Err(error("Hosted search returned an invalid SSE event"))),
    };
    match value["type"].as_str() {
        Some("response.completed" | "response.done" | "response.incomplete") => {
            Some(map_response(&value["response"]))
        }
        Some("response.failed" | "error") => Some(Err(error(format!(
            "Hosted search failed: {}",
            value
                .get("error")
                .or_else(|| value["response"].get("error"))
                .unwrap_or(&Value::Null)
        )))),
        _ => None,
    }
}

fn map_response(value: &Value) -> Result<WebSearchResult, WebError> {
    if matches!(value["status"].as_str(), Some("failed" | "cancelled"))
        || value.get("error").is_some_and(|e| !e.is_null())
    {
        return Err(error("Hosted search did not complete successfully"));
    }
    let output = value["output"]
        .as_array()
        .ok_or_else(|| error("Hosted search returned no output items"))?;
    if !output.iter().any(|v| v["type"] == "web_search_call") {
        return Err(error("The provider did not execute native web search"));
    }
    if output
        .iter()
        .any(|v| v["type"] == "web_search_call" && v["status"] == "failed")
    {
        return Err(error("The hosted search tool failed"));
    }
    let mut sources = Vec::<WebSearchSource>::new();
    let mut text = Vec::new();
    let mut add = |item: &Value| {
        if let Some(url) = item["url"].as_str() {
            if let Some(source) = sources.iter_mut().find(|s| s.url == url) {
                if source.title.is_none() {
                    source.title = item["title"].as_str().map(str::to_string);
                }
                return;
            }
            if reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "https" | "http"))
                && !sources.iter().any(|s: &WebSearchSource| s.url == url)
            {
                sources.push(WebSearchSource {
                    url: url.into(),
                    title: item["title"].as_str().map(str::to_string),
                    snippet: None,
                    published_at: None,
                });
            }
        }
    };
    for item in output {
        if item["type"] == "web_search_call" {
            for source in item["action"]["sources"].as_array().into_iter().flatten() {
                add(source);
            }
        }
        for block in item["content"].as_array().into_iter().flatten() {
            if block["type"] == "output_text" {
                if let Some(value) = block["text"].as_str() {
                    text.push(value.to_string());
                }
            }
            for cite in block["annotations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|v| v["type"] == "url_citation")
            {
                add(cite);
            }
        }
    }
    let truncated = value["status"] == "incomplete";
    if truncated && sources.is_empty() {
        return Err(error("Hosted search ended before finding sources"));
    }
    Ok(WebSearchResult {
        content: (!sources.is_empty() && !text.is_empty()).then(|| text.join("\n")),
        sources,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hosted_protocol_keeps_native_sources_and_rejects_unsourced_answers() {
        let body = search_body("fixture", "query", false);
        assert_eq!(body["tools"][0]["external_web_access"], false);
        assert_eq!(body["store"], false);
        let value = json!({"status":"completed","output":[{"type":"web_search_call","status":"completed","action":{"sources":[{"url":"https://example.test/a"}]}},{"type":"message","content":[{"type":"output_text","text":"Answer","annotations":[{"type":"url_citation","url":"https://example.test/a","title":"A"}]}]}]});
        assert_eq!(map_response(&value).unwrap().sources.len(), 1);
        assert_eq!(
            parse_response(
                format!(
                    "data: {}\n\n",
                    json!({"type":"response.completed","response":value})
                )
                .as_bytes()
            )
            .unwrap()
            .content,
            Some("Answer".into())
        );
        assert!(map_response(&json!({"output":[{"type":"message","content":[{"type":"output_text","text":"Invented"}]}]})).is_err());
        assert!(parse_response(b"data: [DONE]\n").is_err());
        assert!(endpoint("https://user:key@example.test").is_err());
        assert_eq!(sse_boundary(b"data: {}\r\n\r\n"), Some((8, 4)));
        let multiline = "data: {\n data: ignored";
        assert!(parse_packet(multiline).unwrap().is_err());
        let empty = "data: {\ndata: \"type\":\"response.completed\",\ndata: \"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"web_search_call\",\"status\":\"completed\",\"action\":{\"sources\":[]}}]}}\n\n";
        assert!(parse_response(empty.as_bytes()).unwrap().sources.is_empty());
        assert!(map_response(&json!({"status":"failed","output":[{"type":"web_search_call","status":"completed"}]})).is_err());
    }
}
