use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

struct StartupSandbox {
    prepare_ms: u64,
    ready_ms: u64,
    never: bool,
    fail: bool,
    prepared: Arc<AtomicUsize>,
}
impl SandboxProvider for StartupSandbox {
    fn prepare(
        &self,
        _: &SandboxExecutionPolicy,
    ) -> futures::future::BoxFuture<'static, Result<(), String>> {
        let (delay, never, fail, prepared) = (
            self.prepare_ms,
            self.never,
            self.fail,
            self.prepared.clone(),
        );
        Box::pin(async move {
            prepared.fetch_add(1, Ordering::SeqCst);
            if never {
                std::future::pending::<()>().await;
            }
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if fail {
                Err("fixture OS startup rejected".into())
            } else {
                Ok(())
            }
        })
    }
    fn confine(
        &self,
        _: &[String],
        _: &SandboxPolicy,
    ) -> Result<dsh_sandbox::ConfinedArgv, dsh_sandbox::SandboxUnavailableError> {
        panic!("the consumer must retain its startup handshake")
    }
    fn confine_with_startup(
        &self,
        argv: &[String],
        _: &SandboxPolicy,
    ) -> Result<dsh_sandbox::ConfinedArgv, dsh_sandbox::SandboxUnavailableError> {
        assert_eq!(self.prepared.load(Ordering::SeqCst), 1);
        let started = tokio::time::Instant::now();
        let delay = Duration::from_millis(self.ready_ms);
        Ok(dsh_sandbox::ConfinedArgv {
            argv: argv.to_vec(),
            enforcement: SandboxEnforcement::Full,
            denial_signatures: vec![],
            runner_failure_rules: vec![],
            startup: Some(dsh_sandbox::SandboxStartup::new(
                move || Ok(started.elapsed() >= delay),
                || Ok(false),
            )),
        })
    }
}
fn fixture(
    prepare_ms: u64,
    ready_ms: u64,
    never: bool,
    fail: bool,
) -> (Context, Arc<NodeCodeRuntime>) {
    let ctx = Context::root();
    dsh_subprocess_local::LocalSubprocessRuntime::install(&ctx);
    ctx.register_service(Arc::new(StartupSandbox {
        prepare_ms,
        ready_ms,
        never,
        fail,
        prepared: Arc::new(AtomicUsize::new(0)),
    }) as Arc<dyn SandboxProvider>);
    let runtime = NodeCodeRuntime::install(
        &ctx,
        Config {
            require_os_sandbox: true,
            ..Default::default()
        },
    )
    .unwrap();
    (ctx, runtime)
}
fn request(program: &str, budget: u64, dispatches: Arc<AtomicUsize>) -> CodeRunRequest {
    CodeRunRequest {
        timeout_ms: Some(budget),
        on_dispatch: Some(Arc::new(move || {
            dispatches.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })),
        program: program.into(),
        bindings: vec![],
        signal: None,
    }
}

#[tokio::test]
async fn os_preparation_and_readiness_do_not_consume_the_program_budget() {
    let (_ctx, runtime) = fixture(800, 800, false, false);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let began = std::time::Instant::now();
    let result = runtime
        .run(request("return 42;", 500, dispatches.clone()))
        .await
        .unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.value, Some(json!(42)));
    assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    assert!(began.elapsed() >= Duration::from_millis(1600));
    runtime.dispose().await;
    assert!(runtime.lifecycle.state.lock().active.is_empty());
}

#[tokio::test(start_paused = true)]
async fn uncooperative_startup_has_a_separate_bounded_deadline() {
    let (_ctx, runtime) = fixture(0, 0, true, false);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let began = tokio::time::Instant::now();
    let result = runtime
        .run(request("return 1;", 20, dispatches.clone()))
        .await
        .unwrap();
    assert_eq!(result.error.unwrap().kind, CodeRunFailureKind::Startup);
    assert!(began.elapsed() >= STARTUP_BUDGET);
    assert_eq!(dispatches.load(Ordering::SeqCst), 0);
    assert!(runtime.lifecycle.state.lock().active.is_empty());
    runtime.dispose().await;
}

#[tokio::test]
async fn cancellation_while_waiting_for_ready_never_dispatches_model_code() {
    let (_ctx, runtime) = fixture(0, 2000, false, false);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let mut req = request("return 1;", 500, dispatches.clone());
    req.signal = Some(Arc::new(move || flag.load(Ordering::SeqCst)));
    let run = runtime.run(req);
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancelled.store(true, Ordering::SeqCst);
    };
    let (result, _) = tokio::join!(run, cancel);
    assert_eq!(
        result.unwrap().error.unwrap().kind,
        CodeRunFailureKind::Abort
    );
    assert_eq!(dispatches.load(Ordering::SeqCst), 0);
    runtime.dispose().await;
    assert!(runtime.lifecycle.state.lock().active.is_empty());
}

#[tokio::test]
async fn user_cancellation_after_dispatch_keeps_the_started_boundary() {
    let (_ctx, runtime) = fixture(0, 0, false, false);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let mut req = request("await new Promise(() => {});", 5000, dispatches.clone());
    req.signal = Some(Arc::new(move || flag.load(Ordering::SeqCst)));
    let cancel = async {
        while dispatches.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        cancelled.store(true, Ordering::SeqCst);
    };
    let (result, _) = tokio::join!(runtime.run(req), cancel);
    assert_eq!(
        result.unwrap().error.unwrap().kind,
        CodeRunFailureKind::Abort
    );
    assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    runtime.dispose().await;
    assert!(runtime.lifecycle.state.lock().active.is_empty());
}

#[tokio::test]
async fn ready_does_not_extend_a_short_program_deadline_or_reset_dispatch() {
    let (_ctx, runtime) = fixture(0, 350, false, false);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let began = std::time::Instant::now();
    let result = runtime
        .run(request("while(true) {}", 100, dispatches.clone()))
        .await
        .unwrap();
    assert_eq!(result.error.unwrap().kind, CodeRunFailureKind::Timeout);
    assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    assert!(began.elapsed() >= Duration::from_millis(450));
    assert!(began.elapsed() < Duration::from_secs(3));
    runtime.dispose().await;
}

#[tokio::test]
async fn failed_startup_and_rejected_dispatch_guard_do_not_execute_programs() {
    let (_ctx, runtime) = fixture(0, 0, false, true);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let result = runtime
        .run(request(
            "throw new Error('MUST_NOT_RUN')",
            500,
            dispatches.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(result.error.unwrap().kind, CodeRunFailureKind::Startup);
    assert_eq!(dispatches.load(Ordering::SeqCst), 0);
    runtime.dispose().await;
    let (_ctx, runtime) = fixture(0, 0, false, false);
    let mut req = request("throw new Error('MUST_NOT_RUN')", 500, dispatches);
    req.on_dispatch = Some(Arc::new(|| {
        Err("execution context changed before dispatch".into())
    }));
    let result = runtime.run(req).await.unwrap();
    let error = result.error.unwrap();
    assert_eq!(error.kind, CodeRunFailureKind::Startup);
    assert!(error.message.contains("execution context changed"));
    assert!(!error.message.contains("MUST_NOT_RUN"));
    runtime.dispose().await;
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "requires explicitly prepared DSH_NODE_NATIVE_* fixtures and a cold private migration"]
async fn actual_native_private_migration_precedes_the_short_ptc_budget() {
    use std::path::PathBuf;
    let home =
        PathBuf::from(std::env::var_os("DSH_NODE_NATIVE_HOME").expect("explicit fixture home"));
    let workspace = PathBuf::from(
        std::env::var_os("DSH_NODE_NATIVE_WORKSPACE").expect("explicit fixture workspace"),
    );
    let private =
        std::env::var("DSH_NODE_NATIVE_PRIVATE_ROOT").expect("explicit fixture private root");
    let _roots =
        dsh_sandbox::roots::register_private_roots(Arc::new(move || vec![private.clone()]));
    let ctx = Context::root();
    dsh_subprocess_local::LocalSubprocessRuntime::install(&ctx);
    dsh_sandbox_local::LocalSandboxProvider::install_with_runtimes_at_home(
        &ctx,
        Default::default(),
        vec![],
        home.join("cache/sandbox"),
        home,
    );
    let runtime = NodeCodeRuntime::install(
        &ctx,
        Config {
            require_os_sandbox: true,
            runner_directory: Some(workspace),
            ..Default::default()
        },
    )
    .unwrap();
    let dispatches = Arc::new(AtomicUsize::new(0));
    let began = std::time::Instant::now();
    let result = runtime
        .run(request("return 123;", 1500, dispatches.clone()))
        .await
        .unwrap();
    let elapsed = began.elapsed();
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.value, Some(json!(123)));
    assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    assert!(
        elapsed > Duration::from_millis(1500),
        "fixture must exercise a cold startup longer than the program budget"
    );
    let second = runtime
        .run(request("while(true) {}", 100, dispatches.clone()))
        .await
        .unwrap();
    assert_eq!(second.error.unwrap().kind, CodeRunFailureKind::Timeout);
    assert_eq!(dispatches.load(Ordering::SeqCst), 2);
    runtime.dispose().await;
    assert!(runtime.lifecycle.state.lock().active.is_empty());
    println!(
        "NATIVE_PTC_STARTUP={}",
        json!({"coldElapsedMs":elapsed.as_millis(),"programBudgetMs":1500,"shortProgramSucceeded":true,"warmInfiniteProgramTimedOut":true})
    );
}
