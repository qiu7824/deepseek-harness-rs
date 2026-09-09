//! Same-origin GUI bridge for the shared Computer Use runtime.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use base64::Engine;
use dsh_agent::{Agent, AgentRegistry};
use dsh_host_webserver::{
    RouteDisposer, WebHandlerError, WebRequest, WebResponse, WebRoute, WebRouteKind, WebServer,
};
use dsh_session::session_id;
use dsh_tool_computer_use_command::{AbortPredicate, ComputerUseRuntime};
use http::{Method, Response, StatusCode, header};
use serde::Serialize;
use serde_json::{Map, Value, json};

const ROUTE: &str = "/__dsh-computer-use";
const MAX_CONTROL_BYTES: usize = 64 * 1024;
const MAX_ANNOTATION_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_BROWSER_SESSION_ID: &str = "default";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    error: String,
    message: String,
}

fn json_response(status: StatusCode, value: &impl Serialize) -> WebResponse {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(
            serde_json::to_vec(value).unwrap_or_else(|_| b"{\"error\":\"internal\"}".to_vec()),
        ))
        .expect("computer-use response")
}

pub(super) fn error(
    status: StatusCode,
    code: impl Into<String>,
    message: impl Into<String>,
) -> WebResponse {
    json_response(
        status,
        &ErrorBody {
            error: code.into(),
            message: message.into(),
        },
    )
}

pub(super) struct ControlOwner {
    pub(super) id: String,
    agent: Option<Arc<dyn Agent>>,
}
pub(super) async fn valid_owner(
    registry: &AgentRegistry,
    persistence: &Option<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>,
    runtime: &Option<Arc<ComputerUseRuntime>>,
    value: &Value,
) -> Result<ControlOwner, WebResponse> {
    let owner = value
        .get("ownerSessionId")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty() && value.len() <= 200 && !value.chars().any(char::is_control)
        })
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "session-required",
                "缺少有效的 ownerSessionId",
            )
        })?;

    let agent = registry.get(&session_id(owner));
    if agent.is_none()
        && !runtime
            .as_ref()
            .is_some_and(|runtime| runtime.has_session_activity(owner))
    {
        let exists = match persistence {
            Some(p) => p
                .list()
                .await
                .map(|rows| rows.iter().any(|row| row.id.as_str() == owner))
                .unwrap_or(false),
            None => false,
        };
        if !exists {
            return Err(error(
                StatusCode::FORBIDDEN,
                "session-not-found",
                "会话不存在，无法绑定控制环境",
            ));
        }
    }
    Ok(ControlOwner {
        id: owner.to_string(),
        agent,
    })
}

async fn parse_body(request: WebRequest, limit: usize) -> Result<Value, WebResponse> {
    let bytes = to_bytes(Body::new(request.into_body()), limit)
        .await
        .map_err(|_| {
            error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request-too-large",
                "浏览器控制请求超过 64 KiB",
            )
        })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        error(
            StatusCode::BAD_REQUEST,
            "invalid-json",
            "浏览器控制请求不是有效 JSON",
        )
    })?;
    if !value.is_object() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "浏览器控制请求必须为对象",
        ));
    }
    Ok(value)
}

fn adapter_arguments(value: &Value) -> Result<Value, WebResponse> {
    let mut object = value.as_object().cloned().unwrap_or_else(Map::new);
    object.remove("ownerSessionId");
    let browser_session = object
        .remove("browserSessionId")
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| DEFAULT_BROWSER_SESSION_ID.to_string());
    object.insert("sessionId".to_string(), Value::String(browser_session));
    if object.get("action").and_then(Value::as_str).is_none() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "action-required",
            "缺少浏览器 action",
        ));
    }
    Ok(Value::Object(object))
}

async fn handle(
    request: WebRequest,
    agents: Arc<AgentRegistry>,
    runtime: Option<Arc<ComputerUseRuntime>>,
    persistence: Option<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>,
    allow_remote_host: bool,
) -> WebResponse {
    let allowed_host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|authority| super::allowed_web_authority(authority, allow_remote_host));
    if !allowed_host || !super::trusted_web_request(&request, allow_remote_host) {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "浏览器控制请求来源不可信",
        );
    }
    if request.method() != Method::POST {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "method-not-allowed",
            "浏览器控制只允许 POST",
        );
    }
    let operation = request
        .uri()
        .path()
        .strip_prefix(ROUTE)
        .unwrap_or_default()
        .trim_matches('/')
        .to_string();
    let body_limit = if operation == "annotation" { MAX_ANNOTATION_BYTES } else { MAX_CONTROL_BYTES };
    let body = match parse_body(request, body_limit).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let owner = match valid_owner(&agents, &persistence, &runtime, &body).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if operation == "meta" {
        let availability = runtime.as_ref().map(|runtime| runtime.availability());
        let available = availability.as_ref().is_some_and(|result| result.is_ok());
        let availability_error = availability.and_then(Result::err).map(|failure| {
            json!({
                "code": failure.code,
                "message": failure.message
            })
        });
        let actions = runtime.as_ref().map_or(&[][..], |runtime| {
            runtime
                .supported_actions()
                .unwrap_or_else(|| runtime.human_only_actions())
        });
        return json_response(
            StatusCode::OK,
            &json!({
                "enabled": runtime.is_some(),
                "available": available,
                "error": availability_error,
                "adapter": runtime.as_ref().map(|runtime| runtime.adapter_id()),
                "ownerSessionId": owner.id,
                "defaultBrowserSessionId": DEFAULT_BROWSER_SESSION_ID,
                "actions": actions,
                "humanOnlyActions": runtime.as_ref().map_or(&[][..], |runtime| runtime.human_only_actions()),
                "capabilitiesKnown": runtime.as_ref().is_some_and(|runtime| runtime.supported_actions().is_some())
            }),
        );
    }
    if operation == "annotation" {
        let Some(agent) = owner.agent else {
            return error(StatusCode::CONFLICT, "annotation-agent-unavailable", "当前会话没有可接收批注的智能体");
        };
        let annotations = body.get("annotations").cloned().unwrap_or_else(|| json!({}));
        let raw = serde_json::to_string(&annotations).unwrap_or_default();
        if raw.len() > 48 * 1024 {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "annotation-too-large", "批注内容超过大小限制");
        }
        let mut text = String::from("画面批注（已提交给智能体）：\n");
        if let Some(notes) = annotations.get("notes").and_then(Value::as_array) {
            for note in notes.iter().filter_map(|item| item.get("text").and_then(Value::as_str)) {
                text.push_str("- ");
                text.push_str(note);
                text.push('\n');
            }
        }
        if let Some(strokes) = annotations.get("strokes").and_then(Value::as_array) {
            text.push_str(&format!("标注线条：{} 条（坐标已按当前画面归一化）\n", strokes.len()));
        }
        let mut content = vec![dsh_llm::ContentBlock::Text { text }];
        if let Some(screenshot) = body.get("screenshot").and_then(Value::as_object) {
            let media_type = match screenshot.get("mediaType").and_then(Value::as_str).unwrap_or("image/jpeg") {
                "image/png" => dsh_attachment::ImageMediaType::Png,
                "image/webp" => dsh_attachment::ImageMediaType::Webp,
                "image/gif" => dsh_attachment::ImageMediaType::Gif,
                _ => dsh_attachment::ImageMediaType::Jpeg,
            };
            let encoded = screenshot.get("base64").and_then(Value::as_str).unwrap_or("");
            let data = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| error(StatusCode::BAD_REQUEST, "annotation-image-invalid", "批注截图编码无效"));
            let data = match data { Ok(data) => data, Err(response) => return response };
            if data.len() > 3 * 1024 * 1024 { return error(StatusCode::PAYLOAD_TOO_LARGE, "annotation-image-too-large", "批注截图超过大小限制"); }
            let Some(store) = agent.ctx().get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false).map(|slot| slot.as_ref().clone()) else { return error(StatusCode::CONFLICT, "annotation-attachments-unavailable", "当前主机没有可用的图片附件存储"); };
            let reference = match store.save_image(&dsh_attachment::SaveImageAttachment { data, media_type, name: Some("screen-annotation.jpg".into()) }).await {
                Ok(reference) => reference,
                Err(failure) => return error(StatusCode::BAD_REQUEST, "annotation-image-invalid", failure.to_string()),
            };
            content.push(dsh_llm::ContentBlock::Image { attachment: dsh_llm::ImageAttachmentRef {
                attachment_id: reference.attachment_id.to_string(),
                media_type: Some(reference.media_type.as_str().to_string()),
                bytes: Some(reference.bytes),
                width: Some(reference.width),
                height: Some(reference.height),
                name: reference.name,
            } });
        }
        agent.followup(dsh_llm::create_user_message(content, dsh_llm::MessageSource::User { rpc_id: None, client_time_zone: None }));
        return json_response(StatusCode::OK, &json!({"submitted":true,"ownerSessionId":owner.id,"annotation":annotations,"hasScreenshot":body.get("screenshot").is_some()}));
    }
    if operation != "action" {
        return error(
            StatusCode::NOT_FOUND,
            "route-not-found",
            "未知浏览器控制操作",
        );
    }
    let Some(runtime) = runtime else {
        return error(
            StatusCode::CONFLICT,
            "computer-use-disabled",
            "Computer Use 未启用，请先在设置中开启",
        );
    };
    let arguments = match adapter_arguments(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let signal: AbortPredicate = Arc::new(|| false);
    let result = if let Some(agent) = owner.agent {
        runtime.execute_for_human(agent, &arguments, signal).await
    } else {
        runtime
            .execute_for_human_session(owner.id, &arguments, signal)
            .await
    };
    match result {
        Ok(mut output) => {
            let object = output
                .value
                .as_object_mut()
                .expect("ComputerUseRuntime validates object outputs");
            if let Some(screenshot) = output.screenshot.take() {
                object.insert(
                    "screenshot".to_string(),
                    json!({
                        "base64": base64::engine::general_purpose::STANDARD.encode(screenshot.data),
                        "mediaType": screenshot.media_type,
                        "name": screenshot.name
                    }),
                );
            }
            json_response(StatusCode::OK, &output.value)
        }
        Err(failure) => {
            let status = match failure.code.as_str() {
                "COMPUTER_USE_ABORTED" | "COMPUTER_USE_TIMEOUT" => StatusCode::REQUEST_TIMEOUT,
                "COMPUTER_USE_SESSION_NOT_FOUND" => StatusCode::NOT_FOUND,
                "COMPUTER_USE_SESSION_LIMIT"
                | "COMPUTER_USE_MANUAL_CONTROL"
                | "COMPUTER_USE_DEVICE_BUSY" => StatusCode::CONFLICT,
                _ => StatusCode::BAD_REQUEST,
            };
            error(status, failure.code, failure.message)
        }
    }
}

pub fn register(
    web_server: &Arc<WebServer>,
    agents: Arc<AgentRegistry>,
    runtime: Option<Arc<ComputerUseRuntime>>,
    persistence: Option<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>,
    allow_remote_host: bool,
) -> RouteDisposer {
    let streams = Arc::new(super::computer_use_stream::Streams::default());
    let video_agents = agents.clone();
    let video_runtime = runtime.clone();
    let video_persistence = persistence.clone();
    let video = web_server.register_upgrade(dsh_host_webserver::WebUpgradeRoute {
        path: format!("{ROUTE}/stream"),
        handler: Arc::new(move |request, socket| {
            let agents = video_agents.clone();
            let runtime = video_runtime.clone();
            let persistence = video_persistence.clone();
            let streams = streams.clone();
            Box::pin(async move {
                super::computer_use_stream::upgrade(
                    request,
                    socket,
                    agents,
                    runtime,
                    persistence,
                    streams,
                    allow_remote_host,
                )
                .await;
                Ok(())
            })
        }),
    });
    let http = web_server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: ROUTE.to_string(),
        handler: Arc::new(move |request| {
            let agents = Arc::clone(&agents);
            let runtime = runtime.clone();
            let persistence = persistence.clone();
            Box::pin(async move {
                Ok::<_, WebHandlerError>(
                    handle(request, agents, runtime, persistence, allow_remote_host).await,
                )
            })
        }),
    });
    Arc::new(move || {
        video();
        http();
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_arguments_bind_browser_session_without_exposing_owner_override() {
        let value = adapter_arguments(&json!({
            "ownerSessionId":"conversation-a",
            "browserSessionId":"browser-a",
            "action":"navigate",
            "url":"https://example.com"
        }))
        .unwrap();
        assert!(value.get("ownerSessionId").is_none());
        assert!(value.get("browserSessionId").is_none());
        assert_eq!(value["sessionId"], "browser-a");
    }

    #[test]
    fn omitted_gui_browser_session_uses_the_model_default() {
        let value = adapter_arguments(&json!({
            "ownerSessionId":"conversation-a",
            "action":"capture"
        }))
        .unwrap();
        assert_eq!(value["sessionId"], "default");
    }
}
