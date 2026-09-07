//! Same-origin binary H.264 transport, separate from interactive input requests.
use super::computer_use_http::valid_owner;
use axum::extract::Query;
use base64::Engine;
use dsh_host_webserver::{WebRequest, WebUpgraded};
use dsh_tool_computer_use_command::ComputerUseRuntime;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};
type WebSocket = WebSocketStream<WebUpgraded>;

#[derive(Default)]
pub(super) struct Streams {
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
}
struct Lease {
    key: String,
    abort: Arc<AtomicBool>,
    streams: Arc<Streams>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.abort.store(true, Ordering::SeqCst);
        let mut active = self
            .streams
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if active
            .get(&self.key)
            .is_some_and(|v| Arc::ptr_eq(v, &self.abort))
        {
            active.remove(&self.key);
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Parameters {
    owner_session_id: String,
    #[serde(default)]
    browser_session_id: Option<String>,
}
pub(super) async fn upgrade(
    request: WebRequest,
    upgraded: WebUpgraded,
    agents: Arc<dsh_agent::AgentRegistry>,
    runtime: Option<Arc<ComputerUseRuntime>>,
    persistence: Option<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>,
    streams: Arc<Streams>,
    allow_remote_host: bool,
) {
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(2048))
        .max_frame_size(Some(2048));
    let mut socket = WebSocket::from_raw_socket(
        upgraded,
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        Some(config),
    )
    .await;
    if !super::trusted_web_request(&request, allow_remote_host) {
        reject(&mut socket, "forbidden", "视频请求来源不可信").await;
        return;
    }
    if request.uri().query().is_some_and(|q| q.len() > 2048) {
        reject(&mut socket, "invalid-stream", "视频请求参数过长").await;
        return;
    }
    let parameters = match Query::<Parameters>::try_from_uri(request.uri()) {
        Ok(v) => v.0,
        Err(_) => {
            reject(&mut socket, "invalid-stream", "视频请求参数无效").await;
            return;
        }
    };
    let session = parameters
        .browser_session_id
        .unwrap_or_else(|| "default".into());
    if session.is_empty() || session.len() > 200 || session.chars().any(char::is_control) {
        reject(&mut socket, "invalid-stream", "控制会话无效").await;
        return;
    }
    let owner = match valid_owner(
        &agents,
        &persistence,
        &runtime,
        &json!({"ownerSessionId":parameters.owner_session_id}),
    )
    .await
    {
        Ok(v) => v,
        Err(_) => {
            reject(&mut socket, "invalid-owner", "控制会话不存在或不可用").await;
            return;
        }
    };
    let Some(runtime) = runtime else {
        reject(&mut socket, "computer-use-disabled", "Computer Use 未启用").await;
        return;
    };
    if runtime.adapter_id() != "uu-desktop" {
        reject(
            &mut socket,
            "video-unavailable",
            "当前执行器不支持此视频通道",
        )
        .await;
        return;
    }
    let abort = Arc::new(AtomicBool::new(false));
    let key = format!("{}\0{}", owner.id, session);
    {
        let mut active = streams.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.len() >= 4 && !active.contains_key(&key) {
            return;
        }
        if let Some(previous) = active.insert(key.clone(), abort.clone()) {
            previous.store(true, Ordering::SeqCst);
        }
    }
    let lease = Lease {
        key,
        abort,
        streams,
    };
    serve(socket, runtime, owner.id, session, lease).await;
}
async fn reject(socket: &mut WebSocket, code: &str, message: &str) {
    let _ = send(
        socket,
        Message::Text(
            json!({"kind":"error","code":code,"message":message})
                .to_string()
                .into(),
        ),
    )
    .await;
    let _ = send(
        socket,
        Message::Close(Some(CloseFrame {
            code: CloseCode::Policy,
            reason: code.to_string().into(),
        })),
    )
    .await;
}
async fn send(socket: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(2), socket.send(message)).await,
        Ok(Ok(()))
    )
}
async fn serve(
    mut socket: WebSocket,
    runtime: Arc<ComputerUseRuntime>,
    owner: String,
    session: String,
    lease: Lease,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(33));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut keyframe = true;
    let mut last_meta = Value::Null;
    let mut failures = 0;
    loop {
        if lease.abort.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {_ = interval.tick()=>{},message=socket.next()=>{match message{Some(Ok(Message::Text(text)))if text=="keyframe"=>keyframe=true,Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,_=>{}}continue}}
        let signal = lease.abort.clone();
        let args = json!({"action":"video_frame","sessionId":session,"keyFrame":keyframe,"includeScreenshot":false});
        let capture = runtime.execute_for_human_session(
            owner.clone(),
            &args,
            Arc::new(move || signal.load(Ordering::SeqCst)),
        );
        let result = tokio::select! {result=capture=>result,message=socket.next()=>{match message{Some(Ok(Message::Text(text)))if text=="keyframe"=>keyframe=true,Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,_=>{}}continue}};
        let value = match result {
            Ok(output) => {
                failures = 0;
                output.value
            }
            Err(error) if error.code == "COMPUTER_USE_CAPTURE_INTERRUPTED" => continue,
            Err(error) if error.code == "COMPUTER_USE_MANUAL_CONTROL" => {
                keyframe = true;
                continue;
            }
            Err(error) => {
                failures += 1;
                if failures < 20 && error.code == "COMPUTER_USE_FRAME_PENDING" {
                    continue;
                }
                let _ = send(
                    &mut socket,
                    Message::Text(
                        json!({"kind":"error","code":error.code,"message":error.message})
                            .to_string()
                            .into(),
                    ),
                )
                .await;
                break;
            }
        };
        let video = &value["video"];
        let Some(encoded) = video["data"].as_str() else {
            continue;
        };
        if encoded.len() > 4 * 1024 * 1024 {
            break;
        }
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
            break;
        };
        let is_key = video["key"] == true;
        if keyframe && !is_key {
            continue;
        }
        keyframe = false;
        let meta = json!({"kind":"state","state":value["state"],"control":value["control"],"codec":video["codec"],"width":video["width"],"height":video["height"]});
        if meta != last_meta {
            if !send(&mut socket, Message::Text(meta.to_string().into())).await {
                break;
            }
            last_meta = meta;
        }
        let mut packet = Vec::with_capacity(bytes.len() + 9);
        packet.push(u8::from(is_key));
        packet.extend_from_slice(&video["timestamp"].as_u64().unwrap_or(0).to_le_bytes());
        packet.extend_from_slice(&bytes);
        if !send(&mut socket, Message::Binary(packet.into())).await {
            break;
        }
    }
    if !lease.abort.load(Ordering::SeqCst) {
        if let Some(control_id) = last_meta
            .pointer("/state/controlId")
            .and_then(Value::as_str)
        {
            let args = json!({"action":"release_inputs","sessionId":session,"controlId":control_id,"includeScreenshot":false});
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                runtime.execute_for_human_session(owner, &args, Arc::new(|| false)),
            )
            .await;
        }
    }
}
