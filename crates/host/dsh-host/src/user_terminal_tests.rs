use super::*;
use dsh_session::session_id;
use std::time::Duration;

#[derive(Default)]
struct Backend {
    input: Arc<Mutex<String>>,
    closed: Arc<AtomicBool>,
}
impl TerminalBackendSession for Backend {
    fn motd(&self) -> String {
        "ready".into()
    }
    fn pid(&self) -> Option<u32> {
        Some(42)
    }
    fn start_send(&self, _: &TerminalSendRequest) -> Arc<dyn TerminalSendOperation> {
        panic!("not used by raw input tests")
    }
    fn write_input(&self, data: &str) -> BoxFuture<'static, Result<(), String>> {
        self.input.lock().push_str(data);
        Box::pin(async { Ok(()) })
    }
    fn resize(&self, _: u16, _: u16) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn read(&self, _: &TerminalReadRequest) -> TerminalReadResult {
        TerminalReadResult {
            text: self.input.lock().clone(),
            total_lines: 1,
            line_begin: 0,
            line_end: 1,
            truncated: false,
        }
    }
    fn signal(
        &self,
        _: TerminalSignal,
    ) -> BoxFuture<'static, Result<TerminalSignalResult, String>> {
        Box::pin(async {
            Ok(TerminalSignalResult {
                delivered: true,
                target_pgid: 42,
            })
        })
    }
    fn status(&self) -> TerminalSessionStatus {
        if self.closed.load(Ordering::Acquire) {
            TerminalSessionStatus::Exited {
                exit_code: Some(0),
                signal: None,
            }
        } else {
            TerminalSessionStatus::Running
        }
    }
    fn close(&self, _: &str) -> BoxFuture<'static, Result<(), String>> {
        self.closed.store(true, Ordering::Release);
        Box::pin(async { Ok(()) })
    }
}
fn request() -> TerminalSpawnRequest {
    TerminalSpawnRequest {
        type_: "shell".into(),
        cwd: Some("workspace".into()),
        name: None,
    }
}
async fn shutdown(ctx: &Context) {
    for disposer in ctx.fiber.disposables.clear() {
        disposer().await;
    }
}

#[tokio::test]
async fn user_terminals_need_no_agent_and_reject_other_session_control() {
    let ctx = Context::root();
    let backend = Arc::new(Backend::default());
    let factory = backend.clone();
    let terminals = UserTerminals::with_factory(
        &ctx,
        Arc::new(move |_| {
            let backend = factory.clone();
            Box::pin(async move { Ok(backend as Arc<dyn TerminalBackendSession>) })
        }),
    );
    assert!(ctx.get("agents", false).is_none());
    let owner = session_id("cold-conversation");
    let other = session_id("other");
    let opened = terminals
        .spawn_limited(owner.clone(), request(), None, 3)
        .unwrap()
        .await
        .unwrap();
    assert!(opened.session_id.as_str().starts_with("user-"));
    assert!(
        terminals
            .write_input(&other, &opened.session_id, "must not execute")
            .is_err()
    );
    assert!(
        terminals
            .read(&other, &opened.session_id, Default::default())
            .is_err()
    );
    assert!(
        terminals
            .kill(&other, &opened.session_id, "foreign close".into())
            .is_err()
    );
    terminals
        .write_input(&owner, &opened.session_id, "echo ok")
        .unwrap()
        .await
        .unwrap();
    assert_eq!(
        terminals
            .read(&owner, &opened.session_id, Default::default())
            .unwrap()
            .text,
        "echo ok"
    );
    assert_eq!(terminals.list(&owner).len(), 1);
    shutdown(&ctx).await;
    assert!(backend.closed.load(Ordering::Acquire));
    assert!(terminals.list(&owner).is_empty());
}

#[tokio::test]
async fn pending_limit_and_dropped_open_close_a_late_backend() {
    let ctx = Context::root();
    let release = Arc::new(tokio::sync::Notify::new());
    let entered = Arc::new(tokio::sync::Notify::new());
    let backend = Arc::new(Backend::default());
    let (factory_backend, factory_release, factory_entered) =
        (backend.clone(), release.clone(), entered.clone());
    let terminals = UserTerminals::with_factory(
        &ctx,
        Arc::new(move |_| {
            let (backend, release, entered) = (
                factory_backend.clone(),
                factory_release.clone(),
                factory_entered.clone(),
            );
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                Ok(backend as Arc<dyn TerminalBackendSession>)
            })
        }),
    );
    let owner = session_id("owner");
    let opening = terminals
        .spawn_limited(owner.clone(), request(), None, 1)
        .unwrap();
    entered.notified().await;
    assert!(
        terminals
            .spawn_limited(owner.clone(), request(), None, 1)
            .is_err()
    );
    drop(opening);
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), terminals.dispose_all())
        .await
        .unwrap()
        .unwrap();
    assert!(backend.closed.load(Ordering::Acquire));
    assert!(terminals.list(&owner).is_empty());
    assert!(terminals.state.lock().pending.is_empty());
    shutdown(&ctx).await;
}

#[tokio::test]
async fn shutdown_cancels_startup_and_waits_for_its_cleanup() {
    let ctx = Context::root();
    let entered = Arc::new(tokio::sync::Notify::new());
    let factory_entered = entered.clone();
    let terminals = UserTerminals::with_factory(
        &ctx,
        Arc::new(move |spec| {
            let entered = factory_entered.clone();
            Box::pin(async move {
                entered.notify_one();
                while !(spec.signal)() {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                Err(TerminalBackendSpawnError::spawn("startup cancelled"))
            })
        }),
    );
    let opening = terminals
        .spawn_limited(session_id("owner"), request(), None, 3)
        .unwrap();
    entered.notified().await;
    tokio::time::timeout(Duration::from_secs(2), terminals.dispose_all())
        .await
        .unwrap()
        .unwrap();
    assert!(opening.await.is_err());
    assert!(
        terminals
            .spawn_limited(session_id("owner"), request(), None, 3)
            .is_err()
    );
    shutdown(&ctx).await;
}

#[tokio::test]
#[ignore = "launches a real local PTY; explicit integration check"]
async fn real_user_terminal_runs_with_read_only_agent_defaults_and_no_live_agent() {
    let root = std::env::temp_dir().join(format!("user-terminal-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let ctx = Context::root();
    let _runtime = dsh_subprocess_local::LocalSubprocessRuntime::install(&ctx);
    let policy = dsh_sandbox_policy::SandboxPolicyService::install(
        &ctx,
        dsh_sandbox_policy::Config {
            mode: Some(dsh_sandbox::SandboxMode::ReadOnly),
            workspace_root: Some(root.to_string_lossy().into_owned()),
        },
    );
    TerminalSessionService::install(&ctx);
    let backend = ShellTerminalBackend::install(&ctx, Default::default()).unwrap();
    let terminals = UserTerminals::install(&ctx, backend);
    let owner = session_id("cold-user-terminal");
    let opened = tokio::time::timeout(
        Duration::from_secs(25),
        terminals
            .spawn_limited(
                owner.clone(),
                TerminalSpawnRequest {
                    cwd: Some(root.to_string_lossy().into_owned()),
                    ..request()
                },
                None,
                3,
            )
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(opened.pid.is_some());
    assert!(ctx.get("agents", false).is_none());
    terminals
        .write_input(
            &owner,
            &opened.session_id,
            "echo USER_TERMINAL_OK > terminal-proof.txt\r\n",
        )
        .unwrap()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if std::fs::read_to_string(root.join("terminal-proof.txt"))
                .is_ok_and(|text| text.contains("USER_TERMINAL_OK"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        policy.resolve(&Default::default()).mode,
        dsh_sandbox::SandboxMode::ReadOnly
    );
    terminals
        .kill(&owner, &opened.session_id, "integration finished".into())
        .unwrap()
        .await
        .unwrap();
    shutdown(&ctx).await;
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
