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
}
impl Lease {
    fn mode(&self) -> Value {
        json!({"mode":if self.manual.load(Ordering::SeqCst){"manual"}else{"agent"},"generation":self.revision.load(Ordering::SeqCst)})
    }
    fn change(&self, manual: bool) {
        if self.manual.swap(manual, Ordering::SeqCst) != manual {
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn cancel(&self) {
        self.manual.store(true, Ordering::SeqCst);
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
}
pub struct ControlledAdapter {
    inner: Arc<dyn ComputerUseAdapter>,
    leases: Mutex<HashMap<String, Arc<Lease>>>,
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
fn paused() -> AdapterError {
    AdapterError::new(
        "COMPUTER_USE_MANUAL_CONTROL",
        "用户正在接管；等待用户在控制面板交还智能体，期间不可获取画面或发送输入",
    )
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
                });
                leases.insert(scope.clone(), lease.clone());
                lease
            }
        };
        // A caller cannot smuggle a transport identity through arbitrary JSON.
        if let Some(object) = request.arguments.as_object_mut() {
            for key in ["controlOrigin", "human", "origin", "resumeAgent"] {
                object.remove(key);
            }
        }
        if matches!(request.action.as_str(), "takeover" | "resume_agent") {
            if request.origin != ControlOrigin::Human {
                return Err(AdapterError::new(
                    "COMPUTER_USE_HUMAN_REQUIRED",
                    "控制权只能由用户在控制面板切换",
                ));
            }
            lease.cancel();
            self.inner
                .change_control(&request, request.action == "takeover")
                .await?;
            lease.change(request.action == "takeover");
            return Ok(AdapterOutput::json(json!({"control":lease.mode()})));
        }
        if request.origin == ControlOrigin::Human && manual_input(&request.action) {
            lease.change(true);
        }
        if request.origin == ControlOrigin::Agent && lease.manual.load(Ordering::SeqCst) {
            if matches!(request.action.as_str(), "status" | "list_sessions") {
                return Ok(AdapterOutput::json(
                    json!({"control":lease.mode(),"paused":true}),
                ));
            }
            return Err(paused());
        }
        let closing = request.action == "close";
        if closing {
            lease.cancel()
        }
        let revision = lease.revision.load(Ordering::SeqCst);
        let lease_for_cancel = lease.clone();
        let user_signal = signal.clone();
        let action_signal: AbortPredicate = Arc::new(move || {
            user_signal() || lease_for_cancel.revision.load(Ordering::SeqCst) != revision
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
        if closing && result.is_ok() {
            self.leases.lock().remove(&scope);
        }
        if signal() {
            return Err(AdapterError::cancelled());
        }
        if lease.revision.load(Ordering::SeqCst) != revision {
            return Err(paused());
        }
        match result {
            Ok(mut output) => {
                if output
                    .value
                    .pointer("/control/mode")
                    .and_then(Value::as_str)
                    == Some("manual")
                {
                    lease.change(true);
                    if origin == ControlOrigin::Agent {
                        if matches!(action.as_str(), "status" | "list_sessions") {
                            return Ok(AdapterOutput::json(
                                json!({"control":lease.mode(),"paused":true}),
                            ));
                        }
                        return Err(paused());
                    }
                }
                if origin == ControlOrigin::Agent
                    && lease.manual.load(Ordering::SeqCst)
                    && action != "status"
                    && action != "list_sessions"
                    && action != "close"
                {
                    return Err(paused());
                }
                if let Some(value) = output.value.as_object_mut() {
                    value.insert("control".into(), lease.mode());
                }
                Ok(output)
            }
            Err(error) => {
                if error.code == "COMPUTER_USE_MANUAL_CONTROL" {
                    lease.change(true);
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
        for action in ["capture", "type", "start", "close"] {
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
}
