//! Explicit, reviewable feedback packages with an optional user-configured delivery endpoint.
use axum::body::{Body, to_bytes};
use cordis::Context;
use dsh_host_webserver::{WebRequest, WebResponse, WebRoute, WebRouteKind, WebServer};
use dsh_session::{SessionHeader, SessionStore};
use dsh_session_persistence::{SessionInspection, SessionPersistenceApi};
use http::{Method, StatusCode, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Submission {
    payload: Value,
    session: SessionHeader,
    status: String,
    attempts: u32,
    destination_key: Option<String>,
    destination: Option<String>,
    last_error: Option<String>,
    delivered_at: Option<i64>,
}

#[derive(Clone)]
struct Destination {
    url: reqwest::Url,
    display: String,
    key: String,
    loopback: bool,
}
pub(super) fn validate_config(value: &Value) -> Result<(), String> {
    let mut configured = value.clone();
    configured["enabled"] = json!(true);
    destination(&configured).map(|_| ())
}
fn destination(config: &Value) -> Result<Option<Destination>, String> {
    if config["enabled"] != true {
        return Ok(None);
    }
    let raw = config["endpoint"].as_str().unwrap_or("").trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let url = reqwest::Url::parse(raw).map_err(|_| "反馈接收地址无效".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err("反馈接收地址必须是 HTTP / HTTPS 地址，且不能包含用户名或密码".into());
    }
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host == "::1"
            || host == "[::1]"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if url.scheme() == "http" && !loopback {
        return Err("远端反馈接收地址必须使用 HTTPS".into());
    }
    let display = url.origin().ascii_serialization();
    let key = format!("{:x}", Sha256::digest(url.as_str().as_bytes()));
    Ok(Some(Destination {
        url,
        display,
        key,
        loopback,
    }))
}
fn packet_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    let id = uuid::Uuid::parse_str(id).map_err(|_| "反馈提交标识无效".to_string())?;
    Ok(root.join(format!("{id}.json")))
}
async fn read(path: &Path) -> Result<Option<Submission>, String> {
    match tokio::fs::metadata(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
        Ok(meta) if meta.len() > MAX_FILE_BYTES => return Err("反馈提交包超过容量限制".into()),
        _ => {}
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "反馈提交包不可读取".into())
}
async fn save(path: &Path, value: &Submission) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("反馈提交包超过容量限制".into());
    }
    dsh_atomic_write::write_file_atomic(
        path,
        &bytes,
        dsh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .map_err(|error| error.to_string())
}
fn short_text(text: &str, maximum: usize) -> (&str, bool) {
    let mut end = text.len().min(maximum);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], end < text.len())
}
fn capture(
    inspection: &SessionInspection,
    message_id: Option<&str>,
) -> Result<(Vec<Value>, Option<u64>, usize), String> {
    let target = if let Some(id) = message_id {
        Some(
            inspection
                .events
                .iter()
                .find(|event| {
                    event.type_ == "assistant/message"
                        && dsh_session::derive_event_message(event)
                            .is_some_and(|message| message.id.as_str() == id)
                })
                .ok_or_else(|| "反馈对应的回答已不存在".to_string())?
                .seq
                .get(),
        )
    } else {
        inspection.events.last().map(|event| event.seq.get())
    };
    let end = inspection
        .events
        .partition_point(|event| target.is_some_and(|seq| event.seq.get() <= seq));
    let prefix = &inspection.events[..end];
    let surface = dsh_session::fold_surface(prefix)?;
    let mut messages = Vec::new();
    let mut budget = MAX_TRANSCRIPT_BYTES;
    let mut omitted = 0;
    for seq in surface.nodes.into_iter().rev() {
        let Some(event) = prefix.get(seq as usize) else {
            continue;
        };
        let Some(message) = dsh_session::derive_event_message(event) else {
            continue;
        };
        if !matches!(
            message.source,
            dsh_llm::MessageSource::User { .. } | dsh_llm::MessageSource::Model { .. }
        ) {
            continue;
        }
        let text = message
            .content
            .iter()
            .filter_map(|block| match block {
                dsh_llm::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            continue;
        }
        if budget == 0 || messages.len() >= 2000 {
            omitted += 1;
            continue;
        }
        let (text, truncated) = short_text(&text, budget.min(64 * 1024));
        budget -= text.len();
        if truncated {
            omitted += 1;
        }
        messages
            .push(json!({"id":message.id,"role":message.role,"text":text,"truncated":truncated}));
    }
    messages.reverse();
    Ok((messages, target, omitted))
}
fn view(value: &Submission, target: Option<&Destination>) -> Value {
    let changed = value
        .destination_key
        .as_ref()
        .is_some_and(|key| target.is_some_and(|target| key != &target.key));
    json!({"submissionId":value.payload["submissionId"],"status":if value.status=="sending"{"failed"}else{value.status.as_str()},"attempts":value.attempts,
        "lastError":if changed&&value.status!="delivered"{Some("接收地址已改变，请重新截取后确认")}else if value.status=="sending"{Some("上一次提交未获得确认，请重试")}else{value.last_error.as_deref()},"deliveredAt":value.delivered_at,
        "destination":value.destination.as_deref().or_else(||target.map(|destination|destination.display.as_str())),"destinationKey":target.map(|destination|destination.key.as_str()),
        "canSend":target.is_some()&&!changed&&value.status!="delivered","payload":value.payload})
}
async fn transmit(path: &Path, value: &mut Submission, target: &Destination) -> Result<(), String> {
    if value.status == "delivered" {
        return Ok(());
    }
    if value
        .destination_key
        .as_ref()
        .is_some_and(|key| key != &target.key)
    {
        return Err("接收地址已改变，请创建新的提交包后确认".into());
    }
    value.status = "sending".into();
    value.attempts += 1;
    value.destination_key = Some(target.key.clone());
    value.destination = Some(target.display.clone());
    value.last_error = None;
    save(path, value).await?;
    let outcome = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none());
        let client = if target.loopback {
            client.no_proxy()
        } else {
            client
        };
        let client = client.build().map_err(|_| "无法创建反馈连接".to_string())?;
        let response = client
            .post(target.url.clone())
            .header(
                "Idempotency-Key",
                value.payload["submissionId"].as_str().unwrap_or(""),
            )
            .json(&value.payload)
            .send()
            .await
            .map_err(|error| error.without_url().to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "接收端未确认提交：HTTP {}",
                response.status().as_u16()
            ));
        }
        Ok(())
    }
    .await;
    match &outcome {
        Ok(()) => {
            value.status = "delivered".into();
            value.delivered_at = Some(chrono::Utc::now().timestamp_millis());
        }
        Err(error) => {
            value.status = "failed".into();
            value.last_error = Some(error.clone());
        }
    }
    save(path, value).await?;
    outcome
}
fn response(status: StatusCode, value: Value) -> WebResponse {
    http::Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(value.to_string()))
        .expect("feedback response")
}
async fn operation(
    ctx: &Context,
    root: &Path,
    settings: &dsh_settings::SettingsProvider,
    path: &str,
    body: Value,
) -> Result<Value, String> {
    let session_id = body["sessionId"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 200)
        .ok_or_else(|| "请选择会话".to_string())?;
    let id = body[if path.ends_with("/prepare") {
        "requestId"
    } else {
        "submissionId"
    }]
    .as_str()
    .ok_or_else(|| "缺少提交标识".to_string())?;
    let file = packet_path(root, id)?;
    let persistence = ctx
        .get_typed::<Arc<dyn SessionPersistenceApi>>("sessionPersistence", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "会话存储尚未就绪".to_string())?;
    let feedback = ctx
        .get_typed::<Arc<dsh_message_feedback::MessageFeedbackService>>("messageFeedback", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "反馈服务尚未就绪".to_string())?;
    let config = settings
        .get(&dsh_settings::SettingsNamespace::new("feedback-delivery"))
        .and_then(|data| data.to_json())
        .unwrap_or_else(|| json!({}));
    let target = destination(&config)?;
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|error| error.to_string())?;
    async {
        let mut stored = read(&file).await?;
        let inspection = persistence.inspect(&dsh_session::session_id(session_id)).await?;
        if let Some(value)=&stored { if value.session != inspection.meta { return Err("反馈提交包不属于当前会话生命周期".into()); } }
        if path.ends_with("/prepare") {
            let note = body["note"].as_str().unwrap_or("").trim(); if note.len()>16384 { return Err("反馈说明过长".into()); }
            let message_id = body["messageId"].as_str();
            if let Some(value)=&stored {
                if value.payload["note"].as_str()!=Some(note) || value.payload["messageId"].as_str()!=message_id { return Err("提交标识已用于另一份反馈".into()); }
            } else {
                if let Some(sessions)=ctx.get_typed::<Arc<SessionStore>>("sessions",false) { if let Some(live)=sessions.get(&inspection.meta.id) { sessions.flush(&live).await?; } }
                let (messages,captured,omitted)=capture(&inspection,message_id)?;
                let items=feedback.list(&dsh_message_feedback::MessageFeedbackListRequest{session_id:inspection.meta.id.clone()}).await.map_err(|failure|format!("反馈读取失败：{:?}",failure.error))?.value.items.into_iter().filter(|item|message_id.is_none_or(|id|item.message_id.as_str()==id)).collect::<Vec<_>>();
                if note.is_empty()&&items.is_empty(){return Err("请先填写反馈说明或评价回答".into())}
                let value=Submission{payload:json!({"formatVersion":1,"submissionId":id,"sessionId":session_id,"createdAt":chrono::Utc::now().timestamp_millis(),"capturedThroughSeq":captured,"messageId":message_id,"note":note,"feedback":items,"messages":messages,"omittedMessages":omitted}),session:inspection.meta,status:"local".into(),attempts:0,destination_key:None,destination:None,last_error:None,delivered_at:None};
                save(&file,&value).await?;stored=Some(value);
            }
        }
        let mut value=stored.ok_or_else(||"反馈提交包不存在".to_string())?;
        if path.ends_with("/send") {
            let target=target.as_ref().ok_or_else(||"反馈已保存到本地；远端提交未启用或未配置接收地址".to_string())?;
            if body["destinationKey"].as_str()!=Some(target.key.as_str()){return Err("接收地址已改变，请重新查看提交包".into())}
            transmit(&file,&mut value,target).await?;
        }
        Ok(view(&value,target.as_ref()))
    }.await
}
pub(super) fn register(
    server: &Arc<WebServer>,
    ctx: &Context,
    data_root: &Path,
    settings: Arc<dsh_settings::SettingsProvider>,
    allow_remote: bool,
) {
    let root = data_root.join("feedback-submissions");
    let ctx = ctx.clone();
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let _ = server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/__dsh-feedback".into(),
        handler: Arc::new(move |request: WebRequest| {
            let ctx = ctx.clone();
            let root = root.clone();
            let settings = settings.clone();
            let gate = gate.clone();
            Box::pin(async move {
                if !super::trusted_web_request(&request, allow_remote) {
                    return Ok(response(
                        StatusCode::FORBIDDEN,
                        json!({"error":"请求来源不可信"}),
                    ));
                }
                let path = request.uri().path().to_string();
                if request.method() != Method::POST
                    || !matches!(
                        path.as_str(),
                        "/__dsh-feedback/prepare"
                            | "/__dsh-feedback/status"
                            | "/__dsh-feedback/send"
                    )
                {
                    return Ok(response(
                        StatusCode::NOT_FOUND,
                        json!({"error":"反馈入口不存在"}),
                    ));
                }
                let bytes = match to_bytes(Body::new(request.into_body()), 32768).await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return Ok(response(
                            StatusCode::BAD_REQUEST,
                            json!({"error":"请求过大"}),
                        ));
                    }
                };
                let body = match serde_json::from_slice(&bytes) {
                    Ok(value) => value,
                    Err(_) => {
                        return Ok(response(
                            StatusCode::BAD_REQUEST,
                            json!({"error":"请求格式无效"}),
                        ));
                    }
                };
                let _guard = gate.lock().await;
                Ok(match operation(&ctx, &root, &settings, &path, body).await {
                    Ok(value) => response(StatusCode::OK, value),
                    Err(error) => response(StatusCode::BAD_REQUEST, json!({"error":error})),
                })
            })
        }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::{SessionEvent, SessionSeq, SurfaceOp};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn header() -> SessionHeader {
        SessionHeader {
            id: dsh_session::session_id("feedback-fixture"),
            created_at: 1,
            cwd: None,
            version: dsh_session::SESSION_FORMAT_VERSION,
            parent_session: None,
            is_seeded: false,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }
    }
    fn fixture_packet(id: &str) -> Submission {
        Submission {
            payload: json!({"submissionId":id,"sessionId":"feedback-fixture","messages":[{"role":"assistant","text":"fixture only"}]}),
            session: header(),
            status: "local".into(),
            attempts: 0,
            destination_key: None,
            destination: None,
            last_error: None,
            delivered_at: None,
        }
    }
    #[test]
    fn destinations_require_explicit_enable_and_hide_credentials() {
        assert!(
            destination(&json!({"endpoint":"https://example.invalid/private?token=secret"}))
                .unwrap()
                .is_none()
        );
        let target = destination(
            &json!({"enabled":true,"endpoint":"https://example.invalid/private?token=secret"}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(target.display, "https://example.invalid");
        assert!(destination(&json!({"enabled":true,"endpoint":"http://example.invalid"})).is_err());
        assert!(
            destination(&json!({"enabled":true,"endpoint":"https://user:secret@example.invalid"}))
                .is_err()
        );
        assert!(
            destination(&json!({"enabled":true,"endpoint":"http://127.0.0.1:1234/feedback"}))
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn captured_prefix_contains_only_reviewable_conversation_text() {
        let event = |seq, kind: &str, data, surface| SessionEvent {
            type_: kind.into(),
            seq: SessionSeq::new(seq).unwrap(),
            time: 1,
            data,
            ignorable: None,
            surface_op: surface,
            source_event_seqs: None,
        };
        let inspection = SessionInspection {
            meta: header(),
            inherited_event_count: dsh_session::SessionLogOffset::ZERO,
            events: vec![
                event(
                    0,
                    "user/message",
                    json!({"id":"user-1","role":"user","content":[{"type":"text","text":"Review this"}],"source":{"kind":"user"}}),
                    Some(SurfaceOp::Append),
                ),
                event(
                    1,
                    "request/header",
                    json!({"apiKey":"must-not-be-submitted"}),
                    None,
                ),
                event(
                    2,
                    "assistant/message",
                    json!({"turn":1,"step":1,"message":{"id":"answer-1","role":"assistant","content":[{"type":"text","text":"Reviewed"}],"source":{"kind":"model","provider":"fixture","model":"fixture"}}}),
                    Some(SurfaceOp::Append),
                ),
                event(
                    3,
                    "tool/call",
                    json!({"secret":"must-not-be-submitted"}),
                    None,
                ),
            ],
        };
        let (messages, seq, omitted) = capture(&inspection, Some("answer-1")).unwrap();
        assert_eq!(seq, Some(2));
        assert_eq!(messages.len(), 2);
        assert_eq!(omitted, 0);
        assert!(
            !serde_json::to_string(&messages)
                .unwrap()
                .contains("must-not-be-submitted")
        );
        assert!(capture(&inspection, Some("missing")).is_err());
    }
    #[tokio::test]
    async fn only_explicit_transmit_contacts_mock_and_retries_exact_payload_identity() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let receiver = tokio::spawn(async move {
            let mut received = Vec::new();
            for status in [503, 200] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (header_end, size) = loop {
                    let mut chunk = [0_u8; 4096];
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let size = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|value| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (end + 4, size);
                    }
                };
                while bytes.len() < header_end + size {
                    let mut chunk = [0_u8; 4096];
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                received.push((
                    String::from_utf8_lossy(&bytes[..header_end]).into_owned(),
                    bytes[header_end..header_end + size].to_vec(),
                ));
                stream.write_all(format!("HTTP/1.1 {status} fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            }
            received
        });
        let root = std::env::var_os("DSH_TEST_TEMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("feedback-delivery-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = packet_path(&root, &id).unwrap();
        let target =
            destination(&json!({"enabled":true,"endpoint":format!("http://{address}/feedback")}))
                .unwrap()
                .unwrap();
        let mut value = fixture_packet(&id);
        save(&path, &value).await.unwrap();
        assert_eq!(read(&path).await.unwrap().unwrap().status, "local");
        assert_eq!(view(&value, None)["canSend"], false);
        assert!(transmit(&path, &mut value, &target).await.is_err());
        assert_eq!(value.status, "failed");
        let mut retry = read(&path).await.unwrap().unwrap();
        transmit(&path, &mut retry, &target).await.unwrap();
        assert_eq!(retry.status, "delivered");
        let requests = receiver.await.unwrap();
        assert_eq!(
            requests[0].1, requests[1].1,
            "retry sends the original captured bytes"
        );
        for (headers, _) in requests {
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains(&format!("idempotency-key: {id}"))
            );
        }
        transmit(&path, &mut retry, &target).await.unwrap();
        assert_eq!(
            retry.attempts, 2,
            "acknowledged submission never sends again"
        );
        let restored = read(&path).await.unwrap().unwrap();
        assert_eq!(restored.status, "delivered");
        tokio::fs::remove_dir_all(&root).await.unwrap();
    }
}
