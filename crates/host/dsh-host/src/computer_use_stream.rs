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
        atomic::{AtomicBool, AtomicU64, Ordering},
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

fn cleanup_arguments(session: &str, generation: u64, metadata: &Value) -> Option<Value> {
    let control = metadata.pointer("/state/controlId")?.as_str()?;
    Some(
        json!({"action":"release_inputs","sessionId":session,"controlId":control,"expectedStreamGeneration":generation,"includeScreenshot":false}),
    )
}

#[derive(Default)]
pub(super) struct Streams {
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
    next_generation: AtomicU64,
}
impl Streams {
    fn acquire(self: &Arc<Self>, key: String) -> Option<Lease> {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.len() >= 4 && !active.contains_key(&key) {
            return None;
        }
        let generation = self
            .next_generation
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_add(1)
            })
            .ok()?
            + 1;
        let abort = Arc::new(AtomicBool::new(false));
        if let Some(previous) = active.insert(key.clone(), abort.clone()) {
            previous.store(true, Ordering::SeqCst);
        }
        Some(Lease {
            key,
            abort,
            generation,
            streams: self.clone(),
        })
    }
}
struct Lease {
    key: String,
    abort: Arc<AtomicBool>,
    streams: Arc<Streams>,
    generation: u64,
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
    let key = format!("{}\0{}", owner.id, session);
    let Some(lease) = streams.acquire(key) else {
        return;
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
async fn send<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    message: Message,
) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(2), socket.send(message)).await,
        Ok(Ok(()))
    )
}
async fn serve<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut socket: WebSocketStream<S>,
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
        let args = json!({"action":"video_frame","sessionId":session,"streamGeneration":lease.generation,"keyFrame":keyframe,"includeScreenshot":false});
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
        if let Some(args) = cleanup_arguments(&session, lease.generation, &last_meta) {
            let abort = lease.abort.clone();
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                runtime.execute_for_human_session(
                    owner,
                    &args,
                    Arc::new(move || abort.load(Ordering::SeqCst)),
                ),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_tool_computer_use_command::{
        AbortPredicate, AdapterError, AdapterOutput, AdapterRequest, ComputerUseAdapter,
    };
    #[test]
    fn disconnect_cleanup_carries_the_stream_lease_even_when_metadata_is_older_than_input() {
        assert!(cleanup_arguments("default", 7, &Value::Null).is_none());
        let args = cleanup_arguments("default", 7, &json!({"state":{"controlId":"old"}})).unwrap();
        assert_eq!(args["controlId"], "old");
        assert_eq!(args["expectedStreamGeneration"], 7);
        assert!(args.get("expectedInputGeneration").is_none());
        assert_eq!(args["action"], "release_inputs");
        assert_eq!(args["includeScreenshot"], false);
    }

    #[derive(Default)]
    struct Driver {
        calls: Mutex<Vec<Value>>,
        held: AtomicBool,
        manual: AtomicBool,
    }
    #[async_trait::async_trait]
    impl ComputerUseAdapter for Driver {
        fn adapter_id(&self) -> &'static str {
            "uu-desktop"
        }
        async fn execute(
            &self,
            request: AdapterRequest,
            signal: AbortPredicate,
        ) -> Result<AdapterOutput, AdapterError> {
            if signal() {
                return Err(AdapterError::cancelled());
            }
            self.calls.lock().unwrap().push(request.arguments);
            if request.action == "release_inputs" && self.manual.load(Ordering::SeqCst) {
                self.held.store(false, Ordering::SeqCst);
            }
            Ok(AdapterOutput::json(
                json!({"state":{"controlId":"socket-control","connected":true,"interactive":true},"control":{"mode":if self.manual.load(Ordering::SeqCst){"manual"}else{"agent"}},"video":{"data":"AQID","key":true,"timestamp":1,"width":2,"height":2,"codec":"avc1.42C028"}}),
            ))
        }
    }
    async fn fixture(driver: Arc<Driver>) -> (cordis::Context, Arc<ComputerUseRuntime>) {
        let ctx = cordis::Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
            .unwrap();
        dsh_tools::ToolRuntime::install(&ctx, dsh_tools::Config::default()).unwrap();
        let runtime = dsh_tool_computer_use_command::install_adapter(&ctx, 1000, driver).unwrap();
        (ctx, runtime)
    }
    async fn socket_pair() -> (
        WebSocketStream<tokio::io::DuplexStream>,
        WebSocketStream<tokio::io::DuplexStream>,
    ) {
        let (server, client) = tokio::io::duplex(65536);
        (
            WebSocketStream::from_raw_socket(
                server,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await,
            WebSocketStream::from_raw_socket(
                client,
                tokio_tungstenite::tungstenite::protocol::Role::Client,
                None,
            )
            .await,
        )
    }
    async fn receive_frame(socket: &mut WebSocketStream<tokio::io::DuplexStream>) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(message) = socket.next().await {
                if matches!(message.unwrap(), Message::Binary(_)) {
                    return;
                }
            }
            panic!("video socket closed before its first frame");
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn closing_a_real_websocket_releases_keys_pressed_after_its_last_metadata() {
        let driver = Arc::new(Driver::default());
        driver.manual.store(true, Ordering::SeqCst);
        let (_ctx, runtime) = fixture(driver.clone()).await;
        let streams = Arc::new(Streams::default());
        let lease = streams.acquire("owner\0default".into()).unwrap();
        let generation = lease.generation;
        let (server, mut client) = socket_pair().await;
        let task = tokio::spawn(serve(
            server,
            runtime,
            "owner".into(),
            "default".into(),
            lease,
        ));
        receive_frame(&mut client).await;
        driver.held.store(true, Ordering::SeqCst);
        client.close(None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert!(!driver.held.load(Ordering::SeqCst));
        let calls = driver.calls.lock().unwrap();
        let release = calls
            .iter()
            .find(|call| call["action"] == "release_inputs")
            .unwrap();
        assert_eq!(release["expectedStreamGeneration"], generation);
        assert!(release.get("expectedInputGeneration").is_none());
    }
    #[tokio::test]
    async fn replacing_a_real_websocket_retires_old_cleanup_and_keeps_passive_agent_input() {
        let driver = Arc::new(Driver::default());
        let (_ctx, runtime) = fixture(driver.clone()).await;
        let streams = Arc::new(Streams::default());
        let first = streams.acquire("owner\0default".into()).unwrap();
        let (server, mut old_client) = socket_pair().await;
        let old_task = tokio::spawn(serve(
            server,
            runtime.clone(),
            "owner".into(),
            "default".into(),
            first,
        ));
        receive_frame(&mut old_client).await;
        driver.held.store(true, Ordering::SeqCst);
        let second = streams.acquire("owner\0default".into()).unwrap();
        let generation = second.generation;
        let (server, mut current_client) = socket_pair().await;
        let task = tokio::spawn(serve(
            server,
            runtime,
            "owner".into(),
            "default".into(),
            second,
        ));
        receive_frame(&mut current_client).await;
        let _ = old_client.close(None).await;
        tokio::time::timeout(Duration::from_secs(3), old_task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            driver
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| call["action"] != "release_inputs")
        );
        current_client.close(None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            driver.held.load(Ordering::SeqCst),
            "passive viewing does not own the agent's pressed keys"
        );
        let calls = driver.calls.lock().unwrap();
        let cleanup: Vec<_> = calls
            .iter()
            .filter(|call| call["action"] == "release_inputs")
            .collect();
        assert_eq!(cleanup.len(), 1);
        assert_eq!(cleanup[0]["expectedStreamGeneration"], generation);
    }
}
