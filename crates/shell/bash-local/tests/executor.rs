//! Rust port of the TS `executor.spec.ts` suite for `dsh-bash-local`:
//! foreground runs (outputs, cwd, timeout caps, cause classification, stdin/
//! env threading) and background process handles (incremental reads, stderr
//! sections, spill paths, kill, escalation, spawn failures).
//!
//! # Deviations
//!
//! - Every test gates on a live `bash` probe (`bash_available()`), so the
//!   suite passes on hosts without a real bash instead of failing at spawn.
//! - POSIX-path, environment-threading, and shell-quoting assertions are
//!   `#[cfg(unix)]`: the Windows WSL bash launcher reports WSL paths, does
//!   not forward the Windows environment, and mangles quoted argument
//!   payloads, so those cases only hold on real POSIX hosts. The
//!   Windows-runnable subset covers quoting-free commands (echo/true/cat/
//!   simple redirects) and the kill/spawn-failure paths.
//! - Process-tree kill semantics (signal names, trap survivors) are POSIX
//!   assertions; on Windows the subprocess backend kills through `taskkill`,
//!   which has no signal vocabulary.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cordis::{Context, Plugin};
use futures::FutureExt;

use dsh_bash_local::LocalBashExecutor;
use dsh_shell::{ShellExecutor, ShellProcess, ShellProcessStatus};
use dsh_subprocess::{
    SubprocessOutputMode, SubprocessRuntime, SubprocessSpawnSpec, SubprocessStdinMode,
    SubprocessStdio,
};
use dsh_subprocess_local::LocalSubprocessRuntime;

/// One-shot check that a real `bash -c true` runs on this host. The probe
/// actually spawns one bash, so a slow WSL/bootstrap first invocation lands
/// here instead of inside a timed test.
async fn bash_available() -> bool {
    let ctx = Context::root();
    LocalSubprocessRuntime::install(&ctx);
    let subprocess = ctx
        .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
        .map(|slot| slot.as_ref().clone())
        .expect("subprocess service");
    let Ok(executable) = subprocess.resolve_executable("bash", None, None).await else {
        return false;
    };
    let spec = SubprocessSpawnSpec {
        argv: vec![executable, "-c".to_string(), "true".to_string()],
        cwd: std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .into_owned(),
        stdio: SubprocessStdio {
            stdin: SubprocessStdinMode::Ignore,
            stdout: SubprocessOutputMode::Pipe,
            stderr: SubprocessOutputMode::Pipe,
        },
        grace_ms: 5_000,
        signal: None,
        env: None,
    };
    match subprocess.spawn(spec) {
        Ok(handle) => handle.done().await.is_ok(),
        Err(_) => false,
    }
}

/// The executor plugin form (the TS `ctx.plugin(LocalBashExecutor, config)`).
/// The installed concrete handle rides a slot (the TS `ctx.shell as
/// LocalBashExecutor` cast equivalent).
struct BashPlugin {
    config: dsh_bash_local::Config,
    slot: Arc<parking_lot::Mutex<Option<Arc<LocalBashExecutor>>>>,
}

#[async_trait::async_trait]
impl Plugin for BashPlugin {
    async fn apply(
        &self,
        ctx: &Context,
        _config: cordis::ArcValue,
    ) -> Result<(), cordis::PluginError> {
        let executor = LocalBashExecutor::install(ctx, self.config.clone());
        executor.ready().await.map_err(|error| error)?;
        *self.slot.lock() = Some(executor);
        Ok(())
    }
}

struct Harness {
    ctx: Context,
    bash: Arc<LocalBashExecutor>,
    _subprocess: Arc<LocalSubprocessRuntime>,
}

async fn setup(config: dsh_bash_local::Config) -> Harness {
    let ctx = Context::root();
    let subprocess = LocalSubprocessRuntime::install(&ctx);
    let slot = Arc::new(parking_lot::Mutex::new(None));
    let fiber = ctx.plugin(
        Arc::new(BashPlugin {
            config,
            slot: slot.clone(),
        }),
        cordis::arc(()),
    );
    fiber.settle().await.expect("bash executor loads");
    let bash = slot.lock().take().expect("executor installed");
    Harness {
        ctx,
        bash,
        _subprocess: subprocess,
    }
}

fn config() -> dsh_bash_local::Config {
    dsh_bash_local::Config {
        grace_ms: Some(200),
        ..Default::default()
    }
}

/// Poll a handle's consuming readOutput until the ACCUMULATED delta contains
/// `expected` (reads never re-deliver). The generous window absorbs slow
/// WSL/bootstrap spawns.
async fn read_until(proc: &Arc<dyn ShellProcess>, expected: &str, timeout_ms: u64) -> String {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut all = String::new();
    while Instant::now() < deadline {
        all.push_str(&proc.read_output().delta);
        if all.contains(expected) {
            return all;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "process output did not include {:?}; accumulated {:?}",
        expected, all
    )
}

#[tokio::test(flavor = "current_thread")]
async fn resolves_with_output_and_the_effective_timeout() {
    if !bash_available().await {
        return;
    }
    let mut entry = config();
    entry.timeout_ms = Some(5_000);
    let harness = setup(entry).await;
    let result = harness
        .bash
        .run(
            harness
                .bash
                .resolve(dsh_shell::ShellExecRequest::new("echo hi")),
        )
        .await
        .expect("run");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.text, "hi\n");
    assert_eq!(result.timeout_ms, 5_000);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn uses_config_cwd_overridable_per_call() {
    if !bash_available().await {
        return;
    }
    let mut entry = config();
    entry.cwd = Some("/tmp".to_string());
    let harness = setup(entry).await;
    let from_config = harness
        .bash
        .run(
            harness
                .bash
                .resolve(dsh_shell::ShellExecRequest::new("pwd")),
        )
        .await
        .expect("run");
    assert!(
        from_config.stdout.text.trim().ends_with("/tmp"),
        "{}",
        from_config.stdout.text
    );
    let mut request = dsh_shell::ShellExecRequest::new("pwd");
    request.workdir = Some("/".to_string());
    let from_call = harness
        .bash
        .run(harness.bash.resolve(request))
        .await
        .expect("run");
    assert_eq!(from_call.stdout.text.trim(), "/");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn defaults_cwd_to_process_cwd() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let result = harness
        .bash
        .run(
            harness
                .bash
                .resolve(dsh_shell::ShellExecRequest::new("pwd")),
        )
        .await
        .expect("run");
    let expected = std::env::current_dir()
        .expect("cwd")
        .to_string_lossy()
        .into_owned();
    assert_eq!(result.stdout.text.trim(), expected);
}

#[tokio::test(flavor = "current_thread")]
async fn caps_per_call_timeouts_at_max_timeout_ms() {
    if !bash_available().await {
        return;
    }
    let mut entry = config();
    entry.timeout_ms = Some(1_000);
    entry.max_timeout_ms = Some(2_000);
    let harness = setup(entry).await;
    let mut request = dsh_shell::ShellExecRequest::new("true");
    request.timeout_ms = Some(99_999);
    let result = harness
        .bash
        .run(harness.bash.resolve(request))
        .await
        .expect("run");
    assert_eq!(result.timeout_ms, 2_000);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_invalid_numeric_config_and_timeout_overrides() {
    // Config validation runs at construction (the TS constructor throw).
    for (label, entry) in [
        (
            "timeoutMs",
            dsh_bash_local::Config {
                timeout_ms: Some(0),
                ..Default::default()
            },
        ),
        (
            "maxTimeoutMs",
            dsh_bash_local::Config {
                max_timeout_ms: Some(0),
                ..Default::default()
            },
        ),
        (
            "maxOutputBytes",
            dsh_bash_local::Config {
                max_output_bytes: Some(0),
                ..Default::default()
            },
        ),
        (
            "maxSpillBytes",
            dsh_bash_local::Config {
                max_spill_bytes: Some(0),
                ..Default::default()
            },
        ),
        (
            "graceMs",
            dsh_bash_local::Config {
                grace_ms: Some(0),
                ..Default::default()
            },
        ),
    ] {
        let error = dsh_bash_local::ResolvedConfig::resolve(&entry)
            .err()
            .expect("invalid entry");
        assert!(error.contains(label), "{error}");
    }
    let error = dsh_bash_local::ResolvedConfig::resolve(&dsh_bash_local::Config {
        grace_ms: Some(dsh_timeout::MAX_TIMER_DELAY_MS + 1),
        ..Default::default()
    })
    .err()
    .expect("grace bound");
    assert!(
        error.contains(&format!(
            "graceMs must be no greater than {}",
            dsh_timeout::MAX_TIMER_DELAY_MS
        )),
        "{error}"
    );

    // Request-level validation runs in resolve().
    let ctx = Context::root();
    LocalSubprocessRuntime::install(&ctx);
    let bash = LocalBashExecutor::install(&ctx, config());
    for invalid in [0u64] {
        let mut request = dsh_shell::ShellExecRequest::new("true");
        request.timeout_ms = Some(invalid);
        let error =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bash.resolve(request)))
                .err()
                .expect("request.timeoutMs rejection");
        assert!(render_panic(&error).contains("request.timeoutMs"));
    }
    let mut request = dsh_shell::ShellExecRequest::new("true");
    request.stdout_max_bytes = Some(0);
    let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bash.resolve(request)))
        .err()
        .expect("request.stdoutMaxBytes rejection");
    assert!(render_panic(&error).contains("request.stdoutMaxBytes"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn defaults_stdout_max_bytes_and_lets_foreground_callers_raise_stdout_only() {
    if !bash_available().await {
        return;
    }
    let mut entry = config();
    entry.max_output_bytes = Some(100);
    let harness = setup(entry).await;
    let resolved = harness
        .bash
        .resolve(dsh_shell::ShellExecRequest::new("true"));
    assert_eq!(resolved.stdout_max_bytes, 100);

    let mut request = dsh_shell::ShellExecRequest::new(
        r#"printf "%.0sx" $(seq 1 500); printf "%.0se" $(seq 1 500) >&2"#,
    );
    request.stdout_max_bytes = Some(500);
    let result = harness
        .bash
        .run(harness.bash.resolve(request))
        .await
        .expect("run");
    assert!(!result.stdout.truncated);
    assert_eq!(result.stdout.text, "x".repeat(500));
    assert!(result.stderr.truncated);
    assert!(result.stderr.text.len() <= 100);
}

#[tokio::test(flavor = "current_thread")]
async fn per_call_timeout_takes_precedence_under_the_cap_and_kills_on_expiry() {
    if !bash_available().await {
        return;
    }
    let mut entry = config();
    entry.timeout_ms = Some(60_000);
    let harness = setup(entry).await;
    let mut request = dsh_shell::ShellExecRequest::new("sleep 60");
    request.timeout_ms = Some(100);
    let started = Instant::now();
    let result = harness
        .bash
        .run(harness.bash.resolve(request))
        .await
        .expect("run");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "timeout must kill the sleep"
    );
    assert!(result.timed_out);
    // Mutually exclusive: a timeout classifies as timedOut, never also aborted.
    assert!(!result.aborted);
    assert_eq!(result.timeout_ms, 100);
}

#[tokio::test(flavor = "current_thread")]
async fn propagates_abort_signals() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let aborted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal: dsh_shell::ShellAbort = Arc::new({
        let aborted = aborted.clone();
        move || aborted.load(std::sync::atomic::Ordering::SeqCst)
    });
    let mut request = dsh_shell::ShellExecRequest::new("sleep 60");
    request.signal = Some(signal);
    let pending = harness.bash.run(harness.bash.resolve(request));
    let pending = tokio::spawn(pending);
    tokio::time::sleep(Duration::from_millis(50)).await;
    aborted.store(true, std::sync::atomic::Ordering::SeqCst);
    let result = pending.await.expect("task").expect("run");
    assert!(result.aborted);
    // Mutually exclusive: an upstream cancel classifies as aborted, never
    // also timedOut.
    assert!(!result.timed_out);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn classifies_a_self_killed_command_as_neither_timed_out_nor_aborted() {
    if !bash_available().await {
        return;
    }
    let mut entry = config();
    entry.timeout_ms = Some(60_000);
    let harness = setup(entry).await;
    let result = harness
        .bash
        .run(
            harness
                .bash
                .resolve(dsh_shell::ShellExecRequest::new("kill -TERM $$")),
        )
        .await
        .expect("run");
    assert_eq!(result.signal.as_deref(), Some("SIGTERM"));
    assert!(!result.timed_out);
    assert!(!result.aborted);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_on_spawn_failure_bad_workdir() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let mut request = dsh_shell::ShellExecRequest::new("true");
    request.workdir = Some("/nonexistent-dsh".to_string());
    let error = harness
        .bash
        .run(harness.bash.resolve(request))
        .await
        .err()
        .expect("spawn failure rejects");
    assert!(error.contains("spawn"), "{error}");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn resolve_carries_stdin_env_dsh_env_onto_the_spec_and_run_threads_them() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let mut request = dsh_shell::ShellExecRequest::new("cat; echo \"[$SEAM_VAR][$DSH_SEAM_VAR]\"");
    request.stdin = Some("piped\n".to_string());
    request.env = Some(vec![("SEAM_VAR".to_string(), "env-ok".to_string())]);
    request.dsh_env = Some(vec![("DSH_SEAM_VAR".to_string(), "dsh-ok".to_string())]);
    let spec = harness.bash.resolve(request);
    // resolve() keeps the optional input/environment fields verbatim.
    assert_eq!(spec.stdin.as_deref(), Some("piped\n"));
    assert_eq!(
        spec.env.as_ref().map(|entries| entries.as_slice()),
        Some(&[("SEAM_VAR".to_string(), "env-ok".to_string())][..])
    );
    assert_eq!(
        spec.dsh_env.as_ref().map(|entries| entries.as_slice()),
        Some(&[("DSH_SEAM_VAR".to_string(), "dsh-ok".to_string())][..])
    );
    let result = harness.bash.run(spec).await.expect("run");
    assert_eq!(result.stdout.text, "piped\n[env-ok][dsh-ok]\n");
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_omits_stdin_env_dsh_env_when_the_request_supplies_none() {
    let harness = setup(config()).await;
    let spec = harness
        .bash
        .resolve(dsh_shell::ShellExecRequest::new("true"));
    assert!(spec.stdin.is_none());
    assert!(spec.env.is_none());
    assert!(spec.dsh_env.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn start_returns_immediately_with_a_running_handle_that_settles_as_completed() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let before = Instant::now();
    let proc = harness.bash.start(
        harness
            .bash
            .resolve(dsh_shell::ShellExecRequest::new("sleep 0.2; echo done")),
    );
    assert!(before.elapsed() < Duration::from_millis(150));
    assert_eq!(proc.status(), ShellProcessStatus::Running);
    proc.done().await;
    assert_eq!(proc.status(), ShellProcessStatus::Completed);
    assert_eq!(proc.exit_code(), Some(0));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn threads_stdin_and_extra_env_into_a_background_process() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let mut request = dsh_shell::ShellExecRequest::new("cat; echo \"[$BG_VAR][$DSH_BG_VAR]\"");
    request.stdin = Some("bg-stdin\n".to_string());
    request.env = Some(vec![("BG_VAR".to_string(), "bg-env".to_string())]);
    request.dsh_env = Some(vec![("DSH_BG_VAR".to_string(), "bg-dsh-env".to_string())]);
    let proc = harness.bash.start(harness.bash.resolve(request));
    let output = read_until(&proc, "[bg-env][bg-dsh-env]", 5_000).await;
    assert!(output.contains("bg-stdin"));
    proc.done().await;
    assert_eq!(proc.exit_code(), Some(0));
}

#[tokio::test(flavor = "current_thread")]
async fn read_output_is_consuming_and_reads_stay_valid_after_exit() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let proc = harness
        .bash
        .start(harness.bash.resolve(dsh_shell::ShellExecRequest::new(
            "echo first; sleep 1; echo second",
        )));
    let first = read_until(&proc, "first\n", 20_000).await;
    assert_eq!(first, "first\n");
    proc.done().await;
    // Read-after-exit returns the remaining buffered output 鈥?once.
    let second = proc.read_output();
    assert_eq!(second.delta, "second\n");
    assert!(!second.lossy);
    assert_eq!(proc.read_output().delta, "");
}

#[tokio::test(flavor = "current_thread")]
async fn read_output_marks_stderr_sections() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let proc = harness.bash.start(
        harness
            .bash
            .resolve(dsh_shell::ShellExecRequest::new("echo out; echo err >&2")),
    );
    proc.done().await;
    assert_eq!(proc.read_output().delta, "out\n[stderr]\nerr\n");
}

#[tokio::test(flavor = "current_thread")]
async fn read_output_reports_stderr_only_deltas_without_a_leading_newline() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let proc = harness.bash.start(
        harness
            .bash
            .resolve(dsh_shell::ShellExecRequest::new("echo err >&2")),
    );
    proc.done().await;
    assert_eq!(proc.read_output().delta, "[stderr]\nerr\n");
}

#[tokio::test(flavor = "current_thread")]
async fn read_output_adds_a_separator_only_when_stdout_lacks_a_trailing_newline() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let proc = harness.bash.start(
        harness
            .bash
            .resolve(dsh_shell::ShellExecRequest::new("printf out; echo err >&2")),
    );
    proc.done().await;
    assert_eq!(proc.read_output().delta, "out\n[stderr]\nerr\n");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn read_output_flags_lossy_reads_and_reports_stdout_spill_paths() {
    if !bash_available().await {
        return;
    }
    let mut entry = config();
    entry.max_output_bytes = Some(100);
    let harness = setup(entry).await;
    let proc = harness
        .bash
        .start(harness.bash.resolve(dsh_shell::ShellExecRequest::new(
            "for i in $(seq 1 100); do printf \"line-%04d\\n\" $i; done",
        )));
    proc.done().await;
    let read = proc.read_output();
    // Window slid past offset 0 鈫?lossy, spill path points at the full stream.
    assert!(read.lossy);
    assert!(read.stdout_spill_path.is_some(), "stdout spill path");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn read_output_reports_stderr_spill_paths() {
    if !bash_available().await {
        return;
    }
    let mut entry = config();
    entry.max_output_bytes = Some(100);
    let harness = setup(entry).await;
    let proc = harness
        .bash
        .start(harness.bash.resolve(dsh_shell::ShellExecRequest::new(
            "for i in $(seq 1 100); do printf \"line-%04d\\n\" $i >&2; done",
        )));
    proc.done().await;
    let read = proc.read_output();
    assert!(read.lossy);
    assert!(read.stderr_spill_path.is_some(), "stderr spill path");
    assert!(read.delta.contains("[stderr]"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn kill_terminates_the_process_group_true_once_false_after_settlement() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let proc = harness.bash.start(
        harness
            .bash
            .resolve(dsh_shell::ShellExecRequest::new("sleep 60")),
    );
    assert!(proc.kill());
    proc.done().await;
    assert_eq!(proc.status(), ShellProcessStatus::Killed);
    assert_eq!(proc.signal().as_deref(), Some("SIGTERM"));
    assert!(!proc.kill());
}

#[tokio::test(flavor = "current_thread")]
async fn kill_returns_false_for_a_naturally_completed_process() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let proc = harness.bash.start(
        harness
            .bash
            .resolve(dsh_shell::ShellExecRequest::new("true")),
    );
    proc.done().await;
    assert_eq!(proc.status(), ShellProcessStatus::Completed);
    assert!(!proc.kill());
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn kill_escalation_uses_the_configured_grace_ms() {
    if !bash_available().await {
        return;
    }
    // setup pins graceMs: 200 via config; the child echoes AFTER arming the
    // trap, so SIGTERM is already ignored when the kill lands.
    let harness = setup(config()).await;
    let proc = harness
        .bash
        .start(harness.bash.resolve(dsh_shell::ShellExecRequest::new(
            "trap '' TERM; echo armed; sleep 60",
        )));
    read_until(&proc, "armed", 5_000).await;
    proc.kill();
    proc.done().await;
    assert_eq!(proc.status(), ShellProcessStatus::Killed);
    assert_eq!(proc.signal().as_deref(), Some("SIGKILL"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_spec_signal_abort_settles_the_handle_as_killed_not_completed() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let aborted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal: dsh_shell::ShellAbort = Arc::new({
        let aborted = aborted.clone();
        move || aborted.load(std::sync::atomic::Ordering::SeqCst)
    });
    let mut request = dsh_shell::ShellExecRequest::new("sleep 60");
    request.signal = Some(signal);
    let proc = harness.bash.start(harness.bash.resolve(request));
    aborted.store(true, std::sync::atomic::Ordering::SeqCst);
    proc.done().await;
    assert_eq!(proc.status(), ShellProcessStatus::Killed);
    assert_eq!(proc.signal().as_deref(), Some("SIGTERM"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_self_signal_exit_settles_the_handle_as_killed_not_completed() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let proc = harness.bash.start(
        harness
            .bash
            .resolve(dsh_shell::ShellExecRequest::new("kill -TERM $$")),
    );
    proc.done().await;
    assert_eq!(proc.status(), ShellProcessStatus::Killed);
    assert_eq!(proc.exit_code(), None);
    assert_eq!(proc.signal().as_deref(), Some("SIGTERM"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_background_spawn_failure_settles_as_killed_with_the_error_readable_on_stderr() {
    if !bash_available().await {
        return;
    }
    let harness = setup(config()).await;
    let mut request = dsh_shell::ShellExecRequest::new("true");
    request.workdir = Some("/nonexistent-dsh".to_string());
    let proc = harness.bash.start(harness.bash.resolve(request));
    // done resolves (never rejects) even though the process never ran.
    proc.done().await;
    assert_eq!(proc.status(), ShellProcessStatus::Killed);
    let read = proc.read_output();
    assert!(read.delta.contains("spawn failed:"), "{}", read.delta);
}

fn render_panic(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic>".to_string()
    }
}
