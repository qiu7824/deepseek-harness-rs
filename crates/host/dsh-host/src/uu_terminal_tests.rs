use super::*;
use dsh_subprocess::{SubprocessOutcome, SubprocessTerminalForeground, SubprocessTerminalSignal};
use futures::{channel::mpsc, stream::BoxStream};
use std::sync::atomic::AtomicU64;

const READY: &str =
    "[Connect] Connected\r\n[Tips] Type 'exit' to end the remote session.\r\nPS C:\\fixture> ";

#[derive(Default)]
struct ClientState {
    sender: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<Vec<u8>>>>,
    exited: AtomicBool,
    terminated: AtomicBool,
    block_write: AtomicBool,
    started_write: AtomicBool,
    fail_terminate: AtomicBool,
    written: Mutex<Vec<String>>,
    remote: Mutex<Option<(Arc<Mutex<BTreeSet<String>>>, String)>>,
    fail_exit: AtomicBool,
    late_takeover: AtomicBool,
    output_until_terminate: AtomicBool,
    changed: tokio::sync::Notify,
}
struct FakeClient(Arc<ClientState>);
impl FakeClient {
    fn new(ready: bool) -> Arc<Self> {
        let (sender, receiver) = mpsc::unbounded();
        sender
            .unbounded_send(
                if ready {
                    READY.as_bytes()
                } else {
                    b"[Connect] Initializing connection\r\n"
                }
                .to_vec(),
            )
            .unwrap();
        Arc::new(Self(Arc::new(ClientState {
            sender: Mutex::new(Some(sender)),
            receiver: Mutex::new(Some(receiver)),
            ..Default::default()
        })))
    }
    fn end(&self) {
        self.0.exited.store(true, Ordering::Release);
        self.0.sender.lock().take();
        self.0.changed.notify_waiters();
    }
}
impl SubprocessTerminalHandle for FakeClient {
    fn pid(&self) -> u32 {
        1
    }
    fn output(&self) -> BoxStream<'static, Vec<u8>> {
        self.0
            .receiver
            .lock()
            .take()
            .map(|receiver| receiver.boxed())
            .unwrap_or_else(|| futures::stream::empty().boxed())
    }
    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        let state = self.0.clone();
        Box::pin(async move {
            loop {
                let notified = state.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if state.exited.load(Ordering::Acquire) {
                    return Ok(SubprocessOutcome {
                        exit_code: Some(0),
                        signal: None,
                    });
                }
                notified.await;
            }
        })
    }
    fn write(&self, data: &str) -> BoxFuture<'static, Result<(), String>> {
        let state = self.0.clone();
        let data = data.to_string();
        Box::pin(async move {
            state.started_write.store(true, Ordering::Release);
            while state.block_write.load(Ordering::Acquire) && !state.exited.load(Ordering::Acquire)
            {
                state.changed.notified().await;
            }
            if state.exited.load(Ordering::Acquire) {
                return Err("mock client exited".into());
            }
            if data == "exit\r" && state.fail_exit.load(Ordering::Acquire) {
                return Err("mock exit failed".into());
            }
            state.written.lock().push(data.clone());
            if data == "exit\r" {
                if let Some((ids, id)) = state.remote.lock().as_ref() {
                    ids.lock().remove(id);
                }
                state.exited.store(true, Ordering::Release);
                if state.late_takeover.load(Ordering::Acquire) {
                    let delayed = state.sender.lock().take();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        if let Some(sender) = delayed {
                            let _ = sender.unbounded_send(b"Error: Terminal session attached from another window (code 2001)\r\n".to_vec());
                        }
                    });
                } else if !state.output_until_terminate.load(Ordering::Acquire) {
                    state.sender.lock().take();
                }
                state.changed.notify_waiters();
            } else if let Some(sender) = state.sender.lock().as_ref() {
                let _ =
                    sender.unbounded_send(format!("echo:{data}\r\nPS C:\\fixture> ").into_bytes());
            }
            Ok(())
        })
    }
    fn resize(&self, _rows: u16, _cols: u16) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn inspect_foreground(
        &self,
    ) -> BoxFuture<'static, Result<Option<SubprocessTerminalForeground>, String>> {
        Box::pin(async { Ok(None) })
    }
    fn signal_foreground(
        &self,
        _signal: SubprocessTerminalSignal,
    ) -> BoxFuture<'static, Result<u32, String>> {
        Box::pin(async { Err("no process signal should be used for remote cleanup".into()) })
    }
    fn terminate(&self) -> BoxFuture<'static, Result<(), String>> {
        let state = self.0.clone();
        Box::pin(async move {
            if state.fail_terminate.load(Ordering::Acquire) {
                return Err("mock local cleanup failed".into());
            }
            state.terminated.store(true, Ordering::Release);
            state.exited.store(true, Ordering::Release);
            state.sender.lock().take();
            state.changed.notify_waiters();
            Ok(())
        })
    }
}

struct FakeAccess {
    account: AtomicBool,
    binding: AtomicBool,
    target: TerminalTarget,
}
impl Access for Arc<FakeAccess> {
    fn target(
        &self,
        device: Option<String>,
        signal: Abort,
    ) -> BoxFuture<'static, Result<TerminalTarget, String>> {
        let value = self.clone();
        Box::pin(async move {
            if signal()
                || !value.account.load(Ordering::Acquire)
                || !value.binding.load(Ordering::Acquire)
                || device
                    .as_ref()
                    .is_some_and(|id| id != &value.target.device_id)
            {
                return Err("mock bound target unavailable".into());
            }
            Ok(value.target.clone())
        })
    }
    fn validate(
        &self,
        target: TerminalTarget,
        binding: bool,
        signal: Abort,
    ) -> BoxFuture<'static, Result<(), String>> {
        let value = self.clone();
        Box::pin(async move {
            if signal()
                || !value.account.load(Ordering::Acquire)
                || target.account != value.target.account
                || target.device_id != value.target.device_id
            {
                return Err("mock account rejected".into());
            }
            if binding && !value.binding.load(Ordering::Acquire) {
                return Err("mock binding changed".into());
            }
            Ok(())
        })
    }
}
struct FakeTransport {
    ids: Arc<Mutex<BTreeSet<String>>>,
    clients: Mutex<Vec<Arc<FakeClient>>>,
    next: AtomicU64,
    spawned: AtomicU64,
    queries: AtomicU64,
    displace_queries: AtomicBool,
    ambiguous: AtomicBool,
    ready: AtomicBool,
}
impl Transport for Arc<FakeTransport> {
    fn sessions(
        &self,
        _target: TerminalTarget,
        signal: Abort,
    ) -> BoxFuture<'static, Result<BTreeSet<String>, String>> {
        let value = self.clone();
        Box::pin(async move {
            if signal() {
                Err("mock cancelled query".into())
            } else {
                value.queries.fetch_add(1, Ordering::AcqRel);
                if value.displace_queries.load(Ordering::Acquire) {
                    for client in value.clients.lock().iter() {
                        if !client.0.exited.load(Ordering::Acquire) {
                            if let Some(sender) = client.0.sender.lock().as_ref() {
                                let _ = sender.unbounded_send(b"Error: Terminal session attached from another window (code 2001)\r\n".to_vec());
                            }
                            client.end();
                        }
                    }
                }
                Ok(value.ids.lock().clone())
            }
        })
    }
    fn spawn(
        &self,
        _target: TerminalTarget,
        _shell: TerminalShell,
        _cwd: String,
        signal: Abort,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        let value = self.clone();
        Box::pin(async move {
            if signal() {
                return Err("mock spawn cancelled".into());
            }
            let id = value.next.fetch_add(1, Ordering::AcqRel).to_string();
            value.ids.lock().insert(id.clone());
            if value.ambiguous.load(Ordering::Acquire) {
                value.ids.lock().insert("999".into());
            }
            let client = FakeClient::new(value.ready.load(Ordering::Acquire));
            *client.0.remote.lock() = Some((value.ids.clone(), id));
            value.clients.lock().push(client.clone());
            value.spawned.fetch_add(1, Ordering::Release);
            Ok(client as Arc<dyn SubprocessTerminalHandle>)
        })
    }
}
struct Fixture {
    service: Arc<RemoteTerminals>,
    access: Arc<FakeAccess>,
    transport: Arc<FakeTransport>,
    approval_calls: Arc<Mutex<Vec<String>>>,
    denied: Arc<Mutex<BTreeSet<String>>>,
    cancelled: Arc<AtomicBool>,
    tracked: Arc<Mutex<Vec<Arc<Slot>>>>,
}
impl Fixture {
    fn new() -> Self {
        let target = TerminalTarget {
            cli: std::path::PathBuf::from("mock/uuyc-cli.exe"),
            account: "account-a".into(),
            device_id: "device-a".into(),
            device_name: "Remote fixture".into(),
        };
        let access = Arc::new(FakeAccess {
            account: AtomicBool::new(true),
            binding: AtomicBool::new(true),
            target,
        });
        let transport = Arc::new(FakeTransport {
            ids: Arc::new(Mutex::new(BTreeSet::from(["1".into()]))),
            clients: Default::default(),
            next: AtomicU64::new(2),
            spawned: AtomicU64::new(0),
            queries: AtomicU64::new(0),
            displace_queries: AtomicBool::new(true),
            ambiguous: AtomicBool::new(false),
            ready: AtomicBool::new(true),
        });
        let service = Arc::new(RemoteTerminals {
            access: Arc::new(access.clone()),
            transport: Arc::new(transport.clone()),
            slots: Default::default(),
            admission: Default::default(),
            closing: AtomicBool::new(false),
        });
        Self {
            service,
            access,
            transport,
            approval_calls: Default::default(),
            denied: Default::default(),
            cancelled: Default::default(),
            tracked: Default::default(),
        }
    }
    fn caller(&self, owner: &str) -> Caller {
        let calls = self.approval_calls.clone();
        let denied = self.denied.clone();
        let tracked = self.tracked.clone();
        let cancelled = self.cancelled.clone();
        Caller {
            owner: owner.into(),
            cwd: "mock-cwd".into(),
            signal: Arc::new(move || cancelled.load(Ordering::Acquire)),
            approve: Arc::new(move |action, _, _| {
                let action = action.to_string();
                let calls = calls.clone();
                let denied = denied.clone();
                Box::pin(async move {
                    calls.lock().push(action.clone());
                    if denied.lock().contains(&action) {
                        Err("mock approval rejected".into())
                    } else {
                        Ok(())
                    }
                })
            }),
            track: Arc::new(move |slot| {
                tracked.lock().push(slot);
                Ok("mock-job".into())
            }),
        }
    }
    async fn open(&self) -> Value {
        self.service
            .open(&self.caller("owner-a"), None, TerminalShell::PowerShell)
            .await
            .unwrap()
    }
    async fn observe(&self, id: &str) {
        tokio::task::yield_now().await;
        self.service
            .find("owner-a", id)
            .unwrap()
            .observe_read(None, MAX_READ)
            .unwrap();
    }
}

#[tokio::test]
async fn remote_round_trip_owns_the_live_connection_without_interfering_inventory_queries() {
    let f = Fixture::new();
    let opened = f.open().await;
    let id = opened["terminalId"].as_str().unwrap();
    assert_eq!(opened["remoteSessionId"], Value::Null);
    assert_eq!(opened["ownership"], "new-cli-connection");
    assert_eq!(f.transport.queries.load(Ordering::Acquire), 1);
    assert!(f.service.find("owner-b", id).is_err());
    assert_eq!(f.service.list("owner-b")["terminals"], json!([]));
    f.service
        .write(&f.caller("owner-a"), id, "echo fixture", true, 0)
        .await
        .unwrap();
    f.observe(id).await;
    assert_eq!(
        f.transport.queries.load(Ordering::Acquire),
        1,
        "reads and writes must not displace the live CLI"
    );
    let closed = f.service.close(&f.caller("owner-a"), id).await.unwrap();
    assert_eq!(closed["cleanupConfirmed"], true);
    assert_eq!(closed["remoteExitObserved"], true);
    assert_eq!(
        f.transport.clients.lock()[0].0.written.lock().as_slice(),
        ["echo fixture\r", "exit\r"]
    );
    assert_eq!(f.transport.queries.load(Ordering::Acquire), 2);
    assert_eq!(*f.transport.ids.lock(), BTreeSet::from(["1".into()]));
    f.service.close(&f.caller("owner-a"), id).await.unwrap();
    assert_eq!(
        f.approval_calls.lock().as_slice(),
        ["open", "write", "close"]
    );
}

#[tokio::test]
async fn denied_open_never_starts_a_remote_client() {
    let f = Fixture::new();
    f.denied.lock().insert("open".into());
    assert!(
        f.service
            .open(&f.caller("owner-a"), None, TerminalShell::Cmd)
            .await
            .is_err()
    );
    assert_eq!(f.transport.spawned.load(Ordering::Acquire), 0);
    assert!(f.tracked.lock().is_empty());
}

#[tokio::test]
async fn binding_change_and_rejected_write_never_send_input() {
    let f = Fixture::new();
    let value = f.open().await;
    let id = value["terminalId"].as_str().unwrap();
    f.denied.lock().insert("write".into());
    assert!(
        f.service
            .write(&f.caller("owner-a"), id, "forbidden", true, 0)
            .await
            .is_err()
    );
    f.denied.lock().clear();
    f.access.binding.store(false, Ordering::Release);
    assert!(
        f.service
            .write(&f.caller("owner-a"), id, "wrong binding", true, 0)
            .await
            .is_err()
    );
    assert!(f.transport.clients.lock()[0].0.written.lock().is_empty());
    // Switching the selected device does not prevent closing this owned
    // session, provided the same account still owns the original target.
    f.observe(id).await;
    f.service.close(&f.caller("owner-a"), id).await.unwrap();
    assert_eq!(*f.transport.ids.lock(), BTreeSet::from(["1".into()]));
}

#[tokio::test]
async fn concurrent_inventory_changes_do_not_become_guessed_owned_ids() {
    let f = Fixture::new();
    f.transport.ambiguous.store(true, Ordering::Release);
    let opened = f.open().await;
    let id = opened["terminalId"].as_str().unwrap();
    f.observe(id).await;
    assert!(f.service.close(&f.caller("owner-a"), id).await.is_err());
    assert_eq!(
        *f.transport.ids.lock(),
        BTreeSet::from(["1".into(), "999".into()])
    );
    assert_eq!(
        f.service
            .find("owner-a", id)
            .unwrap()
            .snapshot(None, 0)
            .unwrap()["cleanupConfirmed"],
        false
    );
}

#[tokio::test]
async fn cancellation_during_start_does_not_send_commands_or_kill_unowned_sessions() {
    let f = Fixture::new();
    f.transport.ready.store(false, Ordering::Release);
    let caller = f.caller("owner-a");
    let service = f.service.clone();
    let operation =
        tokio::spawn(async move { service.open(&caller, None, TerminalShell::Cmd).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.transport.spawned.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.cancelled.store(true, Ordering::Release);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), operation)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(f.transport.ids.lock().contains("1"));
    assert!(f.transport.clients.lock()[0].0.written.lock().is_empty());
}

#[tokio::test]
async fn cancellation_detaches_without_submitting_commands_to_an_unknown_shell_boundary() {
    let f = Fixture::new();
    let value = f.open().await;
    let id = value["terminalId"].as_str().unwrap().to_string();
    let client = f.transport.clients.lock()[0].clone();
    client.0.block_write.store(true, Ordering::Release);
    let caller = f.caller("owner-a");
    let service = f.service.clone();
    let copy = id.clone();
    let operation =
        tokio::spawn(async move { service.write(&caller, &copy, "blocked", true, 0).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !client.0.started_write.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.cancelled.store(true, Ordering::Release);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), operation)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let slot = f.service.find("owner-a", &id).unwrap();
    tokio::time::timeout(Duration::from_secs(2), RemoteJob(slot).done())
        .await
        .unwrap();
    assert!(f.transport.ids.lock().contains("2"));
    assert!(client.0.written.lock().is_empty());
}

#[tokio::test]
async fn lost_connection_does_not_authorize_killing_a_reused_remote_id() {
    let f = Fixture::new();
    let value = f.open().await;
    let id = value["terminalId"].as_str().unwrap();
    let slot = f.service.find("owner-a", id).unwrap();
    slot.client_exited.store(true, Ordering::Release);
    f.transport.clients.lock()[0].end();
    assert!(slot.close_owned(false).await.is_err());
    assert!(f.transport.ids.lock().contains("1"));
    assert!(f.transport.ids.lock().contains("2"));
    assert_eq!(slot.snapshot(None, 0).unwrap()["cleanupConfirmed"], false);
}

#[tokio::test]
async fn account_change_stops_input_and_remote_cleanup_instead_of_crossing_accounts() {
    let f = Fixture::new();
    let value = f.open().await;
    let id = value["terminalId"].as_str().unwrap();
    f.access.account.store(false, Ordering::Release);
    assert!(
        f.service
            .write(&f.caller("owner-a"), id, "no", true, 0)
            .await
            .is_err()
    );
    assert!(f.service.close(&f.caller("owner-a"), id).await.is_err());
    assert!(f.transport.ids.lock().contains("1"));
    assert!(
        f.transport.clients.lock()[0]
            .0
            .terminated
            .load(Ordering::Acquire)
    );
}

#[tokio::test]
async fn lifecycle_disposal_detaches_and_reports_remote_cleanup_uncertainty() {
    let f = Fixture::new();
    let opened = f.open().await;
    let slot = f
        .service
        .find("owner-a", opened["terminalId"].as_str().unwrap())
        .unwrap();
    f.service.dispose().await;
    assert!(
        f.transport.clients.lock()[0]
            .0
            .terminated
            .load(Ordering::Acquire)
    );
    assert!(f.transport.ids.lock().contains("2"));
    assert!(matches!(
        RemoteJob(slot).done().await.status,
        JobOutcomeStatus::Failed
    ));
    assert!(
        f.service
            .open(&f.caller("owner-a"), None, TerminalShell::Cmd)
            .await
            .is_err()
    );
}

#[test]
fn output_is_bounded_and_utf8_fragments_are_not_consumed_early() {
    let mut output = Output::default();
    output.append(&[0xe4]);
    assert_eq!(output.page(Some(0), 4).unwrap(), ("".into(), 0, false));
    output.append(&[0xbd, 0xa0]);
    assert_eq!(output.page(Some(0), 4).unwrap(), ("你".into(), 3, false));
    output.append(&vec![b'a'; MAX_OUTPUT + 100]);
    assert_eq!(output.bytes.len(), MAX_OUTPUT);
    let (_, cursor, truncated) = output.page(Some(0), 16).unwrap();
    assert!(truncated);
    assert_eq!(cursor, output.start + 16);
    assert!(output.page(Some(output.end + 1), 16).is_err());
}

#[test]
fn numeric_arguments_reject_invalid_values_without_schema_extensions() {
    assert_eq!(
        numeric_argument(&json!({}), "waitMs", Some(250), 0, 5000).unwrap(),
        Some(250)
    );
    assert_eq!(
        numeric_argument(&json!({"waitMs":2.0}), "waitMs", None, 0, 5000).unwrap(),
        Some(2)
    );
    for value in [
        json!(-1),
        json!(1.5),
        json!(5001),
        json!(null),
        json!("100"),
    ] {
        assert!(numeric_argument(&json!({"waitMs":value}), "waitMs", None, 0, 5000).is_err());
    }
    assert!(
        numeric_argument(
            &json!({"limitBytes":1}),
            "limitBytes",
            None,
            4,
            MAX_READ as u64
        )
        .is_err()
    );
}

#[tokio::test]
async fn failed_local_cleanup_keeps_the_handle_for_a_safe_retry() {
    let f = Fixture::new();
    let opened = f.open().await;
    let id = opened["terminalId"].as_str().unwrap();
    let client = f.transport.clients.lock()[0].clone();
    client.0.fail_terminate.store(true, Ordering::Release);
    f.observe(id).await;
    assert!(f.service.close(&f.caller("owner-a"), id).await.is_err());
    let slot = f.service.find("owner-a", id).unwrap();
    assert!(slot.state.lock().client.is_some());
    assert_eq!(slot.snapshot(None, 0).unwrap()["cleanupConfirmed"], false);
    client.0.fail_terminate.store(false, Ordering::Release);
    f.service.close(&f.caller("owner-a"), id).await.unwrap();
    assert!(slot.state.lock().client.is_none());
    assert_eq!(*f.transport.ids.lock(), BTreeSet::from(["1".into()]));
}

#[tokio::test]
async fn same_device_rejects_another_open_before_any_displacing_query() {
    let f = Fixture::new();
    let opened = f.open().await;
    assert!(
        f.service
            .open(&f.caller("owner-b"), None, TerminalShell::PowerShell)
            .await
            .is_err()
    );
    assert_eq!(f.transport.queries.load(Ordering::Acquire), 1);
    assert_eq!(f.transport.spawned.load(Ordering::Acquire), 1);
    let id = opened["terminalId"].as_str().unwrap();
    f.observe(id).await;
    f.service.close(&f.caller("owner-a"), id).await.unwrap();
}
#[tokio::test]
async fn unsubmitted_input_is_never_completed_by_cleanup_exit() {
    let f = Fixture::new();
    let opened = f.open().await;
    let id = opened["terminalId"].as_str().unwrap();
    f.service
        .write(&f.caller("owner-a"), id, "unfinished ", false, 0)
        .await
        .unwrap();
    f.observe(id).await;
    assert!(f.service.close(&f.caller("owner-a"), id).await.is_err());
    assert_eq!(
        f.transport.clients.lock()[0].0.written.lock().as_slice(),
        ["unfinished "]
    );
    assert!(f.transport.ids.lock().contains("2"));
}
#[tokio::test]
async fn takeover_is_latched_before_process_exit_and_survives_output_truncation() {
    let f = Fixture::new();
    let opened = f.open().await;
    let id = opened["terminalId"].as_str().unwrap();
    let client = f.transport.clients.lock()[0].clone();
    client
        .0
        .sender
        .lock()
        .as_ref()
        .unwrap()
        .unbounded_send(
            b"Error: Terminal session attached from another window (code 2001)\r\n".to_vec(),
        )
        .unwrap();
    tokio::task::yield_now().await;
    client
        .0
        .sender
        .lock()
        .as_ref()
        .unwrap()
        .unbounded_send(vec![b'x'; MAX_OUTPUT + 100])
        .unwrap();
    tokio::task::yield_now().await;
    let error = f
        .service
        .write(&f.caller("owner-a"), id, "never", true, 0)
        .await
        .unwrap_err();
    assert!(error.contains("2001"));
    assert!(f.service.close(&f.caller("owner-a"), id).await.is_err());
    assert!(client.0.written.lock().is_empty());
    assert!(f.transport.ids.lock().contains("2"));
}
#[tokio::test]
async fn exit_failure_or_missing_remote_absence_never_claims_cleanup() {
    let f = Fixture::new();
    let opened = f.open().await;
    let id = opened["terminalId"].as_str().unwrap();
    f.observe(id).await;
    f.transport.clients.lock()[0]
        .0
        .fail_exit
        .store(true, Ordering::Release);
    assert!(f.service.close(&f.caller("owner-a"), id).await.is_err());
    assert_eq!(
        f.service
            .find("owner-a", id)
            .unwrap()
            .snapshot(None, 0)
            .unwrap()["cleanupConfirmed"],
        false
    );
    assert!(f.transport.ids.lock().contains("2"));
}

#[tokio::test]
async fn reading_only_an_old_output_prefix_does_not_authorize_cleanup_input() {
    let f = Fixture::new();
    let opened = f.open().await;
    let id = opened["terminalId"].as_str().unwrap();
    let slot = f.service.find("owner-a", id).unwrap();
    slot.observe_read(Some(0), 4).unwrap();
    assert_eq!(slot.snapshot(None, 0).unwrap()["canCloseGracefully"], false);
    assert!(f.service.close(&f.caller("owner-a"), id).await.is_err());
    assert!(f.transport.clients.lock()[0].0.written.lock().is_empty());
}

#[tokio::test]
async fn process_exit_before_trailing_takeover_output_cannot_confirm_remote_cleanup() {
    let f = Fixture::new();
    let opened = f.open().await;
    let id = opened["terminalId"].as_str().unwrap();
    f.observe(id).await;
    f.transport.clients.lock()[0]
        .0
        .late_takeover
        .store(true, Ordering::Release);
    let error = f.service.close(&f.caller("owner-a"), id).await.unwrap_err();
    assert!(error.contains("2001"));
    let state = f
        .service
        .find("owner-a", id)
        .unwrap()
        .snapshot(None, 0)
        .unwrap();
    assert_eq!(state["cleanupConfirmed"], false);
    assert_eq!(state["ownershipLost"], true);
    assert_eq!(state["remoteExitObserved"], false);
}

#[tokio::test]
async fn normal_exit_closes_the_local_pty_before_waiting_for_conpty_output_eof() {
    let f = Fixture::new();
    let opened = f.open().await;
    let id = opened["terminalId"].as_str().unwrap();
    f.observe(id).await;
    f.transport.clients.lock()[0]
        .0
        .output_until_terminate
        .store(true, Ordering::Release);
    let closed = tokio::time::timeout(
        Duration::from_secs(1),
        f.service.close(&f.caller("owner-a"), id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(closed["remoteExitObserved"], true);
    assert_eq!(closed["cleanupConfirmed"], true);
    assert_eq!(*f.transport.ids.lock(), BTreeSet::from(["1".into()]));
}
