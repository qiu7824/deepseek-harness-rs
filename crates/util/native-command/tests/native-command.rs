//! Rust port of the TS `native-command.spec.ts` suite: utf8 stdio capture,
//! non-zero exit facts, missing-executable ENOENT, and abort termination.

use std::path::PathBuf;
use std::sync::Arc;

use dsh_native_command::{NativeCommandAbort, run_native_command};

fn child_path() -> String {
    PathBuf::from(env!("CARGO_BIN_EXE_native-child"))
        .to_string_lossy()
        .into_owned()
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn captures_utf8_stdout_and_stderr_on_exit_zero() {
    let result = run_native_command(&child_path(), &args(&["echo-out", "out✓"]), None)
        .await
        .expect("exit 0");
    assert_eq!(result.stdout, "out✓");
    assert_eq!(result.stderr, "");
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_nonzero_exit_with_code_and_stdio_attached() {
    let failure = run_native_command(&child_path(), &args(&["exit", "3"]), None)
        .await
        .err()
        .expect("non-zero exit");
    assert_eq!(failure.code.as_deref(), Some("3"));
    assert_eq!(failure.stdout, "");
    assert_eq!(failure.stderr, "");
    assert!(failure.message.contains("code 3"), "{}", failure.message);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_missing_executable_with_the_spawn_enoent_code() {
    let failure = run_native_command("dsh-definitely-missing-command", &[], None)
        .await
        .err()
        .expect("missing executable");
    assert_eq!(failure.code.as_deref(), Some("ENOENT"));
}

#[tokio::test(flavor = "current_thread")]
async fn terminates_the_child_when_the_signal_aborts() {
    let aborted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal: NativeCommandAbort = Arc::new({
        let aborted = aborted.clone();
        move || aborted.load(std::sync::atomic::Ordering::SeqCst)
    });
    let child = child_path();
    let sleep_args = args(&["sleep-forever"]);
    let pending =
        tokio::spawn(async move { run_native_command(&child, &sleep_args, Some(signal)).await });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    aborted.store(true, std::sync::atomic::Ordering::SeqCst);
    let failure = pending.await.expect("task").err().expect("aborted");
    assert_eq!(failure.code.as_deref(), Some("ABORT_ERR"));
}
