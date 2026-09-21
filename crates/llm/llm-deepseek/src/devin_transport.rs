use super::*;
use crate::devin_wire::{Encoder, MAX_FRAME, Message, unary_payload, uncompress};
use bytes::{Buf, BytesMut};
use serde_json::{Value, json};
use std::io::Write;

#[cfg(test)]
#[path = "devin_tests.rs"]
mod tests;

fn client() -> Result<reqwest::Client, String> {
    dsh_http_proxy::builder()?
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .user_agent("DeepSeek-Harness-rs")
        .build()
        .map_err(|_| "cannot construct Devin HTTP client".into())
}

pub(crate) fn endpoint(base: &str, path: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(base).map_err(|_| "invalid Devin base URL")?;
    let loopback = matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
    );
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Devin requires an HTTPS service origin; only local test services may use HTTP".into(),
        );
    }
    Ok(format!("{}{}", base.trim_end_matches('/'), path))
}

fn authorized_service(base: &str, custom: &str) -> Result<String, LlmFailure> {
    if custom.is_empty() || custom.trim_end_matches('/') == base.trim_end_matches('/') {
        return Ok(base.to_owned());
    }
    let url = reqwest::Url::parse(custom).map_err(|_| {
        failure(
            "Devin returned an invalid service endpoint",
            "UNTRUSTED_ENDPOINT",
        )
    })?;
    let trusted = url.host_str().is_some_and(|host| {
        host == "codeium.com"
            || host.ends_with(".codeium.com")
            || host == "windsurf.com"
            || host.ends_with(".windsurf.com")
    });
    if !trusted
        || url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(failure(
            "Devin returned a service outside the supported subscription endpoints",
            "UNTRUSTED_ENDPOINT",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn safe_error(status: reqwest::StatusCode, bytes: &[u8], secrets: &[&str]) -> LlmFailure {
    let parsed = (bytes.len() <= 16 * 1024)
        .then(|| serde_json::from_slice::<Value>(bytes).ok())
        .flatten();
    let message = parsed
        .as_ref()
        .and_then(|v| v.pointer("/error/message").or_else(|| v.get("message")))
        .and_then(Value::as_str);
    let mut message = message
        .unwrap_or("Devin service request failed")
        .to_string();
    for secret in secrets {
        if !secret.is_empty() {
            message = message.replace(secret, "[redacted]")
        }
    }
    let message = message.chars().take(2048).collect::<String>();
    http_failure(
        status,
        &Default::default(),
        json!({"error":{"message":message}}).to_string().as_bytes(),
        "Devin",
    )
}

async fn read_limited(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(bytes) = response
        .chunk()
        .await
        .map_err(|_| "Devin response read failed")?
    {
        if body.len().saturating_add(bytes.len()) > limit {
            return Err("Devin response exceeded its byte limit".into());
        }
        body.extend_from_slice(&bytes);
    }
    Ok(body)
}

async fn read_owned(
    response: reqwest::Response,
    limit: usize,
    sender: &tokio::sync::mpsc::Sender<StreamChunk>,
    cancelled: &Option<Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<Vec<u8>, LlmFailure> {
    let read = tokio::time::timeout(Duration::from_secs(25), read_limited(response, limit));
    tokio::pin!(read);
    loop {
        tokio::select! {
            result=&mut read=>return result.map_err(|_|failure("Devin response body timed out","TIMEOUT"))?.map_err(|error|failure(error,"TRANSPORT")),
            _=sender.closed()=>return Err(failure("Devin consumer closed","CANCELLED")),
            _=tokio::time::sleep(Duration::from_millis(15))=>if cancelled.as_ref().is_some_and(|signal|signal()){
                return Err(failure("Devin request cancelled","CANCELLED"));
            }
        }
    }
}

pub(crate) async fn catalog(base: &str, token: &str) -> Result<Value, String> {
    let mut body = Encoder::default();
    body.bytes(1, &devin::metadata(token, "", true));
    let response = client()?
        .post(endpoint(base, devin::CATALOG_PATH)?)
        .header("content-type", "application/proto")
        .header("connect-protocol-version", "1")
        .body(body.0)
        .timeout(Duration::from_secs(25))
        .send()
        .await
        .map_err(|_| "Devin model catalog connection failed")?;
    let status = response.status();
    let bytes = read_limited(response, MAX_FRAME).await?;
    if !status.is_success() {
        return Err(safe_error(status, &bytes, &[token]).message);
    }
    devin::catalog_from_bytes(&unary_payload(&bytes)?)
}

async fn send(
    request: reqwest::RequestBuilder,
    sender: &tokio::sync::mpsc::Sender<StreamChunk>,
    cancelled: &Option<Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<reqwest::Response, LlmFailure> {
    let response = tokio::time::timeout(Duration::from_secs(60), request.send());
    tokio::pin!(response);
    loop {
        tokio::select! {
            result=&mut response=>return result.map_err(|_|failure("Devin response headers timed out","TIMEOUT"))?
                .map_err(|_|failure("Devin connection failed","TRANSPORT")),
            _=sender.closed()=>return Err(failure("Devin stream consumer closed","CANCELLED")),
            _=tokio::time::sleep(Duration::from_millis(15))=>if cancelled.as_ref().is_some_and(|signal|signal()){
                return Err(failure("Devin request cancelled","CANCELLED"));
            }
        }
    }
}

pub(crate) async fn request(
    chat: &Value,
    options: &GenerateOptions,
    connection: &ResolvedDeepSeekOptions,
    token: &str,
    sender: &tokio::sync::mpsc::Sender<StreamChunk>,
    cancelled: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<(), LlmFailure> {
    let client = client().map_err(|e| failure(e, "INVALID_CONFIG"))?;
    let mut auth = Encoder::default();
    auth.bytes(1, &devin::metadata(token, "", false));
    let response = send(
        client
            .post(
                endpoint(&connection.base_url, devin::AUTH_PATH)
                    .map_err(|e| failure(e, "INVALID_CONFIG"))?,
            )
            .header("content-type", "application/proto")
            .header("connect-protocol-version", "1")
            .body(auth.0)
            .timeout(Duration::from_secs(25)),
        sender,
        &cancelled,
    )
    .await?;
    let status = response.status();
    let bytes = read_owned(response, MAX_FRAME, sender, &cancelled).await?;
    if !status.is_success() {
        return Err(safe_error(status, &bytes, &[token]));
    }
    let auth = unary_payload(&bytes).map_err(|e| failure(e, "MALFORMED_RESPONSE"))?;
    let auth = Message::parse(&auth).map_err(|e| failure(e, "MALFORMED_RESPONSE"))?;
    let jwt = auth.text(1).map_err(|e| failure(e, "MALFORMED_RESPONSE"))?;
    if jwt.is_empty() {
        return Err(failure(
            "Devin did not authorize a user session",
            "UNAUTHORIZED",
        ));
    }
    let custom = auth.text(2).map_err(|e| failure(e, "MALFORMED_RESPONSE"))?;
    let mut effective = connection.clone();
    effective.base_url = authorized_service(&connection.base_url, custom)?;
    let scope = devin::scope(options, &effective, token);
    let mut chat = chat.clone();
    let cascade = devin::prepare_replay(&mut chat, options, &scope);
    let body = devin::chat_request(&chat, token, jwt, &cascade)
        .map_err(|e| failure(e, "INVALID_REQUEST"))?;
    let body = {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(&body)
            .map_err(|_| failure("Devin request compression failed", "INVALID_REQUEST"))?;
        gzip.finish()
            .map_err(|_| failure("Devin request compression failed", "INVALID_REQUEST"))?
    };
    let mut envelope = Vec::with_capacity(body.len() + 5);
    envelope.push(1);
    envelope.extend_from_slice(&(body.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&body);
    if let Some(telemetry) = &options.telemetry {
        telemetry.network_start();
    }
    let response = send(
        client
            .post(
                endpoint(&effective.base_url, devin::CHAT_PATH)
                    .map_err(|e| failure(e, "INVALID_CONFIG"))?,
            )
            .version(reqwest::Version::HTTP_11)
            .header("content-type", "application/connect+proto")
            .header("connect-protocol-version", "1")
            .header("connect-accept-encoding", "gzip")
            .header("connect-content-encoding", "gzip")
            .body(envelope),
        sender,
        &cancelled,
    )
    .await?;
    if let Some(telemetry) = &options.telemetry {
        telemetry.phase("response_headers", None);
    }
    let status = response.status();
    if !status.is_success() {
        let bytes = read_owned(response, 1024 * 1024, sender, &cancelled).await?;
        return Err(safe_error(status, &bytes, &[token, jwt]));
    }
    let mut response = response;
    let mut buffer = BytesMut::new();
    let mut translator = devin::NativeTranslator::new();
    let mut ended = false;
    let mut progress_deadline = tokio::time::Instant::now() + connection.stream_progress_timeout;
    loop {
        let next = tokio::time::timeout(connection.stream_idle_timeout, response.chunk());
        tokio::pin!(next);
        let chunk = loop {
            tokio::select! {
                result=&mut next=>break result.map_err(|_|failure("Devin stream idle timeout","TIMEOUT"))?
                    .map_err(|_|failure("Devin stream connection failed","TRANSPORT"))?,
                _=sender.closed()=>return Err(failure("Devin stream consumer closed","CANCELLED")),
                _=tokio::time::sleep_until(progress_deadline)=>return Err(failure("[phase:stream_progress] No observable Devin progress before the deadline","TIMEOUT")),
                _=tokio::time::sleep(Duration::from_millis(15))=>if cancelled.as_ref().is_some_and(|signal|signal()){
                    return Err(failure("Devin stream cancelled","CANCELLED"));
                }
            }
        };
        let Some(chunk) = chunk else { break };
        buffer.extend_from_slice(&chunk);
        while buffer.len() >= 5 {
            let flags = buffer[0];
            let size = u32::from_be_bytes(buffer[1..5].try_into().unwrap()) as usize;
            if flags & !3 != 0 || size > MAX_FRAME {
                return Err(failure(
                    "Invalid or oversized Devin Connect frame",
                    "MALFORMED_RESPONSE",
                ));
            }
            if buffer.len() < size + 5 {
                break;
            }
            buffer.advance(5);
            let data = buffer.split_to(size);
            let payload = if flags & 1 != 0 {
                uncompress(&data).map_err(|e| failure(e, "MALFORMED_RESPONSE"))?
            } else {
                data.to_vec()
            };
            if flags & 2 != 0 {
                let trailer: Value = serde_json::from_slice(&payload)
                    .map_err(|_| failure("Invalid Devin stream trailer", "MALFORMED_RESPONSE"))?;
                if let Some(error) = trailer.get("error").filter(|e| !e.is_null()) {
                    let status = match error["code"].as_str() {
                        Some("unauthenticated") => 401,
                        Some("permission_denied") => 403,
                        Some("resource_exhausted") => 429,
                        _ => 400,
                    };
                    return Err(safe_error(
                        reqwest::StatusCode::from_u16(status).unwrap(),
                        &payload,
                        &[token, jwt],
                    ));
                }
                ended = true;
                break;
            }
            for chunk in translator.consume(&payload)? {
                if dsh_llm::is_token_delta(&chunk) {
                    progress_deadline =
                        tokio::time::Instant::now() + connection.stream_progress_timeout;
                }
                sender
                    .send(chunk)
                    .await
                    .map_err(|_| failure("Devin consumer closed", "CANCELLED"))?;
            }
        }
        if ended {
            break;
        }
    }
    if !ended || !buffer.is_empty() {
        return Err(failure(
            "Devin stream ended without a complete trailer",
            "TRUNCATED_RESPONSE",
        ));
    }
    for chunk in translator.finish(&scope, &cascade)? {
        sender
            .send(chunk)
            .await
            .map_err(|_| failure("Devin consumer closed", "CANCELLED"))?;
    }
    Ok(())
}
