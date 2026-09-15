//! Human-only authorization browsers, isolated from agent browser sessions.
use base64::Engine;
use dsh_tool_computer_use_command::{
    AdapterRequest, ComputerUseAdapter, ControlOrigin, NativeBrowserAdapter, NativeBrowserConfig,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

struct LoginBrowser {
    adapter: Arc<dyn ComputerUseAdapter>,
    url: String,
    cancelled: Arc<AtomicBool>,
    gate: tokio::sync::Mutex<()>,
    expiry: parking_lot::Mutex<Option<tokio::task::AbortHandle>>,
}

pub(crate) struct AccountBrowsers {
    root: PathBuf,
    entries: Arc<parking_lot::Mutex<HashMap<String, Arc<LoginBrowser>>>>,
}
impl AccountBrowsers {
    pub async fn shutdown(&self) {
        let entries = std::mem::take(&mut *self.entries.lock());
        for entry in entries.into_values() {
            entry.cancelled.store(true, Ordering::SeqCst);
            if let Some(expiry) = entry.expiry.lock().take() {
                expiry.abort();
            }
            let _ = entry.adapter.shutdown().await;
        }
    }
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            entries: Default::default(),
        }
    }
    pub fn offer(&self, attempt: &str, url: &str, lifetime: u64) -> Result<(), String> {
        let parsed = reqwest::Url::parse(url).map_err(|_| "授权页面地址无效")?;
        if parsed.scheme() != "https"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("授权页面必须使用 HTTPS".into());
        }
        let adapter = NativeBrowserAdapter::new(NativeBrowserConfig {
            data_root: self.root.clone(),
            max_sessions: 1,
            viewport_width: 1100,
            viewport_height: 760,
            action_timeout: Duration::from_secs(25),
            ..Default::default()
        })
        .map_err(|e| e.message)?;
        let entry = Arc::new(LoginBrowser {
            adapter: Arc::new(adapter),
            url: url.into(),
            cancelled: Arc::new(AtomicBool::new(false)),
            gate: Default::default(),
            expiry: Default::default(),
        });
        self.entries.lock().insert(attempt.into(), entry.clone());
        let entries = Arc::downgrade(&self.entries);
        let id = attempt.to_string();
        let expiry = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(lifetime.min(900))).await;
            if let Some(entries) = entries.upgrade() {
                let entry = entries.lock().remove(&id);
                if let Some(entry) = entry {
                    entry.cancelled.store(true, Ordering::SeqCst);
                    let _ = entry.adapter.shutdown().await;
                }
            }
        });
        *entry.expiry.lock() = Some(expiry.abort_handle());
        Ok(())
    }
    pub async fn close(&self, attempt: &str) {
        let entry = self.entries.lock().remove(attempt);
        if let Some(entry) = entry {
            entry.cancelled.store(true, Ordering::SeqCst);
            if let Some(expiry) = entry.expiry.lock().take() {
                expiry.abort();
            }
            let _ = entry.adapter.shutdown().await;
        }
    }
    pub async fn action(&self, attempt: &str, body: &Value) -> Result<Value, String> {
        let entry = self
            .entries
            .lock()
            .get(attempt)
            .cloned()
            .ok_or("授权浏览器已结束，请重新登录")?;
        let action = body.get("action").and_then(Value::as_str).unwrap_or("");
        if action == "close" {
            self.close(attempt).await;
            return Ok(json!({"closed":true}));
        }
        let arguments = request_arguments(action, body, &entry.url)?;
        let _guard = entry.gate.lock().await;
        if entry.cancelled.load(Ordering::SeqCst) {
            return Err("授权浏览器已关闭".into());
        }
        let cancelled = entry.cancelled.clone();
        let output = entry
            .adapter
            .execute(
                AdapterRequest::from_arguments(&arguments)
                    .map_err(|e| e.message)?
                    .with_owner_id(attempt)
                    .with_origin(ControlOrigin::Human),
                Arc::new(move || cancelled.load(Ordering::SeqCst)),
            )
            .await
            .map_err(|e| format!("授权浏览器操作失败（{}）", e.code))?;
        let mut state = output.value.get("state").cloned().unwrap_or(Value::Null);
        if let Some(url) = state
            .get("url")
            .and_then(Value::as_str)
            .and_then(|s| reqwest::Url::parse(s).ok())
        {
            let mut url = url;
            url.set_query(None);
            url.set_fragment(None);
            state["url"] = json!(url.as_str());
        }
        if let Some(state) = state.as_object_mut() {
            state.remove("activeElement");
        }
        let screenshot = output.screenshot.map(|image| json!({"mediaType":image.media_type,"base64":base64::engine::general_purpose::STANDARD.encode(image.data)}));
        Ok(json!({"state":state,"screenshot":screenshot}))
    }
}
impl Drop for AccountBrowsers {
    fn drop(&mut self) {
        for entry in self.entries.lock().values() {
            entry.cancelled.store(true, Ordering::SeqCst);
            if let Some(expiry) = entry.expiry.lock().take() {
                expiry.abort();
            }
        }
    }
}

fn request_arguments(action: &str, body: &Value, url: &str) -> Result<Value, String> {
    if !matches!(
        action,
        "start" | "capture" | "click" | "type" | "key" | "scroll"
    ) {
        return Err("不支持的授权浏览器操作".into());
    }
    let mut args =
        json!({"action":action,"sessionId":"authorization","includeScreenshot":true,"waitMs":100});
    for field in ["x", "y", "text", "key", "deltaX", "deltaY"] {
        if let Some(value) = body.get(field) {
            args[field] = value.clone();
        }
    }
    if action == "start" {
        args["url"] = json!(url);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Adapter {
        stopped: AtomicBool,
    }
    #[async_trait::async_trait]
    impl ComputerUseAdapter for Adapter {
        fn adapter_id(&self) -> &'static str {
            "authorization-test"
        }
        async fn execute(
            &self,
            request: AdapterRequest,
            _signal: dsh_tool_computer_use_command::AbortPredicate,
        ) -> Result<
            dsh_tool_computer_use_command::AdapterOutput,
            dsh_tool_computer_use_command::AdapterError,
        > {
            assert_eq!(request.origin, ControlOrigin::Human);
            assert_eq!(request.owner_id.as_deref(), Some("login-a"));
            Ok(dsh_tool_computer_use_command::AdapterOutput::json(
                json!({"state":{"url":"https://example.com/callback?code=secret#private","activeElement":{"id":"private"},"viewport":{"width":1100,"height":760}}}),
            ))
        }
        async fn shutdown(&self) -> Result<(), dsh_tool_computer_use_command::AdapterError> {
            self.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    #[tokio::test]
    async fn cancel_drops_browser_state_and_authorization_urls_do_not_expose_codes() {
        let pool = AccountBrowsers::new(PathBuf::new());
        let adapter = Arc::new(Adapter {
            stopped: AtomicBool::new(false),
        });
        let entry = Arc::new(LoginBrowser {
            adapter: adapter.clone(),
            url: "https://example.com".into(),
            cancelled: Arc::new(AtomicBool::new(false)),
            gate: Default::default(),
            expiry: Default::default(),
        });
        let weak = Arc::downgrade(&entry);
        pool.entries.lock().insert("login-a".into(), entry);
        assert!(
            pool.action("login-b", &json!({"action":"capture"}))
                .await
                .is_err()
        );
        let view = pool
            .action("login-a", &json!({"action":"capture"}))
            .await
            .unwrap();
        assert_eq!(view["state"]["url"], "https://example.com/callback");
        assert!(view["state"].get("activeElement").is_none());
        pool.close("login-a").await;
        assert!(weak.upgrade().is_none());
        assert!(adapter.stopped.load(Ordering::SeqCst));
        assert!(
            pool.action("login-a", &json!({"action":"start"}))
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn unused_authorization_offers_expire_without_launching_a_browser() {
        let pool = AccountBrowsers::new(PathBuf::new());
        pool.offer("unused", "https://example.com", 0).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !pool.entries.lock().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("unused authorization offer must expire without browser startup");
        assert!(pool.entries.lock().is_empty());
    }
    #[test]
    fn authorization_page_and_owner_cannot_be_replaced_by_client_arguments() {
        let args=request_arguments("start",&json!({"url":"https://attacker.invalid","ownerId":"agent","sessionId":"shared","includeScreenshot":false}),"https://app.devin.ai/auth/cli/continue").unwrap();
        assert_eq!(args["url"], "https://app.devin.ai/auth/cli/continue");
        assert_eq!(args["sessionId"], "authorization");
        assert!(args.get("ownerId").is_none());
        assert!(request_arguments("navigate", &json!({}), "https://example.com").is_err());
    }
}
