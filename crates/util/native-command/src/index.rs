//! Shared no-shell command runner for host-native OS integrations (the
//! native directory chooser, the open-with-default-application hand-off):
//! utf8 stdio capture, abort propagation, Windows console hide. A library,
//! not a plugin — no ctx, no state, no events. Rust port of
//! `packages/util/native-command/src/index.ts`.
//!
//! # Deviations
//!
//! - `AbortSignal` collapses into the repo-wide cancellation predicate
//!   ([`NativeCommandAbort`]), polled every 15 ms.
//! - Node's `error.code` (`ENOENT`, numeric exit codes, `ABORT_ERR`)
//!   collapses into [`NativeCommandFailure::code`] as an optional string.

use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;

/// The abort/cancellation predicate (the TS `AbortSignal` collapse).
pub type NativeCommandAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// Captured stdio of one successful run (TS `{ stdout, stderr }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCommandOutput {
    pub stdout: String,
    pub stderr: String,
}

/// A failed run: non-zero exit, missing executable, or abort. Carries the
/// Node-shaped `code` (`ENOENT`, the numeric exit code, or `ABORT_ERR`),
/// captured stdio, and the platform message (TS the rejected `Error`).
#[derive(Debug, Clone)]
pub struct NativeCommandFailure {
    pub message: String,
    pub code: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

impl std::fmt::Display for NativeCommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NativeCommandFailure {}

const ABORT_POLL_MS: u64 = 15;

/// Run a host command with utf8 stdio, abort propagation, and Windows hide
/// (TS `runNativeCommand`). No shell interpretation; argv is verbatim.
pub async fn run_native_command(
    command: &str,
    args: &[String],
    signal: Option<NativeCommandAbort>,
) -> Result<NativeCommandOutput, NativeCommandFailure> {
    let mut spawned = Command::new(command);
    spawned
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // The TS `windowsHide: true` equivalent: no console window.
        spawned.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let child = match spawned.spawn() {
        Ok(child) => child,
        Err(error) => {
            let code = if error.kind() == std::io::ErrorKind::NotFound {
                Some("ENOENT".to_string())
            } else {
                None
            };
            return Err(NativeCommandFailure {
                message: error.to_string(),
                code,
                stdout: String::new(),
                stderr: String::new(),
            });
        }
    };

    // `wait_with_output` drains both pipes while the child is running. Race it
    // against the cancellation predicate; `kill_on_drop` terminates the child
    // when the output future loses the race.
    let output = if let Some(signal) = signal {
        let output = child.wait_with_output();
        tokio::pin!(output);
        loop {
            tokio::select! {
                result = &mut output => {
                    break result.map_err(|error| NativeCommandFailure {
                        message: error.to_string(),
                        code: None,
                        stdout: String::new(),
                        stderr: String::new(),
                    })?;
                }
                _ = tokio::time::sleep(Duration::from_millis(ABORT_POLL_MS)) => {
                    if signal() {
                        return Err(NativeCommandFailure {
                            message: "native command aborted".to_string(),
                            code: Some("ABORT_ERR".to_string()),
                            stdout: String::new(),
                            stderr: String::new(),
                        });
                    }
                }
            }
        }
    } else {
        child
            .wait_with_output()
            .await
            .map_err(|error| NativeCommandFailure {
                message: error.to_string(),
                code: None,
                stdout: String::new(),
                stderr: String::new(),
            })?
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    match output.status.code() {
        Some(0) => Ok(NativeCommandOutput { stdout, stderr }),
        Some(code) => Err(NativeCommandFailure {
            message: format!("native command exited with code {code}"),
            code: Some(code.to_string()),
            stdout,
            stderr,
        }),
        None => Err(NativeCommandFailure {
            message: "native command was killed by a signal".to_string(),
            code: None,
            stdout,
            stderr,
        }),
    }
}
