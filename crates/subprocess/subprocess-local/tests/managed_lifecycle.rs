use dsh_subprocess::*;
use dsh_subprocess_local::{SpawnInternals, spawn_subprocess};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

fn spec(mode: &str) -> SubprocessSpawnSpec {
    SubprocessSpawnSpec {
        argv: vec![env!("CARGO_BIN_EXE_child").into(), mode.into()],
        cwd: std::env::temp_dir().to_string_lossy().into(),
        stdio: SubprocessStdio {
            stdin: SubprocessStdinMode::Ignore,
            stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                max_bytes: 4096,
                spill: None,
            }),
            stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                max_bytes: 4096,
                spill: None,
            }),
        },
        grace_ms: 100,
        signal: None,
        env: None,
    }
}

#[tokio::test]
async fn root_done_does_not_release_live_grandchild() {
    let handle = spawn_subprocess(spec("orphan"), SpawnInternals::default()).unwrap();
    assert_eq!(handle.done().await.unwrap().exit_code, Some(0));
    let deadline = Instant::now() + Duration::from_millis(100);
    assert!(
        !handle
            .wait_for_exit(Some(Arc::new(move || Instant::now() >= deadline)))
            .await
    );
    assert!(
        handle
            .collected()
            .stdout
            .unwrap()
            .read_from(0)
            .text
            .contains("grandchild=")
    );
    assert!(
        handle
            .collected()
            .stderr
            .unwrap()
            .read_from(0)
            .text
            .contains("parent completed")
    );
    handle.terminate();
    handle.terminate();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), handle.wait_for_exit(None))
            .await
            .unwrap()
    );
    handle.terminate();
}

#[tokio::test]
async fn cancellation_after_parent_exit_still_cleans_descendants() {
    let canceled = Arc::new(AtomicBool::new(false));
    let flag = canceled.clone();
    let mut spec = spec("orphan");
    spec.signal = Some(Arc::new(move || flag.load(Ordering::SeqCst)));
    let handle = spawn_subprocess(spec, SpawnInternals::default()).unwrap();
    handle.done().await.unwrap();
    canceled.store(true, Ordering::SeqCst);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), handle.wait_for_exit(None))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn runtime_drop_reaps_an_orphaned_execution_tree() {
    let runtime = dsh_subprocess_local::LocalSubprocessRuntime::new();
    let handle = runtime.spawn(spec("orphan")).unwrap();
    handle.done().await.unwrap();
    drop(runtime);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), handle.wait_for_exit(None))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn startup_failure_and_nonzero_exit_are_distinct() {
    let mut missing = spec("exit");
    missing.argv[0] = "dsh-absent-executable-79732".into();
    assert!(spawn_subprocess(missing, SpawnInternals::default()).is_err());
    let mut failed = spec("exit");
    failed.argv.push("23".into());
    let handle = spawn_subprocess(failed, SpawnInternals::default()).unwrap();
    assert_eq!(handle.done().await.unwrap().exit_code, Some(23));
    assert!(handle.wait_for_exit(None).await);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn owned_scope_contains_new_sessions() {
    let probe = spawn_subprocess(spec("sleep"), SpawnInternals::default()).unwrap();
    let capable = !probe.management_backend().starts_with("process-group");
    probe.terminate();
    probe.done().await.unwrap();
    assert!(probe.wait_for_exit(None).await);
    if !capable {
        eprintln!("SKIP setsid containment: no delegated cgroup or user systemd scope");
        return;
    }
    let mut request = spec("orphan");
    request.argv.push("detached".into());
    let handle = spawn_subprocess(request, SpawnInternals::default()).unwrap();
    assert!(!handle.management_backend().starts_with("process-group"));
    handle.done().await.unwrap();
    handle.terminate();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), handle.wait_for_exit(None))
            .await
            .unwrap()
    );
}
