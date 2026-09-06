use std::process::Command;
use std::time::{Duration, Instant};

use dsh_native_command::{NativeCommandLimits, run_native_command_bounded};

const CHILD: &str = env!("CARGO_BIN_EXE_native-child");

fn process_is_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let output = Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .expect("query child process");
        String::from_utf8_lossy(&output.stdout).contains(&format!(",\"{pid}\","))
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }
}

#[tokio::test]
async fn bounded_runner_drains_both_pipes_and_enforces_output_limit() {
    let output = run_native_command_bounded(
        CHILD,
        &["large-stdio".to_string()],
        None,
        NativeCommandLimits {
            timeout: Duration::from_secs(5),
            stdout_bytes: 2 * 1024 * 1024,
            stderr_bytes: 2 * 1024 * 1024,
        },
    )
    .await
    .expect("parallel drains avoid stdout/stderr deadlock");
    assert_eq!(output.stdout.len(), 1024 * 1024);
    assert_eq!(output.stderr.len(), 1024 * 1024);

    let started = Instant::now();
    let failure = run_native_command_bounded(
        CHILD,
        &["large-stdio".to_string()],
        None,
        NativeCommandLimits {
            timeout: Duration::from_secs(5),
            stdout_bytes: 4 * 1024,
            stderr_bytes: 4 * 1024,
        },
    )
    .await
    .expect_err("large output must exceed a pipe limit");
    assert_eq!(failure.code.as_deref(), Some("OUTPUT_LIMIT"));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn bounded_runner_kills_and_reaps_on_timeout() {
    let pid_file = std::env::temp_dir().join(format!(
        "dsh-native-command-timeout-{}.pid",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&pid_file);
    let started = Instant::now();
    let failure = run_native_command_bounded(
        CHILD,
        &[
            "sleep-forever".to_string(),
            pid_file.to_string_lossy().into_owned(),
        ],
        None,
        NativeCommandLimits {
            timeout: Duration::from_secs(2),
            stdout_bytes: 1024,
            stderr_bytes: 1024,
        },
    )
    .await
    .expect_err("sleeping process must time out");
    assert_eq!(failure.code.as_deref(), Some("TIMEOUT"));
    assert!(started.elapsed() < Duration::from_secs(5));
    let pid = std::fs::read_to_string(&pid_file)
        .expect("sleep fixture writes its pid before timeout")
        .trim()
        .parse::<u32>()
        .expect("fixture pid");
    assert!(
        !process_is_alive(pid),
        "timed-out child {pid} was not reaped"
    );
    let _ = std::fs::remove_file(pid_file);
}
