//! Rust port of the TS `spawn.spec.ts` core subset for
//! `dsh-subprocess-local`: environment merging, tail-keep/spill collection,
//! spawn dispositions, termination escalation, abort reaction, and executable
//! resolution.
//!
//! Deviations:
//!
//! - The abort predicate is polled, so abort reactions may lag by one 15 ms
//!   tick.
//! - Spawn failures reject the `spawn` call itself instead of producing a
//!   `pid: -1` handle with a rejected `done`.
//! - Signal outcomes are asserted through the mapped name (`SIGTERM`) on
//!   POSIX; Windows termination reports an exit code (no signals).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cordis::Context;

use dsh_subprocess::{
    SubprocessAbort, SubprocessCollect, SubprocessHandle, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessSpill, SubprocessStdinMode, SubprocessStdio,
};
use dsh_subprocess_local::{
    LocalSubprocessRuntime, OutputCollector, SpawnInternals, child_env, spawn_subprocess,
};

fn child_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_child"))
}

fn argv(mode: &[&str]) -> Vec<String> {
    let mut argv = vec![child_path().to_string_lossy().into_owned()];
    argv.extend(mode.iter().map(|arg| (*arg).to_string()));
    argv
}

fn collect_mode(max_bytes: u64, spill: Option<u64>) -> SubprocessOutputMode {
    SubprocessOutputMode::Collect(SubprocessCollect {
        max_bytes,
        spill: spill.map(|max_bytes| SubprocessSpill { max_bytes }),
    })
}

fn spec(
    argv: Vec<String>,
    stdio: SubprocessStdio,
    grace_ms: u64,
    signal: Option<SubprocessAbort>,
) -> SubprocessSpawnSpec {
    SubprocessSpawnSpec {
        argv,
        cwd: std::env::current_dir()
            .expect("current_dir")
            .to_string_lossy()
            .into_owned(),
        stdio,
        grace_ms,
        signal,
        env: None,
    }
}

fn spawn(
    argv: Vec<String>,
    stdio: SubprocessStdio,
    grace_ms: u64,
) -> dsh_subprocess_local::LocalHandle {
    spawn_subprocess(spec(argv, stdio, grace_ms, None), SpawnInternals::default()).expect("spawn")
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsh-subprocess-test-{}-{label}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn empty_stdio() -> SubprocessStdio {
    SubprocessStdio {
        stdin: SubprocessStdinMode::Ignore,
        stdout: SubprocessOutputMode::Pipe,
        stderr: SubprocessOutputMode::Pipe,
    }
}

#[test]
fn child_env_merges_overrides_and_tombstones() {
    let env = child_env(Some(&[
        (
            "DSH_TEST_CHILD_ENV_VALUE".to_string(),
            Some("kept".to_string()),
        ),
        (
            "DSH_TEST_CHILD_ENV_CREDENTIAL".to_string(),
            Some("secret-survives".to_string()),
        ),
        ("PATH".to_string(), None),
    ]));
    assert!(
        env.iter()
            .any(|(key, value)| key == "DSH_TEST_CHILD_ENV_VALUE" && value == "kept")
    );
    // A deliberately supplied credential-shaped entry survives the scrub.
    assert!(env.iter().any(|(key, value)| {
        key == "DSH_TEST_CHILD_ENV_CREDENTIAL" && value == "secret-survives"
    }));
    // The tombstone removes an ordinary ambient entry.
    assert!(!env.iter().any(|(key, _)| key == "PATH"));
    // The scrub itself strips credential-shaped ambient names.
    assert!(!env.iter().any(|(key, _)| key.contains("TOKEN")));
}

#[cfg(windows)]
#[test]
fn child_env_windows_merge_is_case_insensitive() {
    let env = child_env(Some(&[(
        "path".to_string(),
        Some("C:\\overridden".to_string()),
    )]));
    assert!(
        env.iter()
            .any(|(key, value)| key == "path" && value == "C:\\overridden")
    );
    assert_eq!(
        env.iter()
            .filter(|(key, _)| key.to_uppercase() == "PATH")
            .count(),
        1
    );
}

#[test]
fn collector_keeps_byte_exact_tail() {
    let mut collector = OutputCollector::new(10, None, "stdout", temp_dir("tail"));
    collector.push(b"abcdefgh");
    collector.push(b"ijklmnop");
    // 16 pushed, 10 retained: the head chunk is trimmed byte-exactly.
    let read = collector.read_from(0);
    assert_eq!(read.text, "ghijklmnop");
    assert!(read.lossy);
    assert_eq!(read.next_offset, 16);
    assert!(read.spill_path.is_none());
    // Incremental read at offset 9: bytes 9..16 only.
    let read = collector.read_from(9);
    assert_eq!(read.text, "jklmnop");
    assert!(!read.lossy);
    let finalize = collector.finalize();
    assert_eq!(finalize.text, "ghijklmnop");
    assert!(finalize.truncated);
    assert!(finalize.spill_path.is_none());
}

#[test]
fn collector_spills_whole_stream_then_discards_over_cap() {
    let dir = temp_dir("spill");
    let mut collector = OutputCollector::new(10, Some(1000), "stdout", dir.clone());
    collector.push(b"0123456789");
    collector.push(b"ABCDEFGHIJ");
    let read = collector.read_from(0);
    assert_eq!(read.text, "ABCDEFGHIJ");
    assert!(read.lossy);
    let spill_path = read.spill_path.expect("spill path");
    // The spill file holds the complete stream, head first.
    let content = std::fs::read_to_string(&spill_path).expect("read spill");
    assert_eq!(content, "0123456789ABCDEFGHIJ");

    // Exceeding the spill cap discards the file and keeps the tail only.
    collector.push(&vec![b'x'; 2000]);
    let read = collector.read_from(0);
    assert!(read.lossy);
    assert_eq!(read.text, "xxxxxxxxxx");
    assert!(
        read.spill_path.is_none(),
        "spill must be discarded over cap"
    );
    assert!(
        !PathBuf::from(&spill_path).exists(),
        "spill file must be unlinked"
    );
}

#[test]
fn collector_seal_is_idempotent() {
    let dir = temp_dir("seal");
    let mut collector = OutputCollector::new(5, Some(1000), "stdout", dir.clone());
    collector.push(b"hello world");
    let read = collector.read_from(0);
    assert!(read.spill_path.is_some());
    let finalized = collector.finalize();
    assert!(
        finalized.spill_path.is_some(),
        "seal keeps the intact spill path"
    );
    let again = collector.finalize();
    assert_eq!(finalized.text, again.text);
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_collects_stdout_and_stderr() {
    let stdio = SubprocessStdio {
        stdin: SubprocessStdinMode::Ignore,
        stdout: collect_mode(1 << 20, None),
        stderr: collect_mode(1 << 20, None),
    };
    let handle = spawn(argv(&["both", "line", "5"]), stdio, 2_000);
    let outcome = handle.done().await.expect("done");
    assert_eq!(outcome.exit_code, Some(0));
    let collected = handle.collected();
    let stdout = collected.stdout.expect("stdout reader");
    let stderr = collected.stderr.expect("stderr reader");
    let out = stdout.read_from(0);
    assert_eq!(out.text, "line\nline\nline\nline\nline\n");
    let err = stderr.read_from(0);
    assert_eq!(err.text, "line\nline\nline\nline\nline\n");
    // Incremental reads are non-consuming: the next delta is empty.
    let delta = stdout.read_from(out.next_offset);
    assert_eq!(delta.text, "");
    assert!(!delta.lossy);
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_writes_batch_stdin_data() {
    let stdio = SubprocessStdio {
        stdin: SubprocessStdinMode::Data("ping-pong".to_string()),
        stdout: collect_mode(1 << 20, None),
        stderr: SubprocessOutputMode::Pipe,
    };
    let handle = spawn(argv(&["stdin-cat"]), stdio, 2_000);
    let outcome = handle.done().await.expect("done");
    assert_eq!(outcome.exit_code, Some(0));
    let stdout = handle.collected().stdout.expect("stdout reader");
    assert_eq!(stdout.read_from(0).text, "ping-pong");
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_reports_exit_code() {
    let handle = spawn(argv(&["exit", "7"]), empty_stdio(), 2_000);
    let outcome = handle.done().await.expect("done");
    assert_eq!(outcome.exit_code, Some(7));
    assert_eq!(outcome.signal, None);
    assert!(handle.wait_for_exit(None).await);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn spawn_reports_signal_death() {
    let handle = spawn(argv(&["die-sigterm"]), empty_stdio(), 2_000);
    let outcome = handle.done().await.expect("done");
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.signal.as_deref(), Some("SIGTERM"));
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_terminate_stops_a_long_sleeper() {
    let handle = spawn(argv(&["sleep", "60000"]), empty_stdio(), 2_000);
    let pid = handle.pid();
    assert!(pid > 0);
    handle.terminate();
    assert!(handle.wait_for_exit(None).await);
    let outcome = handle.done().await.expect("done");
    #[cfg(unix)]
    {
        assert_eq!(outcome.exit_code, None);
        assert_eq!(outcome.signal.as_deref(), Some("SIGTERM"));
        // The tree is really gone (ESRCH), not just settled.
        unsafe {
            assert_eq!(libc::kill(pid, 0), -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }
    #[cfg(windows)]
    {
        assert!(outcome.exit_code.is_some());
    }
    // done() is idempotent for every awaiter.
    let second = handle.done().await.expect("done again");
    assert_eq!(second.exit_code, outcome.exit_code);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn spawn_terminate_escalates_past_ignored_term() {
    let handle = spawn(argv(&["trap-ignore-term"]), empty_stdio(), 300);
    handle.terminate();
    let started = Instant::now();
    let outcome = handle.done().await.expect("done");
    // The escalation must wait out the full grace before SIGKILL.
    assert!(started.elapsed() >= Duration::from_millis(250));
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.signal.as_deref(), Some("SIGKILL"));
    assert!(handle.wait_for_exit(None).await);
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_reacts_to_abort_predicate() {
    let started = Instant::now();
    let deadline = started + Duration::from_millis(150);
    let abort: SubprocessAbort = Arc::new(move || Instant::now() >= deadline);
    let handle = spawn_subprocess(
        spec(argv(&["sleep", "60000"]), empty_stdio(), 2_000, Some(abort)),
        SpawnInternals::default(),
    )
    .expect("spawn");
    let outcome = handle.done().await.expect("done");
    assert!(handle.wait_for_exit(None).await);
    #[cfg(unix)]
    assert_eq!(outcome.signal.as_deref(), Some("SIGTERM"));
    #[cfg(windows)]
    assert!(outcome.exit_code.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_wait_for_exit_honors_the_signal() {
    let handle = spawn(argv(&["sleep", "60000"]), empty_stdio(), 2_000);
    let aborted: SubprocessAbort = Arc::new(|| true);
    assert!(!handle.wait_for_exit(Some(aborted)).await);
    handle.terminate();
    assert!(handle.wait_for_exit(None).await);
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_rejects_invalid_requests() {
    // graceMs must be positive and timer-representable.
    let error = spawn_subprocess(
        spec(argv(&["sleep", "1"]), empty_stdio(), 0, None),
        SpawnInternals::default(),
    )
    .err()
    .expect("grace 0 must fail");
    assert!(error.contains("graceMs"), "{error}");
    // argv must name a program.
    let error = spawn_subprocess(
        spec(vec![], empty_stdio(), 1_000, None),
        SpawnInternals::default(),
    )
    .err()
    .expect("empty argv must fail");
    assert!(error.contains("invalid argv"), "{error}");
    // An already-aborted signal fails before spawn.
    let aborted: SubprocessAbort = Arc::new(|| true);
    let error = spawn_subprocess(
        spec(argv(&["sleep", "1"]), empty_stdio(), 1_000, Some(aborted)),
        SpawnInternals::default(),
    )
    .err()
    .expect("aborted spawn must fail");
    assert!(error.contains("aborted before spawn"), "{error}");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn spawn_tree_termination_reaches_descendants() {
    // The child spawns a sleeping grandchild and exits; done settles at the
    // direct child's exit while the tree (grandchild) is still alive.
    let handle = spawn(argv(&["spawn-then-wait", "300"]), empty_stdio(), 2_000);
    let outcome = handle.done().await.expect("done");
    assert_eq!(outcome.exit_code, Some(0));
    handle.terminate();
    // Whole-tree exit: the surviving grandchild must be gone too.
    assert!(handle.wait_for_exit(None).await);
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_resolves_executables() {
    let runtime = LocalSubprocessRuntime::new();
    let executable = std::env::current_exe().expect("current_exe");
    let absolute = executable.to_string_lossy().into_owned();

    // Absolute path resolves to itself.
    let resolved = runtime
        .resolve_executable(&absolute, None, None)
        .await
        .expect("resolve absolute");
    assert_eq!(resolved, absolute);

    // A bare file name resolves through an explicit PATH entry.
    let directory = executable
        .parent()
        .expect("parent dir")
        .to_string_lossy()
        .into_owned();
    let name = executable
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .into_owned();
    let env = vec![("PATH".to_string(), directory.clone())];
    let resolved = runtime
        .resolve_executable(&name, Some(&env), None)
        .await
        .expect("resolve bare name");
    assert!(resolved.starts_with(&directory), "{resolved}");

    // Empty commands and relative paths fail loud.
    let error = runtime
        .resolve_executable("", None, None)
        .await
        .expect_err("empty");
    assert!(error.contains("non-empty"), "{error}");
    let error = runtime
        .resolve_executable("dir/command", None, None)
        .await
        .expect_err("relative");
    assert!(error.contains("relative path"), "{error}");

    // A missing name reports one stable PATH error.
    let error = runtime
        .resolve_executable("definitely-not-a-real-command", Some(&env), None)
        .await
        .expect_err("missing");
    assert!(error.contains("was not found on PATH"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_spawns_and_disposes_through_the_service() {
    let ctx = Context::root();
    // Effects only drain through plugin fibers; the root fiber's dispose is
    // a no-op by design.
    let fiber = ctx.plugin(Arc::new(SubprocessTestPlugin), cordis::arc(()));
    fiber.settle().await.expect("plugin loads");
    let service = ctx
        .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
        .map(|slot| slot.as_ref().clone())
        .expect("registered subprocess service");

    let handle = service
        .spawn(spec(argv(&["sleep", "60000"]), empty_stdio(), 2_000, None))
        .expect("spawn through service");
    // Dispose the plugin fiber while the child runs: normal disposal must
    // terminate and join the whole tree.
    let done = handle.done();
    fiber.dispose().await;
    let outcome = done.await.ok();
    assert!(outcome.is_some(), "disposal must settle the spawned child");
    assert!(handle.wait_for_exit(None).await);
}

#[tokio::test(flavor = "current_thread")]
async fn disposed_runtime_rejects_new_process_spawns() {
    let ctx = Context::root();
    let fiber = ctx.plugin(Arc::new(SubprocessTestPlugin), cordis::arc(()));
    fiber.settle().await.expect("plugin loads");
    let service = ctx
        .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
        .map(|slot| slot.as_ref().clone())
        .expect("registered subprocess service");

    fiber.dispose().await;
    let error = service
        .spawn(spec(
            argv(&["node", "-e", "process.exit(0)"]),
            empty_stdio(),
            2_000,
            None,
        ))
        .err()
        .expect("disposed runtime must reject spawn");
    assert!(error.contains("closing"), "{error}");
}

/// Minimal plugin: installs the local subprocess runtime in its own fiber
/// context so the test can exercise the teardown effect.
struct SubprocessTestPlugin;

#[async_trait::async_trait]
impl cordis::Plugin for SubprocessTestPlugin {
    async fn apply(
        &self,
        ctx: &Context,
        _config: cordis::ArcValue,
    ) -> Result<(), cordis::PluginError> {
        let _ = LocalSubprocessRuntime::install(ctx);
        Ok(())
    }
}
