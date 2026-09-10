//! Control ownership and immediate human takeover across adapter transports.
use crate::adapter::{
    AbortPredicate, AdapterError, AdapterOutput, AdapterRequest, ComputerUseAdapter, ControlOrigin,
};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const MAX_CONTROL_SCOPES: usize = 64;
struct Lease {
    owner: String,
    manual: AtomicBool,
    revision: AtomicU64,
    cleanup_idle: AtomicBool,
    manual_diagnostics: Mutex<Option<Value>>,
}
impl Lease {
    fn mode(&self) -> Value {
        let manual = self.manual.load(Ordering::SeqCst);
        let mut value = json!({"mode":if manual {"manual"}else{"agent"},"generation":self.revision.load(Ordering::SeqCst)});
        if manual {
            value["pauseReason"] = json!(pause_reason(self.manual_diagnostics.lock().as_ref()));
        }
        value
    }
    fn change(&self, manual: bool) {
        if !manual {
            self.manual_diagnostics.lock().take();
        }
        if self.manual.swap(manual, Ordering::SeqCst) != manual {
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn cancel(&self) {
        self.manual.store(true, Ordering::SeqCst);
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
    fn record_manual(&self, trigger: &'static str, diagnostics: Option<&Value>) {
        let mut retained = self.manual_diagnostics.lock();
        if retained.is_none() {
            let mut summary = bounded_control_diagnostics(diagnostics);
            summary["trigger"] = json!(trigger);
            *retained = Some(summary);
        }
    }
}

/// Retain only control provenance, never desktop state or input contents.
fn bounded_control_diagnostics(diagnostics: Option<&Value>) -> Value {
    let mut summary = json!({});
    let Some(value) = diagnostics else {
        return summary;
    };
    for field in ["ownEvents", "physicalEvents", "foreignInjectedEvents"] {
        if let Some(count) = value.get(field).and_then(Value::as_u64) {
            summary[field] = json!(count);
        }
    }
    if let Some(mode) = value.get("modeChange").and_then(Value::as_str)
        && matches!(
            mode,
            "start-agent" | "start-human" | "gui-takeover" | "gui-resume-agent"
        )
    {
        summary["modeChange"] = json!(mode);
    }
    if let Some(reason) = value.get("pauseReason").and_then(Value::as_str)
        && matches!(
            reason,
            "start-human"
                | "gui-input"
                | "gui-takeover"
                | "resume-pending"
                | "escape-hotkey"
                | "reader-shutdown"
                | "release-failed"
                | "emergency-stop-unavailable"
        )
    {
        summary["pauseReason"] = json!(reason);
    }
    if let Some(takeover) = value.get("takeover").filter(|value| value.is_object()) {
        let mut event = json!({});
        if let Some(sequence) = takeover.get("sequence").and_then(Value::as_u64) {
            event["sequence"] = json!(sequence);
        }
        if let Some(device) = takeover.get("device").and_then(Value::as_str)
            && matches!(device, "mouse" | "keyboard")
        {
            event["device"] = json!(device);
        }
        if let Some(source) = takeover.get("source").and_then(Value::as_str)
            && matches!(source, "physical" | "injected")
        {
            event["source"] = json!(source);
        }
        if event.as_object().is_some_and(|value| !value.is_empty()) {
            summary["takeover"] = event;
        }
    }
    summary
}

fn error_control_diagnostics(error: &AdapterError) -> Option<Value> {
    let (_, encoded) = error.message.split_once("; controlDiagnostics=")?;
    (encoded.len() <= 4096)
        .then(|| serde_json::from_str(encoded).ok())
        .flatten()
}
pub struct ControlledAdapter {
    inner: Arc<dyn ComputerUseAdapter>,
    leases: Mutex<HashMap<String, Arc<Lease>>>,
}

struct LeaseUse<'a> {
    adapter: &'a ControlledAdapter,
    scope: &'a str,
    lease: &'a Arc<Lease>,
}
impl Drop for LeaseUse<'_> {
    fn drop(&mut self) {
        if !self.lease.cleanup_idle.load(Ordering::SeqCst) {
            return;
        }
        let mut leases = self.adapter.leases.lock();
        // The map and this request must be the last owners. A concurrent
        // start/capture retains its lease until its own completion guard runs.
        if Arc::strong_count(self.lease) == 2
            && leases
                .get(self.scope)
                .is_some_and(|current| Arc::ptr_eq(current, self.lease))
            && !self.adapter.inner.has_owner_activity(&self.lease.owner)
        {
            leases.remove(self.scope);
        }
    }
}
impl ControlledAdapter {
    pub fn new(inner: Arc<dyn ComputerUseAdapter>) -> Self {
        Self {
            inner,
            leases: Mutex::new(HashMap::new()),
        }
    }
    fn remove_owner(&self, owner: &str) {
        self.leases.lock().retain(|_, lease| {
            if lease.owner == owner {
                lease.record_manual("owner-closed", None);
                lease.change(true);
                false
            } else {
                true
            }
        });
    }
}
fn manual_input(action: &str) -> bool {
    crate::action_requires_approval(action)
        && !matches!(action, "start" | "close" | "release_inputs")
}

fn pause_reason(diagnostics: Option<&Value>) -> &'static str {
    let Some(diagnostics) = diagnostics else {
        return "unknown";
    };
    match diagnostics.get("pauseReason").and_then(Value::as_str) {
        Some("start-human") => return "start-human",
        Some("gui-input") => return "gui-input",
        Some("gui-takeover") => return "gui-takeover",
        Some("resume-pending") => return "resume-pending",
        Some("escape-hotkey") => return "escape-hotkey",
        Some("reader-shutdown") => return "reader-shutdown",
        Some("release-failed") => return "release-failed",
        Some("emergency-stop-unavailable") => return "emergency-stop-unavailable",
        _ => {}
    }
    match diagnostics.get("trigger").and_then(Value::as_str) {
        Some("gui-input") => return "gui-input",
        Some("gui-takeover") => return "gui-takeover",
        Some("gui-resume-pending") => return "resume-pending",
        Some("close" | "owner-closed" | "shutdown") => return "connection-closing",
        _ => {}
    }
    match diagnostics
        .pointer("/takeover/source")
        .and_then(Value::as_str)
    {
        Some("physical") => "local-physical-input",
        Some("injected") => "external-injected-input",
        _ => match diagnostics.get("modeChange").and_then(Value::as_str) {
            Some("start-human") => "start-human",
            Some("gui-takeover") => "gui-takeover",
            _ => "unknown",
        },
    }
}

fn paused(lease: &Lease) -> AdapterError {
    let diagnostics = lease.manual_diagnostics.lock();
    let reason = match pause_reason(diagnostics.as_ref()) {
        "local-physical-input" => "检测到本机键鼠输入，共享桌面的智能体控制已暂停",
        "external-injected-input" => "检测到其他程序注入键鼠输入，智能体控制已暂停",
        "escape-hotkey" => "已触发全局急停快捷键，智能体控制已暂停",
        "gui-input" => "控制画面收到人工输入，智能体控制已暂停",
        "gui-takeover" => "控制面板已切换为人工接管",
        "start-human" => "连接以人工控制模式启动",
        "resume-pending" => "控制权尚未交还智能体",
        "release-failed" => "键鼠释放未完成，智能体保持暂停",
        "emergency-stop-unavailable" => "全局急停快捷键不可用，智能体保持暂停",
        "reader-shutdown" | "connection-closing" => "控制连接正在关闭，智能体控制已停止",
        _ => "智能体控制已暂停，暂停来源尚未确认",
    };
    let mut message = format!("{reason}；等待用户在控制面板交还智能体，期间不可获取画面或发送输入");
    if let Some(diagnostics) = diagnostics.as_ref() {
        message.push_str("; controlDiagnostics=");
        message.push_str(&diagnostics.to_string());
    }
    AdapterError::new("COMPUTER_USE_MANUAL_CONTROL", message)
}

#[async_trait]
impl ComputerUseAdapter for ControlledAdapter {
    fn adapter_id(&self) -> &'static str {
        self.inner.adapter_id()
    }
    fn availability(&self) -> Result<(), AdapterError> {
        self.inner.availability()
    }
    fn control_scope(&self, request: &AdapterRequest) -> Result<String, AdapterError> {
        self.inner.control_scope(request)
    }
    async fn execute(
        &self,
        mut request: AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        if matches!(request.action.as_str(), "takeover" | "resume_agent")
            && request.origin != ControlOrigin::Human
        {
            return Err(AdapterError::new(
                "COMPUTER_USE_HUMAN_REQUIRED",
                "控制权只能由用户在控制面板切换",
            ));
        }
        let scope = self.inner.control_scope(&request)?;
        let owner = request.owner_id.as_deref().unwrap_or("host").to_string();
        let lease = {
            let mut leases = self.leases.lock();
            if let Some(lease) = leases.get(&scope) {
                if lease.owner != owner {
                    return Err(AdapterError::new(
                        "COMPUTER_USE_DEVICE_BUSY",
                        "此设备正由另一个会话使用，请先结束其控制会话",
                    ));
                }
                lease.clone()
            } else {
                if leases.len() >= MAX_CONTROL_SCOPES {
                    return Err(AdapterError::new(
                        "COMPUTER_USE_SESSION_LIMIT",
                        "控制会话数量已达上限，请关闭不用的会话",
                    ));
                }
                let lease = Arc::new(Lease {
                    owner,
                    manual: AtomicBool::new(false),
                    revision: AtomicU64::new(0),
                    cleanup_idle: AtomicBool::new(false),
                    manual_diagnostics: Mutex::new(None),
                });
                leases.insert(scope.clone(), lease.clone());
                lease
            }
        };
        let _lease_use = LeaseUse {
            adapter: self,
            scope: &scope,
            lease: &lease,
        };
        // A caller cannot smuggle a transport identity through arbitrary JSON.
        if let Some(object) = request.arguments.as_object_mut() {
            for key in ["controlOrigin", "human", "origin", "resumeAgent"] {
                object.remove(key);
            }
        }
        if matches!(request.action.as_str(), "takeover" | "resume_agent") {
            lease.record_manual(
                if request.action == "takeover" {
                    "gui-takeover"
                } else {
                    "gui-resume-pending"
                },
                None,
            );
            lease.cancel();
            // Also cover an outer timeout/cancellation dropping this future
            // while the worker handoff is still pending.
            lease.cleanup_idle.store(true, Ordering::SeqCst);
            if let Err(error) = self
                .inner
                .change_control(&request, request.action == "takeover")
                .await
            {
                if error.code == "COMPUTER_USE_EMERGENCY_STOP_UNAVAILABLE" {
                    *lease.manual_diagnostics.lock() = Some(json!({
                        "trigger":"gui-resume-failed",
                        "pauseReason":"emergency-stop-unavailable"
                    }));
                }
                return Err(error);
            }
            lease.change(request.action == "takeover");
            lease.cleanup_idle.store(false, Ordering::SeqCst);
            return Ok(AdapterOutput::json(json!({"control":lease.mode()})));
        }
        if request.origin == ControlOrigin::Human && manual_input(&request.action) {
            lease.record_manual("gui-input", None);
            lease.change(true);
        }
        let closing = request.action == "close";
        if request.origin == ControlOrigin::Agent && lease.manual.load(Ordering::SeqCst) && !closing
        {
            if matches!(request.action.as_str(), "status" | "list_sessions") {
                return Ok(AdapterOutput::json(
                    json!({"control":lease.mode(),"paused":true}),
                ));
            }
            return Err(paused(&lease));
        }
        if closing {
            lease.record_manual("close", None);
            lease.cancel()
        }
        let revision = lease.revision.load(Ordering::SeqCst);
        // A GUI viewer opening the existing control session is not agent
        // work. Another viewer's initial manual-mode publication must not
        // cancel its pending start. Explicit close still stops the transport.
        let viewing_start = request.origin == ControlOrigin::Human && request.action == "start";
        let lease_for_cancel = lease.clone();
        let user_signal = signal.clone();
        let action_signal: AbortPredicate = Arc::new(move || {
            user_signal()
                || (!viewing_start && lease_for_cancel.revision.load(Ordering::SeqCst) != revision)
        });
        let action = request.action.clone();
        let origin = request.origin;
        let result = self.inner.execute(request, action_signal.clone()).await;
        if matches!(action.as_str(), "status" | "list_sessions")
            && !self.inner.has_owner_activity(&lease.owner)
            && !lease.manual.load(Ordering::SeqCst)
        {
            self.leases.lock().remove(&scope);
        }
        let close_confirmed = closing
            && result.as_ref().is_ok_and(|output| {
                output.value.get("closed").and_then(Value::as_bool) == Some(true)
                    || !self.inner.has_owner_activity(&lease.owner)
            });
        if close_confirmed {
            let mut leases = self.leases.lock();
            if leases
                .get(&scope)
                .is_some_and(|current| Arc::ptr_eq(current, &lease))
            {
                leases.remove(&scope);
            }
        }
        if signal() {
            return Err(AdapterError::cancelled());
        }
        if lease.revision.load(Ordering::SeqCst) != revision && !closing && !viewing_start {
            return Err(paused(&lease));
        }
        match result {
            Ok(mut output) => {
                if closing && origin == ControlOrigin::Agent {
                    // Releasing this owner's transport is allowed while the
                    // human controls it, but never publishes a private frame.
                    let closed = output
                        .value
                        .get("closed")
                        .and_then(Value::as_bool)
                        .unwrap_or_else(|| !self.inner.has_owner_activity(&lease.owner));
                    return Ok(AdapterOutput::json(
                        json!({"closed":closed,"control":lease.mode()}),
                    ));
                }
                if output
                    .value
                    .pointer("/control/mode")
                    .and_then(Value::as_str)
                    == Some("manual")
                {
                    lease.record_manual(
                        if action == "status" {
                            "worker-status"
                        } else {
                            "worker-output"
                        },
                        output.value.pointer("/state/controlDiagnostics"),
                    );
                    lease.change(true);
                    if origin == ControlOrigin::Agent {
                        if matches!(action.as_str(), "status" | "list_sessions") {
                            return Ok(AdapterOutput::json(
                                json!({"control":lease.mode(),"paused":true}),
                            ));
                        }
                        return Err(paused(&lease));
                    }
                }
                if origin == ControlOrigin::Agent
                    && lease.manual.load(Ordering::SeqCst)
                    && action != "status"
                    && action != "list_sessions"
                    && action != "close"
                {
                    return Err(paused(&lease));
                }
                if let Some(value) = output.value.as_object_mut() {
                    value.insert("control".into(), lease.mode());
                }
                Ok(output)
            }
            Err(error) => {
                if error.code == "COMPUTER_USE_MANUAL_CONTROL" {
                    let diagnostics = error_control_diagnostics(&error);
                    lease.record_manual("worker-error", diagnostics.as_ref());
                    lease.change(true);
                    return Err(paused(&lease));
                }
                if error.code != "COMPUTER_USE_MANUAL_CONTROL"
                    && !self.inner.has_owner_activity(&lease.owner)
                {
                    self.leases.lock().remove(&scope);
                }
                Err(error)
            }
        }
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        for lease in self.leases.lock().drain().map(|(_, v)| v) {
            lease.record_manual("shutdown", None);
            lease.change(true)
        }
        self.inner.shutdown().await
    }
    async fn close_owner(&self, owner: &str) -> Result<(), AdapterError> {
        for lease in self
            .leases
            .lock()
            .values()
            .filter(|lease| lease.owner == owner)
        {
            lease.record_manual("owner-closed", None);
            lease.change(true)
        }
        let result = self.inner.close_owner(owner).await;
        if result.is_ok() {
            self.remove_owner(owner)
        }
        result
    }
    fn has_owner_activity(&self, owner: &str) -> bool {
        self.inner.has_owner_activity(owner)
    }
    fn mark_owner_active(&self, owner: &str) {
        self.inner.mark_owner_active(owner)
    }
    async fn reap_inactive(&self) -> Vec<String> {
        let owners = self.inner.reap_inactive().await;
        for owner in &owners {
            self.remove_owner(owner)
        }
        owners
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::AdapterScreenshot;
    use tokio::sync::Notify;
    struct Driver {
        calls: AtomicU64,
        entered: Notify,
        release: Notify,
    }
    #[async_trait]
    impl ComputerUseAdapter for Driver {
        fn adapter_id(&self) -> &'static str {
            "fixture-desktop"
        }
        fn control_scope(&self, _: &AdapterRequest) -> Result<String, AdapterError> {
            Ok("physical-device".into())
        }
        async fn execute(
            &self,
            request: AdapterRequest,
            signal: AbortPredicate,
        ) -> Result<AdapterOutput, AdapterError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if request.action == "slow_capture" {
                self.entered.notify_one();
                self.release.notified().await;
            }
            if signal() {
                return Err(AdapterError::cancelled());
            }
            Ok(AdapterOutput {
                value: json!({"state":{"text":"private content"}}),
                screenshot: Some(AdapterScreenshot {
                    data: vec![1, 2, 3],
                    media_type: "image/png".into(),
                    name: None,
                }),
            })
        }
    }
    fn request(action: &str, origin: ControlOrigin, owner: &str) -> AdapterRequest {
        AdapterRequest::from_arguments(&json!({"action":action}))
            .unwrap()
            .with_owner_id(owner)
            .with_origin(origin)
    }
    fn driver() -> Arc<Driver> {
        Arc::new(Driver {
            calls: AtomicU64::new(0),
            entered: Notify::new(),
            release: Notify::new(),
        })
    }

    #[tokio::test]
    async fn concurrent_human_starts_survive_the_first_successful_manual_mode_publication() {
        struct StartDriver {
            entered: Notify,
            release: tokio::sync::Semaphore,
            calls: AtomicU64,
        }
        #[async_trait]
        impl ComputerUseAdapter for StartDriver {
            fn adapter_id(&self) -> &'static str {
                "uu-desktop"
            }
            fn control_scope(&self, _: &AdapterRequest) -> Result<String, AdapterError> {
                Ok("device".into())
            }
            fn has_owner_activity(&self, _: &str) -> bool {
                true
            }
            async fn execute(
                &self,
                _: AdapterRequest,
                signal: AbortPredicate,
            ) -> Result<AdapterOutput, AdapterError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.entered.notify_one();
                self.release.acquire().await.unwrap().forget();
                if signal() {
                    return Err(AdapterError::cancelled());
                }
                Ok(AdapterOutput::json(
                    json!({"state":{"controlId":"ready-control","connected":true},"control":{"mode":"manual"}}),
                ))
            }
        }
        let driver = Arc::new(StartDriver {
            entered: Notify::new(),
            release: tokio::sync::Semaphore::new(0),
            calls: AtomicU64::new(0),
        });
        let adapter = Arc::new(ControlledAdapter::new(driver.clone()));
        let first_adapter = adapter.clone();
        let first = tokio::spawn(async move {
            first_adapter
                .execute(
                    request("start", ControlOrigin::Human, "owner"),
                    Arc::new(|| false),
                )
                .await
        });
        driver.entered.notified().await;
        let second_adapter = adapter.clone();
        let second = tokio::spawn(async move {
            second_adapter
                .execute(
                    request("start", ControlOrigin::Human, "owner"),
                    Arc::new(|| false),
                )
                .await
        });
        driver.entered.notified().await;
        driver.release.add_permits(1);
        assert!(first.await.unwrap().is_ok());
        driver.release.add_permits(1);
        assert!(
            second.await.unwrap().is_ok(),
            "a view's pending start is not an agent action to cancel on manual-mode publication"
        );
        assert_eq!(driver.calls.load(Ordering::SeqCst), 2);
    }

    struct DiagnosticDriver {
        calls: AtomicU64,
        manual: AtomicBool,
        fail: AtomicBool,
        emergency_stop_available: AtomicBool,
    }
    #[async_trait]
    impl ComputerUseAdapter for DiagnosticDriver {
        fn adapter_id(&self) -> &'static str {
            "diagnostic-desktop"
        }
        fn control_scope(&self, _: &AdapterRequest) -> Result<String, AdapterError> {
            Ok("physical-device".into())
        }
        fn has_owner_activity(&self, _: &str) -> bool {
            true
        }
        async fn change_control(
            &self,
            _: &AdapterRequest,
            manual: bool,
        ) -> Result<(), AdapterError> {
            if !manual && !self.emergency_stop_available.load(Ordering::SeqCst) {
                return Err(AdapterError::new(
                    "COMPUTER_USE_EMERGENCY_STOP_UNAVAILABLE",
                    "全局急停不可用",
                ));
            }
            self.manual.store(manual, Ordering::SeqCst);
            Ok(())
        }
        async fn execute(
            &self,
            _: AdapterRequest,
            _: AbortPredicate,
        ) -> Result<AdapterOutput, AdapterError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let diagnostic = json!({
                "ownEvents":45, "physicalEvents":1, "foreignInjectedEvents":0, "modeChange":"start-agent",
                "takeover":{"sequence":46,"device":"mouse","source":"physical","x":1234,"key":"SECRET_KEY","text":"PRIVATE_TEXT"},
                "title":"PRIVATE_TITLE", "lastEvent":{"text":"PRIVATE_TEXT"}, "input":"PRIVATE_INPUT"
            });
            if self.fail.load(Ordering::SeqCst) {
                return Err(AdapterError::new(
                    "COMPUTER_USE_MANUAL_CONTROL",
                    format!("COMPUTER_USE_MANUAL_CONTROL; controlDiagnostics={diagnostic}"),
                ));
            }
            Ok(AdapterOutput::json(json!({
                "control":{"mode":if self.manual.load(Ordering::SeqCst){"manual"}else{"agent"}},
                "state":{"controlDiagnostics":diagnostic,"text":"PRIVATE_STATE"}
            })))
        }
    }
    fn diagnostic_driver() -> Arc<DiagnosticDriver> {
        Arc::new(DiagnosticDriver {
            calls: AtomicU64::new(0),
            manual: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            emergency_stop_available: AtomicBool::new(true),
        })
    }
    fn error_diagnostic(error: &AdapterError) -> Value {
        serde_json::from_str(error.message.split_once("; controlDiagnostics=").unwrap().1).unwrap()
    }
    #[tokio::test]
    async fn gui_status_preserves_manual_provenance_without_sending_further_agent_input() {
        let driver = diagnostic_driver();
        let adapter = ControlledAdapter::new(driver.clone());
        let signal: AbortPredicate = Arc::new(|| false);
        adapter
            .execute(request("status", ControlOrigin::Human, "a"), signal.clone())
            .await
            .unwrap();
        adapter
            .execute(
                request("capture", ControlOrigin::Agent, "a"),
                signal.clone(),
            )
            .await
            .unwrap();
        driver.manual.store(true, Ordering::SeqCst);
        adapter
            .execute(request("status", ControlOrigin::Human, "a"), signal.clone())
            .await
            .unwrap();
        let calls = driver.calls.load(Ordering::SeqCst);
        let error = adapter
            .execute(
                request("focus_window", ControlOrigin::Agent, "a"),
                signal.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "COMPUTER_USE_MANUAL_CONTROL");
        assert_eq!(
            driver.calls.load(Ordering::SeqCst),
            calls,
            "paused input must not reach the worker"
        );
        assert_eq!(
            error_diagnostic(&error),
            json!({"trigger":"worker-status","ownEvents":45,"physicalEvents":1,"foreignInjectedEvents":0,"modeChange":"start-agent","takeover":{"sequence":46,"device":"mouse","source":"physical"}})
        );
        for private in ["PRIVATE", "SECRET", "1234", "key", "lastEvent"] {
            assert!(!error.message.contains(private));
        }
        let status = adapter
            .execute(request("status", ControlOrigin::Agent, "a"), signal.clone())
            .await
            .unwrap();
        assert!(status.value.get("state").is_none());
        assert!(status.screenshot.is_none());
        assert_eq!(
            status.value["control"]["pauseReason"],
            "local-physical-input"
        );
        assert_eq!(driver.calls.load(Ordering::SeqCst), calls);
        adapter
            .execute(
                request("resume_agent", ControlOrigin::Human, "a"),
                signal.clone(),
            )
            .await
            .unwrap();
        adapter
            .execute(
                request("capture", ControlOrigin::Agent, "a"),
                signal.clone(),
            )
            .await
            .unwrap();
        assert!(
            adapter
                .leases
                .lock()
                .values()
                .next()
                .unwrap()
                .manual_diagnostics
                .lock()
                .is_none()
        );
    }
    #[tokio::test]
    async fn worker_error_keeps_only_bounded_control_provenance() {
        let driver = diagnostic_driver();
        driver.fail.store(true, Ordering::SeqCst);
        let adapter = ControlledAdapter::new(driver.clone());
        let signal: AbortPredicate = Arc::new(|| false);
        let first = adapter
            .execute(
                request("capture", ControlOrigin::Agent, "a"),
                signal.clone(),
            )
            .await
            .unwrap_err();
        let second = adapter
            .execute(request("key", ControlOrigin::Agent, "a"), signal)
            .await
            .unwrap_err();
        assert_eq!(driver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(error_diagnostic(&first), error_diagnostic(&second));
        assert_eq!(error_diagnostic(&second)["trigger"], "worker-error");
        assert!(!second.message.contains("PRIVATE"));
        assert!(!second.message.contains("SECRET"));
        assert!(second.message.len() < 1024);
    }
    #[test]
    fn diagnostic_whitelist_rejects_arbitrary_strings_and_unbounded_error_payloads() {
        let data = json!({"ownEvents":"PRIVATE", "physicalEvents":-1, "modeChange":"PRIVATE", "takeover":{"sequence":"PRIVATE", "device":"PRIVATE", "source":"PRIVATE", "text":"x".repeat(10000)}});
        assert_eq!(bounded_control_diagnostics(Some(&data)), json!({}));
        assert!(
            error_control_diagnostics(&AdapterError::new(
                "COMPUTER_USE_MANUAL_CONTROL",
                format!("error; controlDiagnostics={data}")
            ))
            .is_none()
        );
    }

    #[tokio::test]
    async fn failed_emergency_stop_registration_keeps_manual_control_and_reports_the_reason() {
        let driver = diagnostic_driver();
        let adapter = ControlledAdapter::new(driver.clone());
        let signal: AbortPredicate = Arc::new(|| false);
        adapter
            .execute(
                request("takeover", ControlOrigin::Human, "a"),
                signal.clone(),
            )
            .await
            .unwrap();
        driver
            .emergency_stop_available
            .store(false, Ordering::SeqCst);
        assert_eq!(
            adapter
                .execute(
                    request("resume_agent", ControlOrigin::Human, "a"),
                    signal.clone()
                )
                .await
                .unwrap_err()
                .code,
            "COMPUTER_USE_EMERGENCY_STOP_UNAVAILABLE"
        );
        let status = adapter
            .execute(request("status", ControlOrigin::Agent, "a"), signal.clone())
            .await
            .unwrap();
        assert_eq!(status.value["control"]["mode"], "manual");
        assert_eq!(
            status.value["control"]["pauseReason"],
            "emergency-stop-unavailable"
        );
        let error = adapter
            .execute(request("key", ControlOrigin::Agent, "a"), signal)
            .await
            .unwrap_err();
        assert!(error.message.contains("全局急停快捷键不可用"));
        assert_eq!(driver.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn pause_causes_distinguish_external_input_from_a_human_claim() {
        assert_eq!(
            pause_reason(Some(&json!({"takeover":{"source":"injected"}}))),
            "external-injected-input"
        );
        assert_eq!(
            pause_reason(Some(&json!({"pauseReason":"escape-hotkey"}))),
            "escape-hotkey"
        );
        assert_eq!(
            pause_reason(Some(&json!({"trigger":"gui-input"}))),
            "gui-input"
        );
        assert_eq!(
            pause_reason(Some(&json!({"pauseReason":"PRIVATE"}))),
            "unknown"
        );
        assert_eq!(pause_reason(None), "unknown");
    }
    #[tokio::test]
    async fn manual_control_is_authoritative_and_private() {
        let driver = driver();
        let adapter = ControlledAdapter::new(driver.clone());
        let signal: AbortPredicate = Arc::new(|| false);
        adapter
            .execute(
                request("takeover", ControlOrigin::Human, "a"),
                signal.clone(),
            )
            .await
            .unwrap();
        for action in ["capture", "type", "start"] {
            assert_eq!(
                adapter
                    .execute(request(action, ControlOrigin::Agent, "a"), signal.clone())
                    .await
                    .unwrap_err()
                    .code,
                "COMPUTER_USE_MANUAL_CONTROL"
            );
        }
        let status = adapter
            .execute(request("status", ControlOrigin::Agent, "a"), signal.clone())
            .await
            .unwrap();
        assert!(status.screenshot.is_none());
        assert!(status.value.get("state").is_none());
        assert_eq!(driver.calls.load(Ordering::SeqCst), 0);
        let mut forged = request("resume_agent", ControlOrigin::Agent, "a");
        forged.arguments["origin"] = json!("human");
        assert_eq!(
            adapter
                .execute(forged, signal.clone())
                .await
                .unwrap_err()
                .code,
            "COMPUTER_USE_HUMAN_REQUIRED"
        );
        adapter
            .execute(
                request("resume_agent", ControlOrigin::Human, "a"),
                signal.clone(),
            )
            .await
            .unwrap();
        adapter
            .execute(request("capture", ControlOrigin::Agent, "a"), signal)
            .await
            .unwrap();
        assert_eq!(driver.calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn takeover_invalidates_an_inflight_screenshot() {
        let driver = driver();
        let adapter = Arc::new(ControlledAdapter::new(driver.clone()));
        let run = adapter.clone();
        let pending = tokio::spawn(async move {
            run.execute(
                request("slow_capture", ControlOrigin::Agent, "a"),
                Arc::new(|| false),
            )
            .await
        });
        driver.entered.notified().await;
        adapter
            .execute(
                request("takeover", ControlOrigin::Human, "a"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
        driver.release.notify_one();
        assert_eq!(
            pending.await.unwrap().unwrap_err().code,
            "COMPUTER_USE_MANUAL_CONTROL"
        );
    }
    #[tokio::test]
    async fn agent_can_release_its_manual_transport_without_reading_private_state() {
        let driver = driver();
        let adapter = ControlledAdapter::new(driver.clone());
        adapter
            .execute(
                request("takeover", ControlOrigin::Human, "a"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
        assert_eq!(
            adapter
                .execute(
                    request("close", ControlOrigin::Agent, "b"),
                    Arc::new(|| false)
                )
                .await
                .unwrap_err()
                .code,
            "COMPUTER_USE_DEVICE_BUSY"
        );
        let closed = adapter
            .execute(
                request("close", ControlOrigin::Agent, "a"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
        assert_eq!(closed.value["closed"], true);
        assert!(closed.screenshot.is_none());
        assert!(closed.value.get("state").is_none());
        assert!(!closed.value.to_string().contains("private content"));
        assert_eq!(driver.calls.load(Ordering::SeqCst), 1);
        assert!(adapter.leases.lock().is_empty());
    }
    #[tokio::test]
    async fn incomplete_close_preserves_manual_control_and_owner_until_confirmed() {
        struct ClosingDriver {
            active: AtomicBool,
            confirm_close: AtomicBool,
        }
        #[async_trait]
        impl ComputerUseAdapter for ClosingDriver {
            fn adapter_id(&self) -> &'static str {
                "fixture-desktop"
            }
            fn control_scope(&self, _: &AdapterRequest) -> Result<String, AdapterError> {
                Ok("physical-device".into())
            }
            fn has_owner_activity(&self, _: &str) -> bool {
                self.active.load(Ordering::SeqCst)
            }
            async fn execute(
                &self,
                request: AdapterRequest,
                _: AbortPredicate,
            ) -> Result<AdapterOutput, AdapterError> {
                if request.action == "close" {
                    let closed = self.confirm_close.load(Ordering::SeqCst);
                    if closed {
                        self.active.store(false, Ordering::SeqCst);
                    }
                    return Ok(AdapterOutput::json(json!({"closed":closed})));
                }
                assert_eq!(
                    request.action, "start",
                    "blocked reads must never reach the driver"
                );
                self.active.store(true, Ordering::SeqCst);
                Ok(AdapterOutput::json(json!({"started":true})))
            }
        }
        let driver = Arc::new(ClosingDriver {
            active: AtomicBool::new(false),
            confirm_close: AtomicBool::new(false),
        });
        let adapter = ControlledAdapter::new(driver.clone());
        adapter
            .execute(
                request("start", ControlOrigin::Agent, "a"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
        adapter
            .execute(
                request("takeover", ControlOrigin::Human, "a"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
        let incomplete = adapter
            .execute(
                request("close", ControlOrigin::Agent, "a"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
        assert_eq!(incomplete.value["closed"], false);
        assert_eq!(incomplete.value["control"]["mode"], "manual");
        assert_eq!(
            adapter
                .execute(
                    request("capture", ControlOrigin::Agent, "a"),
                    Arc::new(|| false)
                )
                .await
                .unwrap_err()
                .code,
            "COMPUTER_USE_MANUAL_CONTROL"
        );
        assert_eq!(
            adapter
                .execute(
                    request("start", ControlOrigin::Agent, "b"),
                    Arc::new(|| false)
                )
                .await
                .unwrap_err()
                .code,
            "COMPUTER_USE_DEVICE_BUSY"
        );
        driver.confirm_close.store(true, Ordering::SeqCst);
        assert_eq!(
            adapter
                .execute(
                    request("close", ControlOrigin::Agent, "a"),
                    Arc::new(|| false)
                )
                .await
                .unwrap()
                .value["closed"],
            true
        );
        assert!(adapter.leases.lock().is_empty());
        adapter
            .execute(
                request("start", ControlOrigin::Agent, "b"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
    }
    #[test]
    fn uu_pause_diagnostics_accept_only_fixed_source_labels() {
        let value = bounded_control_diagnostics(Some(
            &json!({"pauseReason":"escape-hotkey","key":"PRIVATE_KEY","text":"PRIVATE_TEXT"}),
        ));
        assert_eq!(value, json!({"pauseReason":"escape-hotkey"}));
        assert_eq!(
            bounded_control_diagnostics(Some(&json!({"pauseReason":"PRIVATE_TEXT"}))),
            json!({})
        );
    }
    #[tokio::test]
    async fn two_owners_cannot_control_one_physical_desktop() {
        let adapter = ControlledAdapter::new(driver());
        adapter
            .execute(
                request("start", ControlOrigin::Agent, "a"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
        assert_eq!(
            adapter
                .execute(
                    request("start", ControlOrigin::Agent, "b"),
                    Arc::new(|| false)
                )
                .await
                .unwrap_err()
                .code,
            "COMPUTER_USE_DEVICE_BUSY"
        );
        adapter.close_owner("a").await.unwrap();
        adapter
            .execute(
                request("start", ControlOrigin::Agent, "b"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn reading_an_unopened_desktop_does_not_reserve_it() {
        let adapter = ControlledAdapter::new(driver());
        adapter
            .execute(
                request("list_sessions", ControlOrigin::Agent, "a"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
        adapter
            .execute(
                request("start", ControlOrigin::Agent, "b"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
    }

    struct MissingControl {
        active: AtomicBool,
        block_handoff: bool,
        entered: Notify,
        release: Notify,
    }

    #[async_trait]
    impl ComputerUseAdapter for MissingControl {
        fn adapter_id(&self) -> &'static str {
            "fixture-desktop"
        }
        fn control_scope(&self, _: &AdapterRequest) -> Result<String, AdapterError> {
            Ok("physical-device".into())
        }
        fn has_owner_activity(&self, _: &str) -> bool {
            self.active.load(Ordering::SeqCst)
        }
        async fn change_control(&self, _: &AdapterRequest, _: bool) -> Result<(), AdapterError> {
            if self.block_handoff {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Err(AdapterError::new(
                "COMPUTER_USE_SESSION_NOT_FOUND",
                "control worker unavailable",
            ))
        }
        async fn execute(
            &self,
            request: AdapterRequest,
            _: AbortPredicate,
        ) -> Result<AdapterOutput, AdapterError> {
            if request.action == "slow_capture" {
                self.entered.notify_one();
                self.release.notified().await;
            }
            if request.action == "start" {
                self.active.store(true, Ordering::SeqCst);
            }
            Ok(AdapterOutput::json(json!({"state":{}})))
        }
    }

    fn missing_control(active: bool) -> Arc<MissingControl> {
        Arc::new(MissingControl {
            active: AtomicBool::new(active),
            block_handoff: false,
            entered: Notify::new(),
            release: Notify::new(),
        })
    }

    #[tokio::test]
    async fn failed_handoff_without_a_worker_does_not_reserve_the_device() {
        for action in ["takeover", "resume_agent"] {
            let adapter = ControlledAdapter::new(missing_control(false));
            assert_eq!(
                adapter
                    .execute(
                        request(action, ControlOrigin::Human, "a"),
                        Arc::new(|| false)
                    )
                    .await
                    .unwrap_err()
                    .code,
                "COMPUTER_USE_SESSION_NOT_FOUND"
            );
            assert!(adapter.leases.lock().is_empty());
            adapter
                .execute(
                    request("start", ControlOrigin::Agent, "b"),
                    Arc::new(|| false),
                )
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn failed_handoff_keeps_an_active_worker_paused() {
        let adapter = ControlledAdapter::new(missing_control(true));
        assert!(
            adapter
                .execute(
                    request("resume_agent", ControlOrigin::Human, "a"),
                    Arc::new(|| false)
                )
                .await
                .is_err()
        );
        assert_eq!(
            adapter
                .execute(
                    request("capture", ControlOrigin::Agent, "a"),
                    Arc::new(|| false)
                )
                .await
                .unwrap_err()
                .code,
            "COMPUTER_USE_MANUAL_CONTROL"
        );
        assert_eq!(
            adapter
                .execute(
                    request("start", ControlOrigin::Agent, "b"),
                    Arc::new(|| false)
                )
                .await
                .unwrap_err()
                .code,
            "COMPUTER_USE_DEVICE_BUSY"
        );
    }

    #[tokio::test]
    async fn failed_idle_handoff_waits_for_an_inflight_request_to_finish() {
        let driver = missing_control(false);
        let adapter = Arc::new(ControlledAdapter::new(driver.clone()));
        let pending_adapter = adapter.clone();
        let pending = tokio::spawn(async move {
            pending_adapter
                .execute(
                    request("slow_capture", ControlOrigin::Agent, "a"),
                    Arc::new(|| false),
                )
                .await
        });
        driver.entered.notified().await;
        assert!(
            adapter
                .execute(
                    request("takeover", ControlOrigin::Human, "a"),
                    Arc::new(|| false)
                )
                .await
                .is_err()
        );
        assert_eq!(
            adapter.leases.lock().len(),
            1,
            "the inflight request still owns the old generation"
        );
        driver.release.notify_one();
        assert_eq!(
            pending.await.unwrap().unwrap_err().code,
            "COMPUTER_USE_MANUAL_CONTROL"
        );
        assert!(adapter.leases.lock().is_empty());
        adapter
            .execute(
                request("start", ControlOrigin::Agent, "b"),
                Arc::new(|| false),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn model_handoff_rejection_never_creates_a_lease() {
        let adapter = ControlledAdapter::new(missing_control(false));
        assert_eq!(
            adapter
                .execute(
                    request("resume_agent", ControlOrigin::Agent, "a"),
                    Arc::new(|| false)
                )
                .await
                .unwrap_err()
                .code,
            "COMPUTER_USE_HUMAN_REQUIRED"
        );
        assert!(adapter.leases.lock().is_empty());
    }

    #[tokio::test]
    async fn cancelled_handoff_without_a_worker_releases_its_lease() {
        let driver = Arc::new(MissingControl {
            active: AtomicBool::new(false),
            block_handoff: true,
            entered: Notify::new(),
            release: Notify::new(),
        });
        let adapter = Arc::new(ControlledAdapter::new(driver.clone()));
        let pending_adapter = adapter.clone();
        let pending = tokio::spawn(async move {
            pending_adapter
                .execute(
                    request("takeover", ControlOrigin::Human, "a"),
                    Arc::new(|| false),
                )
                .await
        });
        driver.entered.notified().await;
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        assert!(adapter.leases.lock().is_empty());
    }
}
