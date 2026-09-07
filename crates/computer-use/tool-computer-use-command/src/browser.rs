use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex as SyncMutex;
use serde_json::{Value, json};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::adapter::{
    AbortPredicate, AdapterError, AdapterOutput, AdapterRequest, AdapterScreenshot,
    ComputerUseAdapter,
};

const DEFAULT_SESSION: &str = "default";
const DEVTOOLS_PORT_FILE: &str = "DevToolsActivePort";
const MAX_SCREENSHOT_BYTES: usize = 16 * 1024 * 1024;
const STATE_EXPRESSION: &str = r#"(() => {
  const active = document.activeElement;
  return {
    url: String(location.href).slice(0, 8192),
    title: String(document.title || "").slice(0, 4096),
    readyState: String(document.readyState || ""),
    scrollX: Number(window.scrollX || 0),
    scrollY: Number(window.scrollY || 0),
    viewport: {
      width: Number(window.innerWidth || 0),
      height: Number(window.innerHeight || 0),
      deviceScaleFactor: Number(window.devicePixelRatio || 1)
    },
    activeElement: active ? {
      tagName: String(active.tagName || "").toLowerCase(),
      id: String(active.id || "").slice(0, 1024),
      value: active.type !== "password" && typeof active.value === "string" ? active.value.slice(0, 4096) : null
    } : null
  };
})()"#;

#[derive(Debug, Clone)]
pub struct NativeBrowserConfig {
    pub executable: Option<PathBuf>,
    pub data_root: PathBuf,
    pub headless: bool,
    pub max_sessions: usize,
    pub launch_timeout: Duration,
    pub action_timeout: Duration,
    pub viewport_width: u32,
    pub viewport_height: u32,
}

impl Default for NativeBrowserConfig {
    fn default() -> Self {
        Self {
            executable: None,
            data_root: std::env::temp_dir()
                .join("deepseek-harness")
                .join("computer-use"),
            headless: true,
            max_sessions: 4,
            launch_timeout: Duration::from_secs(15),
            action_timeout: Duration::from_secs(60),
            viewport_width: 1280,
            viewport_height: 720,
        }
    }
}

pub struct NativeBrowserAdapter {
    executable: SyncMutex<Option<PathBuf>>,
    config: NativeBrowserConfig,
    http: reqwest::Client,
    sessions: Mutex<HashMap<(String, String), Arc<Mutex<BrowserSession>>>>,
    owner_activity: SyncMutex<HashMap<String, usize>>,
}

impl NativeBrowserAdapter {
    pub fn new(mut config: NativeBrowserConfig) -> Result<Self, AdapterError> {
        if config.max_sessions == 0 || config.max_sessions > 16 {
            return Err(AdapterError::new(
                "COMPUTER_USE_SESSION_LIMIT",
                "native browser max_sessions must be between 1 and 16",
            ));
        }
        if !(320..=3840).contains(&config.viewport_width)
            || !(240..=2160).contains(&config.viewport_height)
        {
            return Err(AdapterError::new(
                "COMPUTER_USE_VIEWPORT",
                "native browser viewport must be between 320x240 and 3840x2160",
            ));
        }
        if config.launch_timeout < Duration::from_secs(1)
            || config.action_timeout < Duration::from_secs(1)
        {
            return Err(AdapterError::new(
                "COMPUTER_USE_TIMEOUT",
                "native browser timeouts must be at least one second",
            ));
        }
        if !config.data_root.is_absolute() {
            config.data_root = std::env::current_dir()
                .map_err(|error| AdapterError::new("COMPUTER_USE_DATA_ROOT", error.to_string()))?
                .join(&config.data_root);
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|error| AdapterError::new("COMPUTER_USE_HTTP_CLIENT", error.to_string()))?;
        Ok(Self {
            executable: SyncMutex::new(None),
            config,
            http,
            sessions: Mutex::new(HashMap::new()),
            owner_activity: SyncMutex::new(HashMap::new()),
        })
    }

    fn prepare(&self) -> Result<PathBuf, AdapterError> {
        std::fs::create_dir_all(&self.config.data_root).map_err(|error| {
            AdapterError::new(
                "COMPUTER_USE_DATA_ROOT",
                format!(
                    "cannot create isolated browser data directory {}: {error}",
                    self.config.data_root.display()
                ),
            )
        })?;
        {
            let mut cached = self.executable.lock();
            if let Some(executable) = cached.as_ref()
                && executable.is_file()
            {
                return Ok(executable.clone());
            }
            cached.take();
        }
        let executable = discover_browser_executable(self.config.executable.as_deref())?;
        *self.executable.lock() = Some(executable.clone());
        Ok(executable)
    }

    async fn launch_session(
        &self,
        session_id: &str,
        signal: &AbortPredicate,
    ) -> Result<BrowserSession, AdapterError> {
        if signal() {
            return Err(AdapterError::cancelled());
        }
        let executable = self.prepare()?;
        let profile = self
            .config
            .data_root
            .join(format!("session-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&profile).await.map_err(|error| {
            AdapterError::new(
                "COMPUTER_USE_PROFILE_CREATE",
                format!("cannot create isolated browser profile: {error}"),
            )
        })?;

        let mut command = Command::new(&executable);
        command
            .arg("--remote-debugging-address=127.0.0.1")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-default-apps")
            .arg("--disable-extensions")
            .arg("--disable-component-update")
            .arg("--disable-background-networking")
            .arg("--remote-allow-origins=*")
            .arg("--force-device-scale-factor=1")
            .arg(format!(
                "--window-size={},{}",
                self.config.viewport_width, self.config.viewport_height
            ))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if self.config.headless {
            command.arg("--headless=new");
        }
        #[cfg(windows)]
        {
            command.creation_flags(0x0800_0000);
            // Edge otherwise relaunches and detaches from the owned process
            // when Windows supplies an application-compatibility layer. Keep
            // that layer intact and let the original process own the session.
            if executable
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("msedge.exe"))
            {
                command.arg("--edge-skip-compat-layer-relaunch");
            }
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&profile).await;
                return Err(AdapterError::new(
                    "COMPUTER_USE_BROWSER_LAUNCH",
                    format!("failed to start browser {}: {error}", executable.display()),
                ));
            }
        };
        let launch_started = Instant::now();
        let port_file = profile.join(DEVTOOLS_PORT_FILE);
        let page_websocket = loop {
            if signal() {
                let _ = child.kill().await;
                let _ = tokio::fs::remove_dir_all(&profile).await;
                return Err(AdapterError::cancelled());
            }
            if launch_started.elapsed() >= self.config.launch_timeout {
                let _ = child.kill().await;
                let _ = tokio::fs::remove_dir_all(&profile).await;
                return Err(AdapterError::new(
                    "COMPUTER_USE_BROWSER_START_TIMEOUT",
                    format!(
                        "browser did not expose DevTools within {} seconds",
                        self.config.launch_timeout.as_secs()
                    ),
                ));
            }
            if let Some(status) = child.try_wait().map_err(|error| {
                AdapterError::new("COMPUTER_USE_BROWSER_STATUS", error.to_string())
            })? {
                let _ = tokio::fs::remove_dir_all(&profile).await;
                return Err(AdapterError::new(
                    "COMPUTER_USE_BROWSER_EXITED",
                    format!("browser exited during startup with status {status}"),
                ));
            }
            if let Ok(text) = tokio::fs::read_to_string(&port_file).await
                && let Some(first_line) = text.lines().next()
                && let Ok(port) = first_line.trim().parse::<u16>()
                && let Ok(websocket) = self.find_page_websocket(port, signal).await
            {
                break websocket;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        };
        Ok(BrowserSession {
            id: session_id.to_string(),
            child,
            profile,
            page_websocket,
            action_timeout: self.config.action_timeout,
        })
    }

    async fn find_page_websocket(
        &self,
        port: u16,
        signal: &AbortPredicate,
    ) -> Result<String, AdapterError> {
        let url = format!("http://127.0.0.1:{port}/json/list");
        let response = run_cancellable(
            async {
                let response = self.http.get(url).send().await.map_err(|error| {
                    AdapterError::new("COMPUTER_USE_DEVTOOLS_HTTP", error.to_string())
                })?;
                response.json::<Value>().await.map_err(|error| {
                    AdapterError::new("COMPUTER_USE_DEVTOOLS_JSON", error.to_string())
                })
            },
            Arc::clone(signal),
            Duration::from_secs(3),
        )
        .await?;
        response
            .as_array()
            .into_iter()
            .flatten()
            .find(|target| target.get("type").and_then(Value::as_str) == Some("page"))
            .and_then(|target| target.get("webSocketDebuggerUrl"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                AdapterError::new(
                    "COMPUTER_USE_PAGE_TARGET",
                    "browser started but did not expose a page target",
                )
            })
    }

    async fn get_session(
        &self,
        owner_id: &str,
        session_id: &str,
    ) -> Result<Arc<Mutex<BrowserSession>>, AdapterError> {
        self.sessions
            .lock()
            .await
            .get(&(owner_id.to_string(), session_id.to_string()))
            .cloned()
            .ok_or_else(|| {
                AdapterError::new(
                    "COMPUTER_USE_SESSION_NOT_FOUND",
                    format!(
                        "browser session {session_id:?} is not running; call start or navigate first"
                    ),
                )
            })
    }

    async fn ensure_session(
        &self,
        owner_id: &str,
        session_id: &str,
        signal: &AbortPredicate,
    ) -> Result<(Arc<Mutex<BrowserSession>>, bool), AdapterError> {
        let mut sessions = self.sessions.lock().await;
        let key = (owner_id.to_string(), session_id.to_string());
        if let Some(existing) = sessions.get(&key) {
            return Ok((Arc::clone(existing), false));
        }
        if sessions.len() >= self.config.max_sessions {
            return Err(AdapterError::new(
                "COMPUTER_USE_SESSION_LIMIT",
                format!(
                    "native browser session limit ({}) reached; close a session before starting another",
                    self.config.max_sessions
                ),
            ));
        }
        let session = Arc::new(Mutex::new(self.launch_session(session_id, signal).await?));
        sessions.insert(key, Arc::clone(&session));
        *self
            .owner_activity
            .lock()
            .entry(owner_id.to_string())
            .or_default() += 1;
        Ok((session, true))
    }

    async fn close_session(&self, owner_id: &str, session_id: &str) -> Result<bool, AdapterError> {
        let session = self
            .sessions
            .lock()
            .await
            .remove(&(owner_id.to_string(), session_id.to_string()));
        let Some(session) = session else {
            return Ok(false);
        };
        self.decrement_owner_activity(owner_id);
        session.lock().await.close().await?;
        Ok(true)
    }

    fn decrement_owner_activity(&self, owner_id: &str) {
        let mut activity = self.owner_activity.lock();
        let remove_owner = if let Some(count) = activity.get_mut(owner_id) {
            *count = count.saturating_sub(1);
            *count == 0
        } else {
            false
        };
        if remove_owner {
            activity.remove(owner_id);
        }
    }

    async fn discard_session_if_matches(
        &self,
        owner_id: &str,
        session_id: &str,
        expected: &Arc<Mutex<BrowserSession>>,
    ) -> bool {
        let removed = {
            let mut sessions = self.sessions.lock().await;
            let key = (owner_id.to_string(), session_id.to_string());
            if sessions
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, expected))
            {
                sessions.remove(&key)
            } else {
                None
            }
        };
        if let Some(removed) = removed {
            self.decrement_owner_activity(owner_id);
            let _ = removed.lock().await.close().await;
            true
        } else {
            false
        }
    }

    async fn execute_browser_action(
        &self,
        request: AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        let session_id = session_id(&request.arguments)?;
        let owner_id = request.owner_id.as_deref().unwrap_or("host");
        let action = request.action.as_str();
        if action == "list_sessions" {
            let mut sessions = self
                .sessions
                .lock()
                .await
                .keys()
                .filter(|(owner, _)| owner == owner_id)
                .map(|(_, session)| session.clone())
                .collect::<Vec<_>>();
            sessions.sort();
            return Ok(AdapterOutput::json(json!({
                "ok": true,
                "adapter": "native-browser",
                "action": action,
                "sessions": sessions
            })));
        }
        if action == "close" {
            let closed = self.close_session(owner_id, &session_id).await?;
            return Ok(AdapterOutput::json(json!({
                "ok": true,
                "adapter": "native-browser",
                "action": action,
                "sessionId": session_id,
                "closed": closed
            })));
        }

        let (session, created) = match action {
            "start" | "navigate" => self.ensure_session(owner_id, &session_id, &signal).await?,
            _ => (self.get_session(owner_id, &session_id).await?, false),
        };
        let mut locked = session.lock().await;
        let outcome = locked.execute_action(&request, created, &signal).await;
        let exited = locked.ensure_alive().is_err();
        drop(locked);
        if exited {
            self.discard_session_if_matches(owner_id, &session_id, &session)
                .await;
        }
        outcome
    }
}

#[async_trait]
impl ComputerUseAdapter for NativeBrowserAdapter {
    fn adapter_id(&self) -> &'static str {
        "native-browser"
    }

    fn availability(&self) -> Result<(), AdapterError> {
        self.prepare().map(|_| ())
    }

    async fn execute(
        &self,
        request: AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        self.execute_browser_action(request, signal).await
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        let sessions = {
            let mut sessions = self.sessions.lock().await;
            std::mem::take(&mut *sessions)
        };
        self.owner_activity.lock().clear();
        let mut first_error = None;
        for session in sessions.into_values() {
            if let Err(error) = session.lock().await.close().await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn close_owner(&self, owner_id: &str) -> Result<(), AdapterError> {
        let sessions = {
            let mut sessions = self.sessions.lock().await;
            let keys = sessions
                .keys()
                .filter(|(owner, _)| owner == owner_id)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| sessions.remove(&key))
                .collect::<Vec<_>>()
        };
        self.owner_activity.lock().remove(owner_id);
        let mut first_error = None;
        for session in sessions {
            if let Err(error) = session.lock().await.close().await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn has_owner_activity(&self, owner_id: &str) -> bool {
        self.owner_activity
            .lock()
            .get(owner_id)
            .is_some_and(|count| *count > 0)
    }

    async fn reap_inactive(&self) -> Vec<String> {
        let candidates = self
            .sessions
            .lock()
            .await
            .iter()
            .map(|((owner, session_id), session)| {
                (owner.clone(), session_id.clone(), Arc::clone(session))
            })
            .collect::<Vec<_>>();
        let mut idle = Vec::new();
        for (owner_id, session_id, session) in candidates {
            let exited = match session.try_lock() {
                Ok(mut locked) => locked.ensure_alive().is_err(),
                Err(_) => false,
            };
            if exited
                && self
                    .discard_session_if_matches(&owner_id, &session_id, &session)
                    .await
                && !self.has_owner_activity(&owner_id)
            {
                idle.push(owner_id);
            }
        }
        idle.sort();
        idle.dedup();
        idle
    }
}

struct BrowserSession {
    id: String,
    child: Child,
    profile: PathBuf,
    page_websocket: String,
    action_timeout: Duration,
}

impl BrowserSession {
    fn ensure_alive(&mut self) -> Result<(), AdapterError> {
        match self.child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(status)) => Err(AdapterError::new(
                "COMPUTER_USE_BROWSER_EXITED",
                format!("browser session {:?} exited with status {status}", self.id),
            )),
            Err(error) => Err(AdapterError::new(
                "COMPUTER_USE_BROWSER_STATUS",
                error.to_string(),
            )),
        }
    }

    async fn execute_action(
        &mut self,
        request: &AdapterRequest,
        created: bool,
        signal: &AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        self.ensure_alive()?;
        let action = request.action.as_str();
        let session_id = session_id(&request.arguments)?;
        let mut result = json!({
            "ok": true,
            "adapter": "native-browser",
            "action": action,
            "sessionId": session_id,
            "created": created
        });

        match action {
            "start" => {
                if let Some(url) = request.arguments.get("url").and_then(Value::as_str) {
                    self.navigate(validate_navigation_url(url)?, signal).await?;
                    settle(signal, argument_wait_ms(&request.arguments, 250)?).await?;
                }
            }
            "status" | "cua_browser_state" | "capture" => {}
            "navigate" => {
                let url = required_string(&request.arguments, "url", 8_192)?;
                self.navigate(validate_navigation_url(url)?, signal).await?;
                settle(signal, argument_wait_ms(&request.arguments, 250)?).await?;
            }
            "click" | "double_click" => {
                let x = required_number(&request.arguments, "x", 0.0, 100_000.0)?;
                let y = required_number(&request.arguments, "y", 0.0, 100_000.0)?;
                let button = request
                    .arguments
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left");
                if !matches!(button, "left" | "right" | "middle" | "back" | "forward") {
                    return Err(AdapterError::new(
                        "COMPUTER_USE_INVALID_ARGUMENT",
                        "button must be left, right, middle, back or forward",
                    ));
                }
                self.click(x, y, button, action == "double_click", signal)
                    .await?;
                settle(signal, argument_wait_ms(&request.arguments, 100)?).await?;
            }
            "type" | "input" => {
                if let (Some(x), Some(y)) = (
                    request.arguments.get("x").and_then(Value::as_f64),
                    request.arguments.get("y").and_then(Value::as_f64),
                ) {
                    validate_number("x", x, 0.0, 100_000.0)?;
                    validate_number("y", y, 0.0, 100_000.0)?;
                    self.click(x, y, "left", false, signal).await?;
                }
                let text = required_string(&request.arguments, "text", 32_768)?;
                self.insert_text(text, signal).await?;
                settle(signal, argument_wait_ms(&request.arguments, 100)?).await?;
            }
            "scroll" => {
                let x = optional_number(&request.arguments, "x", 0.0, 100_000.0, 0.0)?;
                let y = optional_number(&request.arguments, "y", 0.0, 100_000.0, 0.0)?;
                let delta_x =
                    optional_number(&request.arguments, "deltaX", -100_000.0, 100_000.0, 0.0)?;
                let delta_y =
                    optional_number(&request.arguments, "deltaY", -100_000.0, 100_000.0, 0.0)?;
                self.scroll(x, y, delta_x, delta_y, signal).await?;
                settle(signal, argument_wait_ms(&request.arguments, 100)?).await?;
            }
            "key" | "keypress" => {
                let key = key_event(&request.arguments)?;
                let mut down = key.clone();
                down["type"] = json!("keyDown");
                let pressed = self.cdp("Input.dispatchKeyEvent", down, signal).await;
                let mut up = key;
                up["type"] = json!("keyUp");
                up.as_object_mut().unwrap().remove("text");
                let cleanup: AbortPredicate = Arc::new(|| false);
                let released = tokio::time::timeout(
                    Duration::from_secs(2),
                    self.cdp("Input.dispatchKeyEvent", up, &cleanup),
                )
                .await;
                pressed?;
                released.map_err(|_| {
                    AdapterError::new(
                        "COMPUTER_USE_TIMEOUT",
                        "releasing the browser key timed out",
                    )
                })??;
                settle(signal, argument_wait_ms(&request.arguments, 100)?).await?;
            }
            "drag" => {
                let x = required_number(&request.arguments, "x", 0.0, 100_000.0)?;
                let y = required_number(&request.arguments, "y", 0.0, 100_000.0)?;
                let end_x = required_number(&request.arguments, "endX", 0.0, 100_000.0)?;
                let end_y = required_number(&request.arguments, "endY", 0.0, 100_000.0)?;
                let pressed=self.cdp("Input.dispatchMouseEvent",json!({"type":"mousePressed","x":x,"y":y,"button":"left","buttons":1,"clickCount":1}),signal).await;
                let mut last = (x, y);
                let moved=async {
                    pressed?;
                    for step in 1..=12 {
                        let ratio=f64::from(step)/12.0;last=(x+(end_x-x)*ratio,y+(end_y-y)*ratio);
                        self.cdp("Input.dispatchMouseEvent",json!({"type":"mouseMoved","x":last.0,"y":last.1,"button":"left","buttons":1}),signal).await?;
                        settle(signal,16).await?;
                    }
                    Ok::<(),AdapterError>(())
                }.await;
                let cleanup: AbortPredicate = Arc::new(|| false);
                let released=tokio::time::timeout(Duration::from_secs(2),self.cdp("Input.dispatchMouseEvent",json!({"type":"mouseReleased","x":last.0,"y":last.1,"button":"left","buttons":0,"clickCount":1}),&cleanup)).await;
                moved?;
                released.map_err(|_| {
                    AdapterError::new(
                        "COMPUTER_USE_TIMEOUT",
                        "releasing the browser pointer timed out",
                    )
                })??;
            }
            other => {
                return Err(AdapterError::new(
                    "COMPUTER_USE_UNSUPPORTED_ACTION",
                    format!(
                        "native browser does not support action {other:?}; use start, status, capture, navigate, click, double_click, type, scroll, list_sessions or close"
                    ),
                ));
            }
        }

        result["state"] = self.state(signal).await?;
        let default_screenshot = matches!(
            action,
            "start"
                | "capture"
                | "navigate"
                | "click"
                | "double_click"
                | "type"
                | "input"
                | "scroll"
                | "key"
                | "keypress"
                | "drag"
        );
        let include_screenshot = request
            .arguments
            .get("includeScreenshot")
            .and_then(Value::as_bool)
            .unwrap_or(default_screenshot);
        let screenshot = if include_screenshot {
            Some(AdapterScreenshot {
                data: self.screenshot(signal).await?,
                media_type: "image/png".to_string(),
                name: Some(format!("computer-use-{session_id}.png")),
            })
        } else {
            None
        };
        Ok(AdapterOutput {
            value: result,
            screenshot,
        })
    }

    async fn close(&mut self) -> Result<(), AdapterError> {
        if self.child.try_wait().ok().flatten().is_none() {
            self.child.kill().await.map_err(|error| {
                AdapterError::new(
                    "COMPUTER_USE_BROWSER_CLOSE",
                    format!("failed to close browser session {:?}: {error}", self.id),
                )
            })?;
        }
        let _ = self.child.wait().await;
        for _ in 0..10 {
            match tokio::fs::remove_dir_all(&self.profile).await {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        Ok(())
    }

    async fn cdp(
        &self,
        method: &str,
        params: Value,
        signal: &AbortPredicate,
    ) -> Result<Value, AdapterError> {
        let websocket = self.page_websocket.clone();
        let method_name = method.to_string();
        run_cancellable(
            async move {
                let (mut stream, _) =
                    tokio_tungstenite::connect_async(&websocket)
                        .await
                        .map_err(|error| {
                            AdapterError::new(
                                "COMPUTER_USE_DEVTOOLS_CONNECT",
                                format!("cannot connect to browser DevTools: {error}"),
                            )
                        })?;
                stream
                    .send(Message::Text(
                        json!({"id": 1, "method": method_name, "params": params})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .map_err(|error| {
                        AdapterError::new("COMPUTER_USE_DEVTOOLS_SEND", error.to_string())
                    })?;
                while let Some(message) = stream.next().await {
                    let message = message.map_err(|error| {
                        AdapterError::new("COMPUTER_USE_DEVTOOLS_RECEIVE", error.to_string())
                    })?;
                    let Message::Text(text) = message else {
                        continue;
                    };
                    let value: Value = serde_json::from_str(&text).map_err(|error| {
                        AdapterError::new("COMPUTER_USE_DEVTOOLS_JSON", error.to_string())
                    })?;
                    if value.get("id").and_then(Value::as_u64) != Some(1) {
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        return Err(AdapterError::new(
                            "COMPUTER_USE_DEVTOOLS_PROTOCOL",
                            error.to_string(),
                        ));
                    }
                    return Ok(value.get("result").cloned().unwrap_or_else(|| json!({})));
                }
                Err(AdapterError::new(
                    "COMPUTER_USE_DEVTOOLS_CLOSED",
                    "browser DevTools connection closed before a response arrived",
                ))
            },
            Arc::clone(signal),
            self.action_timeout,
        )
        .await
    }

    async fn navigate(&self, url: &str, signal: &AbortPredicate) -> Result<(), AdapterError> {
        let result = self
            .cdp("Page.navigate", json!({"url": url}), signal)
            .await?;
        if let Some(error) = result.get("errorText").and_then(Value::as_str) {
            return Err(AdapterError::new(
                "COMPUTER_USE_NAVIGATION_FAILED",
                format!("browser navigation failed: {error}"),
            ));
        }
        Ok(())
    }

    async fn click(
        &self,
        x: f64,
        y: f64,
        button: &str,
        double: bool,
        signal: &AbortPredicate,
    ) -> Result<(), AdapterError> {
        let count = if double { 2 } else { 1 };
        for click_count in 1..=count {
            self.cdp(
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mousePressed",
                    "x": x,
                    "y": y,
                    "button": button,
                    "clickCount": click_count
                }),
                signal,
            )
            .await?;
            self.cdp(
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseReleased",
                    "x": x,
                    "y": y,
                    "button": button,
                    "clickCount": click_count
                }),
                signal,
            )
            .await?;
        }
        Ok(())
    }

    async fn insert_text(&self, text: &str, signal: &AbortPredicate) -> Result<(), AdapterError> {
        self.cdp("Input.insertText", json!({"text": text}), signal)
            .await?;
        Ok(())
    }

    async fn scroll(
        &self,
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        signal: &AbortPredicate,
    ) -> Result<(), AdapterError> {
        self.cdp(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseWheel",
                "x": x,
                "y": y,
                "deltaX": delta_x,
                "deltaY": delta_y
            }),
            signal,
        )
        .await?;
        Ok(())
    }

    async fn state(&self, signal: &AbortPredicate) -> Result<Value, AdapterError> {
        let result = self
            .cdp(
                "Runtime.evaluate",
                json!({
                    "expression": STATE_EXPRESSION,
                    "returnByValue": true,
                    "awaitPromise": true
                }),
                signal,
            )
            .await?;
        if let Some(exception) = result.get("exceptionDetails") {
            return Err(AdapterError::new(
                "COMPUTER_USE_STATE_FAILED",
                exception.to_string(),
            ));
        }
        result.pointer("/result/value").cloned().ok_or_else(|| {
            AdapterError::new(
                "COMPUTER_USE_STATE_FAILED",
                "browser did not return serializable page state",
            )
        })
    }

    async fn screenshot(&self, signal: &AbortPredicate) -> Result<Vec<u8>, AdapterError> {
        let result = self
            .cdp(
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "fromSurface": true,
                    "captureBeyondViewport": false
                }),
                signal,
            )
            .await?;
        let encoded = result.get("data").and_then(Value::as_str).ok_or_else(|| {
            AdapterError::new(
                "COMPUTER_USE_SCREENSHOT_MISSING",
                "browser screenshot response did not contain image data",
            )
        })?;
        decode_screenshot(encoded, MAX_SCREENSHOT_BYTES)
    }
}

fn key_event(arguments: &Value) -> Result<Value, AdapterError> {
    let names = arguments
        .get("keys")
        .and_then(Value::as_array)
        .filter(|keys| !keys.is_empty() && keys.len() <= 5)
        .ok_or_else(|| {
            AdapterError::new(
                "COMPUTER_USE_INVALID_ARGUMENT",
                "keys must contain a key with optional Control, Alt, Shift or Meta modifiers",
            )
        })?;
    let mut modifiers = 0;
    for name in &names[..names.len() - 1] {
        modifiers |= match name.as_str().unwrap_or("").to_ascii_lowercase().as_str() {
            "alt" => 1,
            "ctrl" | "control" => 2,
            "meta" | "command" => 4,
            "shift" => 8,
            _ => {
                return Err(AdapterError::new(
                    "COMPUTER_USE_INVALID_ARGUMENT",
                    "unknown keyboard modifier",
                ));
            }
        };
    }
    let name = names.last().and_then(Value::as_str).unwrap_or("");
    let (key, code, vk) = match name.to_ascii_lowercase().as_str() {
        "enter" => ("Enter".into(), "Enter".into(), 13),
        "tab" => ("Tab".into(), "Tab".into(), 9),
        "escape" | "esc" => ("Escape".into(), "Escape".into(), 27),
        "backspace" => ("Backspace".into(), "Backspace".into(), 8),
        "delete" => ("Delete".into(), "Delete".into(), 46),
        "space" => (" ".into(), "Space".into(), 32),
        "arrowleft" | "left" => ("ArrowLeft".into(), "ArrowLeft".into(), 37),
        "arrowup" | "up" => ("ArrowUp".into(), "ArrowUp".into(), 38),
        "arrowright" | "right" => ("ArrowRight".into(), "ArrowRight".into(), 39),
        "arrowdown" | "down" => ("ArrowDown".into(), "ArrowDown".into(), 40),
        "home" => ("Home".into(), "Home".into(), 36),
        "end" => ("End".into(), "End".into(), 35),
        "pageup" => ("PageUp".into(), "PageUp".into(), 33),
        "pagedown" => ("PageDown".into(), "PageDown".into(), 34),
        _ if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() => {
            let ch = name.as_bytes()[0];
            (
                name.to_string(),
                format!(
                    "{}{}",
                    if ch.is_ascii_digit() { "Digit" } else { "Key" },
                    (ch as char).to_ascii_uppercase()
                ),
                i32::from(ch.to_ascii_uppercase()),
            )
        }
        _ => {
            return Err(AdapterError::new(
                "COMPUTER_USE_INVALID_ARGUMENT",
                "unsupported browser key",
            ));
        }
    };
    let mut value = json!({"key":key,"code":code,"windowsVirtualKeyCode":vk,"nativeVirtualKeyCode":vk,"modifiers":modifiers});
    if modifiers & 7 == 0 {
        if key == "Enter" {
            value["text"] = json!("\r")
        } else if key.len() == 1 {
            value["text"] = json!(key)
        }
    }
    Ok(value)
}

fn decode_screenshot(encoded: &str, max_bytes: usize) -> Result<Vec<u8>, AdapterError> {
    let encoded_limit = (max_bytes * 4 / 3) + 8;
    if encoded.len() > encoded_limit {
        return Err(AdapterError::new(
            "COMPUTER_USE_SCREENSHOT_TOO_LARGE",
            format!("browser screenshot exceeds the {max_bytes}-byte limit"),
        ));
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| {
            AdapterError::new(
                "COMPUTER_USE_SCREENSHOT_ENCODING",
                format!("browser returned an invalid screenshot: {error}"),
            )
        })?;
    if data.len() > max_bytes {
        return Err(AdapterError::new(
            "COMPUTER_USE_SCREENSHOT_TOO_LARGE",
            format!("browser screenshot exceeds the {max_bytes}-byte limit"),
        ));
    }
    Ok(data)
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn run_cancellable<T>(
    future: impl Future<Output = Result<T, AdapterError>>,
    signal: AbortPredicate,
    timeout: Duration,
) -> Result<T, AdapterError> {
    let cancelled = async move {
        loop {
            if signal() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        result = future => result,
        _ = cancelled => Err(AdapterError::cancelled()),
        _ = tokio::time::sleep(timeout) => Err(AdapterError::new(
            "COMPUTER_USE_TIMEOUT",
            format!("computer-use action exceeded {} ms", timeout.as_millis()),
        )),
    }
}

async fn settle(signal: &AbortPredicate, wait_ms: u64) -> Result<(), AdapterError> {
    if wait_ms == 0 {
        return Ok(());
    }
    run_cancellable(
        async {
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            Ok(())
        },
        Arc::clone(signal),
        Duration::from_millis(wait_ms.saturating_add(500)),
    )
    .await
}

fn session_id(arguments: &Value) -> Result<String, AdapterError> {
    let value = arguments
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_SESSION);
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(AdapterError::new(
            "COMPUTER_USE_SESSION_ID",
            "sessionId must contain 1-64 ASCII letters, digits, dots, dashes or underscores",
        ));
    }
    Ok(value.to_string())
}

fn required_string<'a>(
    arguments: &'a Value,
    name: &str,
    max_len: usize,
) -> Result<&'a str, AdapterError> {
    let value = arguments.get(name).and_then(Value::as_str).ok_or_else(|| {
        AdapterError::new(
            "COMPUTER_USE_INVALID_ARGUMENT",
            format!("{name} must be a string"),
        )
    })?;
    if value.len() > max_len {
        return Err(AdapterError::new(
            "COMPUTER_USE_INVALID_ARGUMENT",
            format!("{name} exceeds the {max_len}-byte limit"),
        ));
    }
    Ok(value)
}

fn required_number(arguments: &Value, name: &str, min: f64, max: f64) -> Result<f64, AdapterError> {
    let value = arguments.get(name).and_then(Value::as_f64).ok_or_else(|| {
        AdapterError::new(
            "COMPUTER_USE_INVALID_ARGUMENT",
            format!("{name} must be a number"),
        )
    })?;
    validate_number(name, value, min, max)?;
    Ok(value)
}

fn optional_number(
    arguments: &Value,
    name: &str,
    min: f64,
    max: f64,
    default: f64,
) -> Result<f64, AdapterError> {
    match arguments.get(name) {
        None => Ok(default),
        Some(value) => {
            let value = value.as_f64().ok_or_else(|| {
                AdapterError::new(
                    "COMPUTER_USE_INVALID_ARGUMENT",
                    format!("{name} must be a number"),
                )
            })?;
            validate_number(name, value, min, max)?;
            Ok(value)
        }
    }
}

fn validate_number(name: &str, value: f64, min: f64, max: f64) -> Result<(), AdapterError> {
    if !value.is_finite() || value < min || value > max {
        return Err(AdapterError::new(
            "COMPUTER_USE_INVALID_ARGUMENT",
            format!("{name} must be between {min} and {max}"),
        ));
    }
    Ok(())
}

fn argument_wait_ms(arguments: &Value, default: u64) -> Result<u64, AdapterError> {
    match arguments.get("waitMs") {
        None => Ok(default),
        Some(value) => {
            let value = value.as_u64().ok_or_else(|| {
                AdapterError::new(
                    "COMPUTER_USE_INVALID_ARGUMENT",
                    "waitMs must be a non-negative integer",
                )
            })?;
            if value > 10_000 {
                return Err(AdapterError::new(
                    "COMPUTER_USE_INVALID_ARGUMENT",
                    "waitMs cannot exceed 10000",
                ));
            }
            Ok(value)
        }
    }
}

fn validate_navigation_url(value: &str) -> Result<&str, AdapterError> {
    if value == "about:blank" {
        return Ok(value);
    }
    let parsed = reqwest::Url::parse(value).map_err(|error| {
        AdapterError::new(
            "COMPUTER_USE_URL",
            format!("url must be an absolute HTTP or HTTPS address: {error}"),
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AdapterError::new(
            "COMPUTER_USE_URL_SCHEME",
            "native browser navigation only permits http, https and about:blank",
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AdapterError::new(
            "COMPUTER_USE_URL_CREDENTIALS",
            "credentials must not be embedded in a browser URL",
        ));
    }
    Ok(value)
}

pub fn discover_browser_executable(explicit: Option<&Path>) -> Result<PathBuf, AdapterError> {
    if let Some(explicit) = explicit {
        if explicit.is_file() {
            return Ok(explicit.to_path_buf());
        }
        return Err(AdapterError::new(
            "COMPUTER_USE_BROWSER_NOT_FOUND",
            format!(
                "configured browser executable does not exist: {}; select an Edge, Chrome or Chromium executable",
                explicit.display()
            ),
        ));
    }
    if let Some(path) = std::env::var_os("DSH_BROWSER_EXECUTABLE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    let mut candidates = Vec::<PathBuf>::new();
    #[cfg(windows)]
    {
        for root in [
            std::env::var_os("ProgramFiles(x86)"),
            std::env::var_os("ProgramFiles"),
            std::env::var_os("LOCALAPPDATA"),
        ]
        .into_iter()
        .flatten()
        {
            let root = PathBuf::from(root);
            candidates.push(root.join("Microsoft/Edge/Application/msedge.exe"));
            candidates.push(root.join("Google/Chrome/Application/chrome.exe"));
            candidates.push(root.join("Chromium/Application/chrome.exe"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from(
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ));
        candidates.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
        candidates.push(PathBuf::from(
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ));
    }
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let path_names: &[&str] = if cfg!(windows) {
        &["msedge.exe", "chrome.exe", "chromium.exe"]
    } else {
        &[
            "msedge",
            "microsoft-edge",
            "google-chrome",
            "chromium",
            "chromium-browser",
        ]
    };
    if let Some(search_path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&search_path) {
            for name in path_names {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(AdapterError::new(
        "COMPUTER_USE_BROWSER_NOT_FOUND",
        "no supported Edge, Chrome or Chromium executable was found; set the browser executable in Computer Use settings",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_cannot_escape_the_isolated_profile_root() {
        assert!(session_id(&json!({"sessionId":"work-1.alpha"})).is_ok());
        assert!(session_id(&json!({"sessionId":"../escape"})).is_err());
        assert!(session_id(&json!({"sessionId":"path\\escape"})).is_err());
    }

    #[test]
    fn navigation_rejects_local_file_and_credential_urls() {
        assert!(validate_navigation_url("https://example.com/path").is_ok());
        assert!(validate_navigation_url("about:blank").is_ok());
        assert!(validate_navigation_url("file:///etc/passwd").is_err());
        assert!(validate_navigation_url("https://user:secret@example.com").is_err());
    }

    #[test]
    fn explicit_missing_browser_has_actionable_error() {
        let error =
            discover_browser_executable(Some(Path::new("Z:/definitely-missing/browser.exe")))
                .unwrap_err();
        assert_eq!(error.code, "COMPUTER_USE_BROWSER_NOT_FOUND");
        assert!(error.message.contains("configured browser executable"));
    }

    #[test]
    fn construction_defers_missing_browser_failure_until_readiness_probe() {
        let root = std::env::temp_dir().join(format!(
            "dsh-computer-use-unavailable-{}",
            uuid::Uuid::new_v4()
        ));
        let missing = root.join("missing-browser.exe");
        let adapter = NativeBrowserAdapter::new(NativeBrowserConfig {
            executable: Some(missing),
            data_root: root.join("profiles"),
            ..NativeBrowserConfig::default()
        })
        .expect("invalid optional browser path must not prevent Host composition");
        assert!(!root.exists());
        let error = adapter.availability().unwrap_err();
        assert_eq!(error.code, "COMPUTER_USE_BROWSER_NOT_FOUND");
        assert!(error.message.contains("configured browser executable"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn page_state_and_screenshot_outputs_are_bounded_before_publication() {
        assert!(STATE_EXPRESSION.contains("String(location.href).slice(0, 8192)"));
        assert!(STATE_EXPRESSION.contains("String(document.title || \"\").slice(0, 4096)"));
        assert!(STATE_EXPRESSION.contains("String(active.id || \"\").slice(0, 1024)"));
        let valid = base64::engine::general_purpose::STANDARD.encode(b"12345678");
        assert_eq!(decode_screenshot(&valid, 8).unwrap(), b"12345678");
        let decoded_too_large = base64::engine::general_purpose::STANDARD.encode(b"123456789");
        assert_eq!(
            decode_screenshot(&decoded_too_large, 8).unwrap_err().code,
            "COMPUTER_USE_SCREENSHOT_TOO_LARGE"
        );
        assert_eq!(
            decode_screenshot(&"A".repeat((8 * 4 / 3) + 9), 8)
                .unwrap_err()
                .code,
            "COMPUTER_USE_SCREENSHOT_TOO_LARGE"
        );
    }

    #[tokio::test]
    async fn background_reaper_removes_exited_session_activity_and_profile() {
        let root =
            std::env::temp_dir().join(format!("dsh-computer-use-exit-{}", uuid::Uuid::new_v4()));
        let profile = root.join("profile");
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(profile.join("marker"), b"profile")
            .await
            .unwrap();
        let adapter = NativeBrowserAdapter::new(NativeBrowserConfig {
            executable: Some(std::env::current_exe().unwrap()),
            data_root: root.clone(),
            ..NativeBrowserConfig::default()
        })
        .unwrap();
        #[cfg(windows)]
        let mut child = Command::new("cmd")
            .args(["/D", "/S", "/C", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        #[cfg(not(windows))]
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let session = Arc::new(Mutex::new(BrowserSession {
            id: DEFAULT_SESSION.to_string(),
            child,
            profile: profile.clone(),
            page_websocket: "ws://127.0.0.1:1/unreachable".to_string(),
            action_timeout: Duration::from_secs(1),
        }));
        adapter.sessions.lock().await.insert(
            ("owner-a".to_string(), DEFAULT_SESSION.to_string()),
            session,
        );
        adapter
            .owner_activity
            .lock()
            .insert("owner-a".to_string(), 1);

        assert_eq!(adapter.reap_inactive().await, vec!["owner-a"]);
        assert!(!adapter.has_owner_activity("owner-a"));
        assert!(adapter.sessions.lock().await.is_empty());
        assert!(!profile.exists());
        let failure = adapter
            .execute(
                AdapterRequest::from_arguments(&json!({"action":"status"}))
                    .unwrap()
                    .with_owner_id("owner-a"),
                Arc::new(|| false),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, "COMPUTER_USE_SESSION_NOT_FOUND");
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
