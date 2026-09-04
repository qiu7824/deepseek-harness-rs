use super::*;

pub(crate) fn endpoint(base: &str, suffix: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/{suffix}")
    } else {
        format!("{base}/v1/{suffix}")
    }
}

pub(crate) async fn request(
    chat: &serde_json::Value,
    options: &GenerateOptions,
    connection: &ResolvedDeepSeekOptions,
    api_key: &str,
    provider_name: &str,
    sender: &tokio::sync::mpsc::Sender<StreamChunk>,
) -> Result<(), LlmFailure> {
    let mut chat = chat.clone();
    if options
        .reasoning_effort
        .as_ref()
        .is_some_and(|effort| effort.as_str() == "off")
    {
        chat["reasoning_effort"] = serde_json::json!("off");
    }
    anthropic::attach_replay(&mut chat, options);
    let body = anthropic::request_from_chat(&chat)?;
    let encoded = serde_json::to_vec(&body)
        .map_err(|_| failure("Anthropic request encode failed", "INVALID_REQUEST"))?;
    let mut headers = request_headers(connection);
    headers.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
    let official = reqwest::Url::parse(&connection.base_url)
        .ok()
        .is_some_and(|url| url.host_str() == Some("api.anthropic.com"));
    let bearer = if official && !connection.oauth {
        headers.push(("x-api-key".to_string(), api_key.to_string()));
        None
    } else {
        (!connection.keyless).then_some(api_key)
    };
    let mut response = transport::post(
        &endpoint(&connection.base_url, "messages"),
        bearer,
        encoded,
        &headers,
        None,
    )
    .await
    .map_err(|error| failure(format!("Anthropic request failed: {error}"), "TRANSPORT"))?;
    if !response.status.is_success() {
        let status = response.status;
        let headers = response.headers.clone();
        let bytes = response
            .collect_limited(8 * 1024 * 1024)
            .await
            .unwrap_or_default();
        return Err(http_failure(status, &headers, &bytes, provider_name));
    }
    let mut parser = sse::SseParser::new();
    let mut translator = anthropic::AnthropicTranslator::default();
    let mut bytes_read = 0usize;
    let mut chunks_read = 0usize;
    while let Some(bytes) =
        tokio::time::timeout(connection.stream_idle_timeout, response.next_data())
            .await
            .map_err(|_| failure("Anthropic stream idle timeout", "TIMEOUT"))?
            .map_err(|error| failure(format!("Anthropic stream failed: {error}"), "TRANSPORT"))?
    {
        bytes_read = bytes_read.saturating_add(bytes.len());
        if bytes_read > MAX_SUCCESS_RESPONSE_BYTES {
            return Err(failure(
                "Anthropic response exceeded 8 MiB",
                "RESPONSE_TOO_LARGE",
            ));
        }
        for payload in parser.push(&bytes)? {
            let chunks = translator.consume(&payload)?;
            chunks_read = chunks_read.saturating_add(chunks.len());
            if chunks_read > MAX_SUCCESS_STREAM_CHUNKS {
                return Err(failure(
                    "Anthropic response emitted too many chunks",
                    "RESPONSE_TOO_LARGE",
                ));
            }
            for chunk in chunks {
                sender
                    .send(chunk)
                    .await
                    .map_err(|_| failure("Anthropic consumer closed", "CANCELLED"))?;
            }
        }
    }
    for payload in parser.finish_at_eof()? {
        for chunk in translator.consume(&payload)? {
            sender
                .send(chunk)
                .await
                .map_err(|_| failure("Anthropic consumer closed", "CANCELLED"))?;
        }
    }
    translator.finish()
}
