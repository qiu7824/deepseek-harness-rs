use super::*;

pub(crate) fn endpoint(base: &str, suffix: &str) -> String {
    let base = base.trim_end_matches('/');
    let base = base
        .strip_suffix("/messages")
        .or_else(|| base.strip_suffix("/chat/completions"))
        .unwrap_or(base);
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
    anthropic::attach_replay(&mut chat, options, &connection.base_url);
    let body = anthropic::request_from_chat(&chat)?;
    let encoded = serde_json::to_vec(&body)
        .map_err(|_| failure("Anthropic request encode failed", "INVALID_REQUEST"))?;
    let mut headers = request_headers(connection, options.session_id.as_deref());
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
    let response = transport::post_tracked(
        &endpoint(&connection.base_url, "messages"),
        bearer,
        encoded,
        &headers,
        options.signal.clone(),
        options.telemetry.clone(),
    )
    .await
    .map_err(|error| failure(format!("Anthropic request failed: {error}"), "TRANSPORT"))?;
    if !response.status.is_success() {
        if let Some(telemetry) = &options.telemetry {
            telemetry.phase("error_body", None);
        }
        let status = response.status;
        let headers = response.headers.clone();
        let bytes = response
            .collect_limited(8 * 1024 * 1024)
            .await
            .unwrap_or_else(|error| {
                serde_json::json!({"error":{"message":error}})
                    .to_string()
                    .into_bytes()
            });
        return Err(http_failure(status, &headers, &bytes, provider_name));
    }
    consume_response(response, options, connection, sender).await
}

pub(crate) async fn consume_response(
    mut response: transport::CancelableResponse,
    options: &GenerateOptions,
    connection: &ResolvedDeepSeekOptions,
    sender: &tokio::sync::mpsc::Sender<StreamChunk>,
) -> Result<(), LlmFailure> {
    let mut parser = sse::SseParser::new();
    let mut translator = anthropic::AnthropicTranslator::new(&options.model, &connection.base_url);
    translator.set_protocol(if connection.api == messages::API {
        messages::API
    } else {
        "anthropic-messages"
    });
    let mut progress_deadline = tokio::time::Instant::now() + connection.stream_progress_timeout;
    loop {
        let read = tokio::time::timeout(connection.stream_idle_timeout, response.next_data());
        tokio::pin!(read);
        let bytes = loop {
            tokio::select! {
                result = &mut read => break result
                    .map_err(|_| failure("Anthropic stream idle timeout", "TIMEOUT"))?
                    .map_err(|error| failure(format!("Anthropic stream failed: {error}"), "TRANSPORT"))?,
                _ = sender.closed() => return Err(failure("Anthropic consumer closed", "CANCELLED")),
                _ = tokio::time::sleep_until(progress_deadline) => return Err(failure("[phase:stream_progress] No observable Anthropic progress before the deadline", "TIMEOUT")),
                _ = tokio::time::sleep(Duration::from_millis(15)) => {
                    if options.signal.as_ref().is_some_and(|signal| signal()) { return Err(failure("Anthropic stream cancelled", "CANCELLED")); }
                }
            }
        };
        let Some(bytes) = bytes else {
            break;
        };
        for payload in parser.push(&bytes) {
            let payload = payload?;
            let chunks = translator.consume(&payload)?;

            if chunks.len() > MAX_STREAM_EVENT_CHUNKS {
                return Err(failure(
                    "Anthropic event emitted too many chunks",
                    "RESPONSE_TOO_LARGE",
                ));
            }
            for chunk in chunks {
                if dsh_llm::is_token_delta(&chunk) {
                    progress_deadline =
                        tokio::time::Instant::now() + connection.stream_progress_timeout;
                }
                sender
                    .send(chunk)
                    .await
                    .map_err(|_| failure("Anthropic consumer closed", "CANCELLED"))?;
            }
            if translator.is_finished() {
                return Ok(());
            }
        }
    }
    for payload in parser.finish_at_eof() {
        let payload = payload?;
        let chunks = translator.consume(&payload)?;

        if chunks.len() > MAX_STREAM_EVENT_CHUNKS {
            return Err(failure(
                "Anthropic event emitted too many chunks",
                "RESPONSE_TOO_LARGE",
            ));
        }
        for chunk in chunks {
            sender
                .send(chunk)
                .await
                .map_err(|_| failure("Anthropic consumer closed", "CANCELLED"))?;
        }
        if translator.is_finished() {
            return Ok(());
        }
    }
    translator.finish()
}
