//! Persistent, process-isolated native desktop transport. Graphics resources stay out of Host.
use crate::adapter::{
    AbortPredicate, AdapterError, AdapterOutput, AdapterRequest, AdapterScreenshot,
    ComputerUseAdapter, ControlOrigin,
};
use async_trait::async_trait;
use base64::Engine;
use parking_lot::Mutex as SyncMutex;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{Mutex, OnceCell, mpsc, oneshot},
};

const MAX_REPLY: usize = 4 * 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
#[derive(Clone)]
pub struct DesktopBinding {
    pub device_id: String,
    pub install_dir: Option<PathBuf>,
    pub account: Option<String>,
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    async fn fixture_slot(owner: &str, ready: bool) -> Arc<Slot> {
        fixture_rpc_slot(owner, ready).await.0
    }

    async fn fixture_rpc_slot(owner: &str, ready: bool) -> (Arc<Slot>, mpsc::Receiver<Vec<u8>>) {
        // An inert child supplies the real process cleanup path without
        // starting a desktop worker or accessing a user's desktop.
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .creation_flags(0x08000000);
        let child = command.spawn().unwrap();
        let (writer, received) = mpsc::channel(8);
        let (signals, _) = mpsc::channel(32);
        let old = Instant::now() - IDLE_TIMEOUT - Duration::from_secs(1);
        let client = Arc::new(Client {
            child: Arc::new(Mutex::new(child)),
            writer,
            signals,
            rpc: Arc::new(RpcState {
                pending: SyncMutex::new(HashMap::new()),
                alive: AtomicBool::new(true),
            }),
            serial: AtomicU64::new(1),
            last_used: SyncMutex::new(old),
        });
        let slot = Arc::new(Slot {
            owner: owner.into(),
            binding: DesktopBinding {
                device_id: "fixture-device".into(),
                install_dir: None,
                account: None,
            },
            client: OnceCell::new(),
            closed: AtomicBool::new(false),
            ready: AtomicBool::new(ready),
            control_id: SyncMutex::new(None),
            created: old,
        });
        assert!(slot.client.set(client).is_ok());
        (slot, received)
    }

    fn fixture_adapter(backend: Backend) -> DesktopAdapter {
        let mut adapter = DesktopAdapter::new(
            PathBuf::from("unused-worker"),
            PathBuf::new(),
            Arc::new(|| panic!("activity and cleanup must not connect a device")),
        );
        adapter.backend = backend;
        adapter
    }

    #[tokio::test]
    async fn running_activity_preserves_ready_desktops_until_they_are_idle() {
        for backend in [Backend::Native, Backend::Uu] {
            let adapter = fixture_adapter(backend);
            let slot = fixture_slot("owner", true).await;
            adapter.sessions.lock().insert("owner".into(), slot.clone());
            assert!(slot.inactive_at(Instant::now()));
            adapter.mark_owner_active("unknown");
            assert_eq!(adapter.sessions.lock().len(), 1);
            adapter.mark_owner_active("owner");
            assert!(adapter.reap_inactive().await.is_empty());
            assert!(adapter.has_owner_activity("owner"));
            assert!(!slot.inactive_at(Instant::now() + Duration::from_secs(105)));

            *slot.client.get().unwrap().last_used.lock() =
                Instant::now() - IDLE_TIMEOUT - Duration::from_secs(1);
            assert_eq!(adapter.reap_inactive().await, vec!["owner"]);
            assert!(slot.closed.load(Ordering::SeqCst));
            assert!(!adapter.has_owner_activity("owner"));
            adapter.mark_owner_active("owner");
            assert!(adapter.sessions.lock().is_empty());
        }
    }

    #[tokio::test]
    async fn activity_cannot_keep_starting_dead_or_closed_desktops_alive() {
        for state in ["starting", "dead", "closed"] {
            let adapter = fixture_adapter(Backend::Native);
            let slot = fixture_slot("owner", state != "starting").await;
            let client = slot.client.get().unwrap();
            // A startup request must not reset the independent startup limit.
            *client.last_used.lock() = Instant::now();
            if state == "dead" {
                client.rpc.alive.store(false, Ordering::SeqCst);
            } else if state == "closed" {
                slot.closed.store(true, Ordering::SeqCst);
            }
            let before = *client.last_used.lock();
            adapter.sessions.lock().insert("owner".into(), slot.clone());
            adapter.mark_owner_active("owner");
            assert_eq!(*client.last_used.lock(), before, "{state}");
            assert_eq!(adapter.reap_inactive().await, vec!["owner"], "{state}");
            assert!(adapter.sessions.lock().is_empty());
        }
    }

    #[tokio::test]
    async fn explicit_close_releases_an_active_desktop_without_reconnecting() {
        let adapter = fixture_adapter(Backend::Native);
        let slot = fixture_slot("owner", true).await;
        adapter.sessions.lock().insert("owner".into(), slot.clone());
        adapter.mark_owner_active("owner");
        adapter.close_owner("owner").await.unwrap();
        adapter.mark_owner_active("owner");
        assert!(slot.closed.load(Ordering::SeqCst));
        assert!(!slot.client.get().unwrap().rpc.alive.load(Ordering::SeqCst));
        assert!(adapter.sessions.lock().is_empty());
    }

    #[tokio::test]
    async fn failed_worker_launch_releases_device_and_close_does_not_launch() {
        let root = std::env::temp_dir().join(format!(
            "dsh-desktop-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let worker = root.join("invalid.exe");
        std::fs::write(&worker, b"not an executable").unwrap();
        let binding = DesktopBinding {
            device_id: "device".into(),
            install_dir: None,
            account: None,
        };
        let adapter =
            DesktopAdapter::new(worker, root.clone(), Arc::new(move || Ok(binding.clone())));
        let request = |action| {
            AdapterRequest::from_arguments(&json!({"action":action}))
                .unwrap()
                .with_owner_id("a")
        };
        let error = adapter
            .execute(request("start"), Arc::new(|| false))
            .await
            .unwrap_err();
        assert_eq!(error.code, "COMPUTER_USE_DESKTOP_START");
        assert!(!adapter.has_owner_activity("a"));
        assert!(adapter.sessions.lock().is_empty());
        assert_eq!(
            adapter
                .execute(request("close"), Arc::new(|| false))
                .await
                .unwrap()
                .value["closed"],
            true
        );
        let list = adapter
            .execute(request("list_sessions"), Arc::new(|| false))
            .await
            .unwrap();
        assert_eq!(list.value["sessions"], json!([]));
        std::fs::remove_file(root.join("invalid.exe")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn old_stream_release_never_reaches_a_starting_or_replaced_worker() {
        let adapter = Arc::new(fixture_adapter(Backend::Uu));
        let (slot, mut wire) = fixture_rpc_slot("owner", false).await;
        adapter.sessions.lock().insert("owner".into(), slot.clone());
        let request = |action: &str, control: Option<&str>| {
            let mut value = json!({"action":action});
            if let Some(control) = control {
                value["controlId"] = json!(control);
            }
            let mut request = AdapterRequest::from_arguments(&value)
                .unwrap()
                .with_owner_id("owner");
            request.origin = ControlOrigin::Human;
            request
        };
        let start_adapter = adapter.clone();
        let start_request = request("start", None);
        let start = tokio::spawn(async move {
            start_adapter
                .execute(start_request, Arc::new(|| false))
                .await
        });
        let command: Value = serde_json::from_slice(&wire.recv().await.unwrap()).unwrap();
        assert_eq!(command["arguments"]["action"], "start");
        for control in [None, Some("old-control")] {
            let error = adapter
                .execute(request("release_inputs", control), Arc::new(|| false))
                .await
                .unwrap_err();
            assert_eq!(error.code, "COMPUTER_USE_STALE_CONTROL");
            assert!(
                wire.try_recv().is_err(),
                "stale cleanup must not enter the worker queue during start"
            );
        }
        let client = slot.client.get().unwrap();
        client
            .rpc
            .pending
            .lock()
            .remove(&command["id"].as_u64().unwrap())
            .unwrap()
            .send(Ok(
                json!({"state":{"controlId":"new-control","connected":true,"interactive":true}}),
            ))
            .unwrap();
        assert_eq!(
            start.await.unwrap().unwrap().value["state"]["controlId"],
            "new-control"
        );
        let error = adapter
            .execute(
                request("release_inputs", Some("old-control")),
                Arc::new(|| false),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "COMPUTER_USE_STALE_CONTROL");
        assert!(
            wire.try_recv().is_err(),
            "an old controller identity must not preempt the new worker"
        );
        let release_adapter = adapter.clone();
        let release_request = request("release_inputs", Some("new-control"));
        let release = tokio::spawn(async move {
            release_adapter
                .execute(release_request, Arc::new(|| false))
                .await
        });
        let release_command: Value = serde_json::from_slice(&wire.recv().await.unwrap()).unwrap();
        assert_eq!(release_command["arguments"]["action"], "release_inputs");
        assert_eq!(release_command["origin"], "human");
        client
            .rpc
            .pending
            .lock()
            .remove(&release_command["id"].as_u64().unwrap())
            .unwrap()
            .send(Ok(
                json!({"state":{"controlId":"new-control","connected":true}}),
            ))
            .unwrap();
        assert!(
            release.await.unwrap().is_ok(),
            "the current view must still release held inputs through the real worker protocol"
        );
        adapter.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn explicit_close_cancels_pending_start_without_closing_a_subsequent_slot() {
        let adapter = Arc::new(fixture_adapter(Backend::Uu));
        let (slot, mut wire) = fixture_rpc_slot("owner", false).await;
        adapter.sessions.lock().insert("owner".into(), slot.clone());
        let request = |action| {
            AdapterRequest::from_arguments(&json!({"action":action}))
                .unwrap()
                .with_owner_id("owner")
                .with_origin(ControlOrigin::Human)
        };
        let start_adapter = adapter.clone();
        let start_request = request("start");
        let start = tokio::spawn(async move {
            start_adapter
                .execute(start_request, Arc::new(|| false))
                .await
        });
        let command: Value = serde_json::from_slice(&wire.recv().await.unwrap()).unwrap();
        let closed = adapter
            .execute(request("close"), Arc::new(|| false))
            .await
            .unwrap();
        assert_eq!(closed.value["closed"], true);
        assert!(slot.closed.load(Ordering::SeqCst));
        assert!(!slot.client.get().unwrap().rpc.alive.load(Ordering::SeqCst));
        let replacement = fixture_slot("owner", true).await;
        adapter
            .sessions
            .lock()
            .insert("owner".into(), replacement.clone());
        slot.client
            .get()
            .unwrap()
            .rpc
            .pending
            .lock()
            .remove(&command["id"].as_u64().unwrap())
            .unwrap()
            .send(Err(AdapterError::cancelled()))
            .unwrap();
        assert!(start.await.unwrap().is_err());
        assert!(
            adapter
                .sessions
                .lock()
                .get("owner")
                .is_some_and(|current| Arc::ptr_eq(current, &replacement))
        );
        assert!(
            !replacement.closed.load(Ordering::SeqCst),
            "late failure cleanup must stay scoped to its original worker"
        );
        adapter.shutdown().await.unwrap();
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Native,
    Uu,
}
impl Backend {
    fn id(self) -> &'static str {
        match self {
            Self::Native => "native-desktop",
            Self::Uu => "uu-desktop",
        }
    }
}
pub struct DesktopAdapter {
    backend: Backend,
    worker: PathBuf,
    data_root: PathBuf,
    binding: Arc<dyn Fn() -> Result<DesktopBinding, AdapterError> + Send + Sync>,
    sessions: SyncMutex<HashMap<String, Arc<Slot>>>,
}
struct Slot {
    owner: String,
    binding: DesktopBinding,
    client: OnceCell<Arc<Client>>,
    closed: AtomicBool,
    ready: AtomicBool,
    control_id: SyncMutex<Option<String>>,
    created: Instant,
}
impl Slot {
    fn inactive_at(&self, now: Instant) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return true;
        }
        let Some(client) = self.client.get() else {
            return now.saturating_duration_since(self.created) > IDLE_TIMEOUT;
        };
        if !client.rpc.alive.load(Ordering::SeqCst) {
            return true;
        }
        let last_used = if self.ready.load(Ordering::SeqCst) {
            *client.last_used.lock()
        } else {
            self.created
        };
        now.saturating_duration_since(last_used) > IDLE_TIMEOUT
    }
}
struct RpcState {
    pending: SyncMutex<HashMap<u64, oneshot::Sender<Result<Value, AdapterError>>>>,
    alive: AtomicBool,
}
struct Client {
    child: Arc<Mutex<Child>>,
    writer: mpsc::Sender<Vec<u8>>,
    signals: mpsc::Sender<Vec<u8>>,
    rpc: Arc<RpcState>,
    serial: AtomicU64,
    last_used: SyncMutex<Instant>,
}
fn failure(code: &str, message: &str) -> AdapterError {
    AdapterError::new(code, message)
}
struct PendingRequest {
    id: u64,
    rpc: Arc<RpcState>,
    signals: mpsc::Sender<Vec<u8>>,
    child: Arc<Mutex<Child>>,
    sent: bool,
    completed: bool,
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        self.rpc.pending.lock().remove(&self.id);
        if self.sent && !self.completed {
            let mut bytes = json!({"arguments":{"action":"cancel"},"cancelId":self.id})
                .to_string()
                .into_bytes();
            bytes.push(b'\n');
            if let Err(mpsc::error::TrySendError::Full(_)) = self.signals.try_send(bytes) {
                let child = self.child.clone();
                self.rpc.alive.store(false, Ordering::SeqCst);
                tokio::spawn(async move {
                    let _ = child.lock().await.kill().await;
                });
            }
        }
    }
}
impl Client {
    async fn spawn(
        worker: &PathBuf,
        root: &PathBuf,
        binding: &DesktopBinding,
        backend: Backend,
    ) -> Result<Arc<Self>, AdapterError> {
        tokio::fs::create_dir_all(root)
            .await
            .map_err(|_| failure("COMPUTER_USE_DESKTOP_START", "无法创建控制进程目录"))?;
        let mut command = Command::new(worker);
        if backend == Backend::Uu {
            command.env(
                "DSH_UU_INSTALL_DIR",
                binding
                    .install_dir
                    .as_ref()
                    .ok_or_else(|| failure("COMPUTER_USE_UNAVAILABLE", "未找到 UU 安装目录"))?,
            );
        }
        command
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|_| {
            failure(
                "COMPUTER_USE_DESKTOP_START",
                "无法启动独立控制进程，请检查安装包是否完整",
            )
        })?;
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let rpc = Arc::new(RpcState {
            pending: SyncMutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
        });
        let (writer, mut writes) = mpsc::channel::<Vec<u8>>(8);
        let (signals, mut notices) = mpsc::channel::<Vec<u8>>(32);
        let write_rpc = rpc.clone();
        tokio::spawn(async move {
            loop {
                let bytes = tokio::select! {biased;Some(bytes)=notices.recv()=>bytes,Some(bytes)=writes.recv()=>bytes,else=>break};
                if input.write_all(&bytes).await.is_err() || input.flush().await.is_err() {
                    break;
                }
            }
            write_rpc.alive.store(false, Ordering::SeqCst);
        });
        let read_rpc = rpc.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                let result = (&mut reader)
                    .take((MAX_REPLY + 1) as u64)
                    .read_until(b'\n', &mut line)
                    .await;
                match result {
                    Ok(0) | Err(_) => break,
                    _ => {}
                }
                if line.len() > MAX_REPLY {
                    break;
                }
                let prefix: &[u8] = match backend {
                    Backend::Native => b"DSH_DESKTOP_RESPONSE=",
                    Backend::Uu => b"DSH_UU_RESPONSE=",
                };
                if !line.starts_with(prefix) {
                    continue;
                }
                let Ok(value) = serde_json::from_slice::<Value>(&line[prefix.len()..]) else {
                    break;
                };
                let Some(id) = value["id"].as_u64() else {
                    continue;
                };
                let pending = read_rpc.pending.lock().remove(&id);
                if let Some(pending) = pending {
                    let result = if value["ok"] == true {
                        Ok(value["value"].clone())
                    } else {
                        Err(AdapterError::new(
                            value
                                .pointer("/error/code")
                                .and_then(Value::as_str)
                                .unwrap_or("COMPUTER_USE_DESKTOP_ERROR"),
                            value
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .unwrap_or("桌面控制操作失败"),
                        ))
                    };
                    let _ = pending.send(result);
                }
            }
            read_rpc.alive.store(false, Ordering::SeqCst);
            for sender in read_rpc.pending.lock().drain().map(|(_, v)| v) {
                let _ = sender.send(Err(failure(
                    "COMPUTER_USE_DESKTOP_DISCONNECTED",
                    "独立控制进程已退出，请重新连接设备",
                )));
            }
        });
        Ok(Arc::new(Self {
            child: Arc::new(Mutex::new(child)),
            writer,
            signals,
            rpc,
            serial: AtomicU64::new(1),
            last_used: SyncMutex::new(Instant::now()),
        }))
    }
    async fn request(
        &self,
        request: &AdapterRequest,
        binding: &DesktopBinding,
        signal: AbortPredicate,
    ) -> Result<Value, AdapterError> {
        if !self.rpc.alive.load(Ordering::SeqCst) {
            return Err(failure(
                "COMPUTER_USE_DESKTOP_DISCONNECTED",
                "设备控制连接已结束",
            ));
        }
        *self.last_used.lock() = Instant::now();
        let id = self.serial.fetch_add(1, Ordering::SeqCst);
        let wire = json!({"id":id,"ownerId":request.owner_id.as_deref().unwrap_or("host"),"origin":if request.origin==ControlOrigin::Human{"human"}else{"agent"},"binding":{"deviceId":binding.device_id,"account":binding.account},"arguments":request.arguments});
        let mut bytes = serde_json::to_vec(&wire)
            .map_err(|_| failure("COMPUTER_USE_INVALID_ARGUMENT", "无法编码控制命令"))?;
        if bytes.len() > 65535 {
            return Err(failure(
                "COMPUTER_USE_INVALID_ARGUMENT",
                "控制命令超过大小限制",
            ));
        }
        bytes.push(b'\n');
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.rpc.pending.lock();
            if pending.len() >= 8 {
                return Err(failure("COMPUTER_USE_BUSY", "正在处理控制操作，请稍后重试"));
            }
            pending.insert(id, tx);
        }
        let mut pending_guard = PendingRequest {
            id,
            rpc: self.rpc.clone(),
            signals: self.signals.clone(),
            child: self.child.clone(),
            sent: false,
            completed: false,
        };
        let result = async {
            self.writer
                .send(bytes)
                .await
                .map_err(|_| failure("COMPUTER_USE_DESKTOP_DISCONNECTED", "控制通道已断开"))?;
            pending_guard.sent = true;
            rx.await
                .map_err(|_| failure("COMPUTER_USE_DESKTOP_DISCONNECTED", "控制进程未返回结果"))?
        };
        let cancelled = async {
            loop {
                if signal() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await
            }
        };
        let result = tokio::select! {value=result=>value,_=cancelled=>Err(AdapterError::cancelled()),_=tokio::time::sleep(Duration::from_secs(45))=>Err(failure("COMPUTER_USE_TIMEOUT","设备控制操作超时"))};
        pending_guard.completed = !matches!(
            result.as_ref().err().map(|error| error.code.as_str()),
            Some("COMPUTER_USE_ABORTED" | "COMPUTER_USE_TIMEOUT")
        );
        result
    }
    async fn notice(&self, value: Value) {
        let mut bytes = value.to_string().into_bytes();
        bytes.push(b'\n');
        let _ = tokio::time::timeout(Duration::from_millis(200), self.signals.send(bytes)).await;
    }
    async fn stop(&self) {
        self.notice(json!({"arguments":{"action":"close"}})).await;
        self.rpc.alive.store(false, Ordering::SeqCst);
        let mut child = self.child.lock().await;
        if tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}
impl DesktopAdapter {
    pub fn new(
        worker: PathBuf,
        data_root: PathBuf,
        binding: Arc<dyn Fn() -> Result<DesktopBinding, AdapterError> + Send + Sync>,
    ) -> Self {
        Self {
            backend: Backend::Native,
            worker,
            data_root,
            binding,
            sessions: SyncMutex::new(HashMap::new()),
        }
    }
    pub fn for_uu(
        worker: PathBuf,
        data_root: PathBuf,
        binding: Arc<dyn Fn() -> Result<DesktopBinding, AdapterError> + Send + Sync>,
    ) -> Self {
        let mut adapter = Self::new(worker, data_root, binding);
        adapter.backend = Backend::Uu;
        adapter
    }
    fn key(request: &AdapterRequest) -> String {
        request.owner_id.as_deref().unwrap_or("host").to_string()
    }
    async fn slot(&self, request: &AdapterRequest) -> Result<Arc<Slot>, AdapterError> {
        let key = Self::key(request);
        if let Some(slot) = self.sessions.lock().get(&key).cloned() {
            return Ok(slot);
        }
        if request.action != "start" {
            return Err(failure(
                "COMPUTER_USE_SESSION_NOT_FOUND",
                "请先连接已绑定的设备",
            ));
        }
        self.availability()?;
        let binding = (self.binding)()?;
        let mut sessions = self.sessions.lock();
        if let Some(slot) = sessions.get(&key) {
            return Ok(slot.clone());
        }
        if sessions.len() >= 4 {
            return Err(failure(
                "COMPUTER_USE_SESSION_LIMIT",
                "最多保留 4 个设备控制会话",
            ));
        }
        let slot = Arc::new(Slot {
            owner: request.owner_id.as_deref().unwrap_or("host").to_string(),
            binding,
            client: OnceCell::new(),
            closed: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            control_id: SyncMutex::new(None),
            created: Instant::now(),
        });
        sessions.insert(key, slot.clone());
        Ok(slot)
    }
    async fn close_slot(&self, slot: &Arc<Slot>) {
        {
            let mut sessions = self.sessions.lock();
            if sessions
                .get(&slot.owner)
                .is_some_and(|current| Arc::ptr_eq(current, slot))
            {
                sessions.remove(&slot.owner);
            }
        }
        slot.closed.store(true, Ordering::SeqCst);
        if let Some(client) = slot.client.get() {
            client.stop().await;
        }
    }
}
#[async_trait]
impl ComputerUseAdapter for DesktopAdapter {
    fn adapter_id(&self) -> &'static str {
        self.backend.id()
    }
    fn availability(&self) -> Result<(), AdapterError> {
        if !cfg!(windows) {
            return Err(failure(
                "COMPUTER_USE_UNAVAILABLE",
                "桌面控制器目前需要 Windows",
            ));
        }
        if !self.worker.is_file() {
            return Err(failure(
                "COMPUTER_USE_UNAVAILABLE",
                "安装包中缺少桌面控制器",
            ));
        }
        if self.backend == Backend::Uu {
            (self.binding)()?;
        }
        Ok(())
    }
    fn control_scope(&self, request: &AdapterRequest) -> Result<String, AdapterError> {
        let key = Self::key(request);
        let device = if let Some(slot) = self.sessions.lock().get(&key) {
            slot.binding.device_id.clone()
        } else {
            (self.binding)()?.device_id
        };
        Ok(format!("{}:{device}", self.backend.id()))
    }
    async fn change_control(
        &self,
        request: &AdapterRequest,
        manual: bool,
    ) -> Result<(), AdapterError> {
        let slot = self.slot(request).await?;
        let client = slot
            .client
            .get()
            .ok_or_else(|| failure("COMPUTER_USE_BUSY", "设备仍在连接中"))?;
        let value = client
            .request(request, &slot.binding, Arc::new(|| false))
            .await?;
        if value.pointer("/control/mode").and_then(Value::as_str)
            != Some(if manual { "manual" } else { "agent" })
        {
            return Err(failure(
                "COMPUTER_USE_MANUAL_CONTROL",
                "控制权切换未完成，继续保持人工接管",
            ));
        }
        Ok(())
    }
    async fn execute(
        &self,
        request: AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        if request.action == "close" {
            self.close_owner(&Self::key(&request)).await?;
            return Ok(AdapterOutput::json(json!({"closed":true})));
        }
        if request.action == "list_sessions" {
            let sessions = self.sessions.lock();
            let active = sessions.get(&Self::key(&request)).filter(|s| {
                !s.closed.load(Ordering::SeqCst)
                    && s.client
                        .get()
                        .is_some_and(|c| c.rpc.alive.load(Ordering::SeqCst))
            });
            return Ok(AdapterOutput::json(
                json!({"sessions":active.map(|_| json!({"sessionId":"default"})).into_iter().collect::<Vec<_>>()}),
            ));
        }
        let slot = self.slot(&request).await?;
        if request.action == "release_inputs"
            && (!slot.ready.load(Ordering::SeqCst)
                || request.arguments["controlId"].as_str().is_none()
                || request.arguments["controlId"].as_str() != slot.control_id.lock().as_deref())
        {
            // Reject an old video consumer before its command reaches the
            // worker reader, which preempts queued capture/input immediately.
            // A new worker has no published control identity until start
            // succeeds; stale cleanup must never initialize or interrupt it.
            return Err(failure(
                "COMPUTER_USE_STALE_CONTROL",
                "旧画面的输入清理不属于当前已连接桌面",
            ));
        }
        let client = match slot
            .client
            .get_or_try_init(|| {
                Client::spawn(&self.worker, &self.data_root, &slot.binding, self.backend)
            })
            .await
        {
            Ok(client) => client.clone(),
            Err(error) => {
                self.close_slot(&slot).await;
                return Err(error);
            }
        };
        if slot.closed.load(Ordering::SeqCst) {
            client.stop().await;
            return Err(AdapterError::cancelled());
        }
        let result = client.request(&request, &slot.binding, signal).await;
        if request.action == "start" && result.is_err() {
            self.close_slot(&slot).await;
        }
        let mut value = result?;
        if request.action == "start" {
            *slot.control_id.lock() = value
                .pointer("/state/controlId")
                .and_then(Value::as_str)
                .map(str::to_string);
            slot.ready.store(true, Ordering::SeqCst);
        }
        let screenshot = value.as_object_mut().and_then(|v| v.remove("screenshot"));
        let screenshot = if let Some(screenshot) = screenshot {
            let encoded = screenshot["base64"]
                .as_str()
                .ok_or_else(|| failure("COMPUTER_USE_INVALID_OUTPUT", "控制器画面格式无效"))?;
            if encoded.len() > 3 * 1024 * 1024 {
                return Err(failure(
                    "COMPUTER_USE_INVALID_OUTPUT",
                    "控制器画面超过大小限制",
                ));
            }
            let data = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| failure("COMPUTER_USE_INVALID_OUTPUT", "控制器画面编码无效"))?;
            Some(AdapterScreenshot {
                data,
                media_type: "image/jpeg".into(),
                name: Some("desktop.jpg".into()),
            })
        } else {
            None
        };
        Ok(AdapterOutput { value, screenshot })
    }
    async fn close_owner(&self, owner: &str) -> Result<(), AdapterError> {
        let slots = {
            let mut sessions = self.sessions.lock();
            let keys = sessions
                .iter()
                .filter(|(_, s)| s.owner == owner)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| sessions.remove(&key))
                .collect::<Vec<_>>()
        };
        for slot in slots {
            slot.closed.store(true, Ordering::SeqCst);
            if let Some(client) = slot.client.get() {
                client.stop().await
            }
        }
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        let owners = self
            .sessions
            .lock()
            .values()
            .map(|s| s.owner.clone())
            .collect::<Vec<_>>();
        for owner in owners {
            self.close_owner(&owner).await?
        }
        Ok(())
    }
    fn has_owner_activity(&self, owner: &str) -> bool {
        self.sessions.lock().values().any(|s| {
            s.owner == owner
                && !s.closed.load(Ordering::SeqCst)
                && s.client
                    .get()
                    .is_none_or(|c| c.rpc.alive.load(Ordering::SeqCst))
        })
    }
    fn mark_owner_active(&self, owner: &str) {
        let sessions = self.sessions.lock();
        if let Some(slot) = sessions.get(owner)
            && !slot.closed.load(Ordering::SeqCst)
            && slot.ready.load(Ordering::SeqCst)
            && let Some(client) = slot.client.get()
            && client.rpc.alive.load(Ordering::SeqCst)
        {
            *client.last_used.lock() = Instant::now();
        }
    }
    async fn reap_inactive(&self) -> Vec<String> {
        let now = Instant::now();
        let owners = self
            .sessions
            .lock()
            .values()
            .filter(|s| s.inactive_at(now))
            .map(|s| s.owner.clone())
            .collect::<Vec<_>>();
        for owner in &owners {
            let _ = self.close_owner(owner).await;
        }
        owners
    }
}
