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

use tokio::io::{AsyncRead, AsyncReadExt};
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

/// Hard resource boundaries for a captured native command. Both byte limits
/// are checked while the pipes are drained, before data is appended.
#[derive(Debug, Clone, Copy)]
pub struct NativeCommandLimits {
    pub timeout: Duration,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

enum StreamReadFailure {
    OutputLimit {
        stream: &'static str,
        limit: usize,
    },
    Io {
        stream: &'static str,
        error: std::io::Error,
    },
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    stream: &'static str,
    limit: usize,
) -> Result<Vec<u8>, StreamReadFailure> {
    let mut output = Vec::new();
    let mut chunk = vec![0_u8; 16 * 1024];
    loop {
        let count = reader
            .read(&mut chunk)
            .await
            .map_err(|error| StreamReadFailure::Io { stream, error })?;
        if count == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(count) > limit {
            return Err(StreamReadFailure::OutputLimit { stream, limit });
        }
        output.extend_from_slice(&chunk[..count]);
    }
}

async fn terminate_and_reap(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn spawn_command(
    command: &str,
    args: &[String],
) -> Result<tokio::process::Child, NativeCommandFailure> {
    let mut spawned = Command::new(command);
    spawned
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        spawned.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    spawned.spawn().map_err(|error| NativeCommandFailure {
        message: error.to_string(),
        code: (error.kind() == std::io::ErrorKind::NotFound).then(|| "ENOENT".to_string()),
        stdout: String::new(),
        stderr: String::new(),
    })
}

/// Run a native command while concurrently draining both pipes under hard
/// timeout and output limits. Timeout, cancellation, output overflow, and IO
/// failures actively terminate and reap the child before returning.
pub async fn run_native_command_bounded(
    command: &str,
    args: &[String],
    signal: Option<NativeCommandAbort>,
    limits: NativeCommandLimits,
) -> Result<NativeCommandOutput, NativeCommandFailure> {
    let mut child = spawn_command(command, args)?;
    let stdout = child.stdout.take().expect("native command stdout is piped");
    let stderr = child.stderr.take().expect("native command stderr is piped");
    let stdout_read = read_bounded(stdout, "stdout", limits.stdout_bytes);
    let stderr_read = read_bounded(stderr, "stderr", limits.stderr_bytes);
    tokio::pin!(stdout_read);
    tokio::pin!(stderr_read);
    let deadline = tokio::time::sleep(limits.timeout);
    tokio::pin!(deadline);
    let mut abort_poll = tokio::time::interval(Duration::from_millis(ABORT_POLL_MS));
    abort_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut stdout_bytes = None;
    let mut stderr_bytes = None;
    let mut status = None;

    loop {
        tokio::select! {
            result = &mut stdout_read, if stdout_bytes.is_none() => match result {
                Ok(value) => stdout_bytes = Some(value),
                Err(StreamReadFailure::OutputLimit { stream, limit }) => {
                    terminate_and_reap(&mut child).await;
                    return Err(NativeCommandFailure {
                        message: format!("native command {stream} exceeded {limit} bytes"),
                        code: Some("OUTPUT_LIMIT".to_string()),
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
                Err(StreamReadFailure::Io { stream, error }) => {
                    terminate_and_reap(&mut child).await;
                    return Err(NativeCommandFailure {
                        message: format!("failed to read native command {stream}: {error}"),
                        code: Some("OUTPUT_IO".to_string()),
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
            },
            result = &mut stderr_read, if stderr_bytes.is_none() => match result {
                Ok(value) => stderr_bytes = Some(value),
                Err(StreamReadFailure::OutputLimit { stream, limit }) => {
                    terminate_and_reap(&mut child).await;
                    return Err(NativeCommandFailure {
                        message: format!("native command {stream} exceeded {limit} bytes"),
                        code: Some("OUTPUT_LIMIT".to_string()),
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
                Err(StreamReadFailure::Io { stream, error }) => {
                    terminate_and_reap(&mut child).await;
                    return Err(NativeCommandFailure {
                        message: format!("failed to read native command {stream}: {error}"),
                        code: Some("OUTPUT_IO".to_string()),
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
            },
            result = child.wait(), if status.is_none() => {
                match result {
                    Ok(value) => status = Some(value),
                    Err(error) => {
                        terminate_and_reap(&mut child).await;
                        return Err(NativeCommandFailure {
                            message: error.to_string(),
                            code: Some("WAIT_IO".to_string()),
                            stdout: String::new(),
                            stderr: String::new(),
                        });
                    }
                }
            }
            _ = &mut deadline => {
                terminate_and_reap(&mut child).await;
                return Err(NativeCommandFailure {
                    message: format!("native command timed out after {} ms", limits.timeout.as_millis()),
                    code: Some("TIMEOUT".to_string()),
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            _ = abort_poll.tick(), if signal.is_some() => {
                if signal.as_ref().is_some_and(|cancelled| cancelled()) {
                    terminate_and_reap(&mut child).await;
                    return Err(NativeCommandFailure {
                        message: "native command aborted".to_string(),
                        code: Some("ABORT_ERR".to_string()),
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
            }
        }
        if status.is_some() && stdout_bytes.is_some() && stderr_bytes.is_some() {
            break;
        }
    }

    let stdout = String::from_utf8_lossy(stdout_bytes.as_deref().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(stderr_bytes.as_deref().unwrap_or_default()).into_owned();
    match status.and_then(|status| status.code()) {
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
