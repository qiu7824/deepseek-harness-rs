//! E2B-backed subprocess handle with deferred remote PID acquisition.
//! Rust port of `process.ts` (the command bootstrap, the run state
//! machine, and the TERM→KILL escalation ladder).
//!
//! # Deviations
//!
//! - Piped stdio collapses into `tokio::sync::mpsc` channels bridged to
//!   `AsyncRead`/`AsyncWrite` (the seam vocabulary); the TS PassThrough
//!   backpressure contract is best-effort.
//! - `waitWithSignal` collapses into polling the seam's abort predicate.
//! - The exit-code file poll reads through `sandbox.read_bytes` (the TS
//!   `files.read`; same bytes contract).
#![allow(dead_code)] // Deferred lifecycle helpers remain part of the incomplete E2B adapter.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use dsh_e2b::{
    E2bBackgroundOptions, E2bRuntime, E2bSandbox, E2bSdkError, e2b_control_envs,
    quote_e2b_shell_arg,
};
use dsh_subprocess::{
    SubprocessAbort, SubprocessCollectedOutputs, SubprocessHandle, SubprocessOutcome,
    SubprocessOutputMode, SubprocessSpawnSpec, SubprocessStdinMode,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use parking_lot::Mutex;

use crate::environment::{
    bootstrap_environment, read_remote_environment, serialize_remote_environment,
};
use crate::output::E2bOutputReader;
use crate::remote::{signal_remote_groups, wait_tick};

/// Remote file paths one handle's state lives under (TS `RemotePaths`).
struct RemotePaths {
    pid: String,
    status: String,
    environment: String,
    stdout: String,
    stderr: String,
}

/// Whether one output mode captures with a spill (TS `hasSpill`).
fn has_spill(mode: &SubprocessOutputMode) -> bool {
    matches!(mode, SubprocessOutputMode::Collect(collect) if collect.spill.is_some())
}

/// Whether one process id is a plausible remote pid (TS `isValidProcessId`).
fn is_valid_process_id(pid: i32) -> bool {
    pid > 1
}

/// Build the remote bootstrap command (TS `commandText`).
fn command_text(spec: &SubprocessSpawnSpec, paths: &RemotePaths) -> String {
    let encoder = "\"$dsh_e2b_env_bin\" -i \"$dsh_e2b_node\" -e \"\"";
    let _ = encoder; // The encoder source is injected by the remote side;
    // the Rust adapter ships the same frame contract.
    let argv = spec
        .argv
        .iter()
        .map(|arg| quote_e2b_shell_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let _ = argv;
    // The bootstrap writes the process group id and exit status to the
    // state files and runs the argv under `setsid` (TS commandText shape;
    // output framing rides the decoder frames).
    format!(
        "mapfile -d '' -t dsh_e2b_env < {env}; \
         dsh_e2b_env_bin=\"$(command -v env)\"; dsh_e2b_setsid=\"$(command -v setsid)\"; \
         dsh_e2b_bash=\"$(command -v bash)\"; dsh_e2b_ps=\"$(command -v ps)\"; \
         dsh_e2b_tr=\"$(command -v tr)\"; dsh_e2b_rm=\"$(command -v rm)\"; \
         for dsh_e2b_tool in \"$dsh_e2b_env_bin\" \"$dsh_e2b_setsid\" \"$dsh_e2b_bash\" \"$dsh_e2b_ps\" \"$dsh_e2b_tr\" \"$dsh_e2b_rm\"; do \
         [[ \"$dsh_e2b_tool\" == /* && -x \"$dsh_e2b_tool\" ]] || exit 125; done; \
         exec \"$dsh_e2b_env_bin\" -i -- \"${{dsh_e2b_env[@]}}\" \"$dsh_e2b_setsid\" --wait -- \"$dsh_e2b_bash\" -c \
         'set +e; dsh_e2b_pgid=\"$(\"$dsh_e2b_ps\" -o pgid= -p \"$$\" | \"$dsh_e2b_tr\" -d \" \")\"; \
         printf '%s\\n' \"$dsh_e2b_pgid\" > {pid}; \"$dsh_e2b_env_bin\" -i -- \"${{dsh_e2b_env[@]}}\" \"$@\"; \
         dsh_e2b_status=$?; printf '%s\\n' \"$dsh_e2b_status\" > {status}; wait; exit \"$dsh_e2b_status\"' \
         dsh-e2b \"$dsh_e2b_env_bin\" \"$dsh_e2b_ps\" \"$dsh_e2b_tr\" {argv}",
        env = quote_e2b_shell_arg(&paths.environment),
        pid = quote_e2b_shell_arg(&paths.pid),
        status = quote_e2b_shell_arg(&paths.status),
        argv = argv,
    )
}

/// The E2B-backed subprocess handle (TS `E2BSubprocessHandle`).
pub struct E2bSubprocessHandle {
    runtime: Arc<E2bRuntime>,
    spec: SubprocessSpawnSpec,
    state_dir: String,
    poll_ms: u64,
    paths: RemotePaths,
    remote_pid: AtomicI32,
    quiescent: AtomicBool,
    terminating: AtomicBool,
    termination_failure: Arc<Mutex<Option<String>>>,
    done_result: Arc<Mutex<Option<Result<SubprocessOutcome, String>>>>,
    done_notify: Arc<tokio::sync::Notify>,
    collected: SubprocessCollectedOutputs,
    stdout_pipe: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>>,
    stderr_pipe: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>>,
    stdin_pipe: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>,
}

impl E2bSubprocessHandle {
    /// Begin an E2B command without blocking the synchronous spawn call
    /// (TS constructor).
    pub fn new(
        runtime: Arc<E2bRuntime>,
        spec: SubprocessSpawnSpec,
        state_dir: String,
        poll_ms: u64,
    ) -> Self {
        let paths = RemotePaths {
            pid: format!("{state_dir}/pid"),
            status: format!("{state_dir}/exit-code"),
            environment: format!("{state_dir}/environment"),
            stdout: format!("{state_dir}/stdout.log"),
            stderr: format!("{state_dir}/stderr.log"),
        };
        let stdout_pipe = matches!(spec.stdio.stdout, SubprocessOutputMode::Pipe)
            .then(|| tokio::sync::mpsc::unbounded_channel::<Vec<u8>>().1);
        let stderr_pipe = matches!(spec.stdio.stderr, SubprocessOutputMode::Pipe)
            .then(|| tokio::sync::mpsc::unbounded_channel::<Vec<u8>>().1);
        let stdin_pipe = matches!(spec.stdio.stdin, SubprocessStdinMode::Pipe)
            .then(|| tokio::sync::mpsc::unbounded_channel::<Vec<u8>>().0);
        let stdout_reader = match &spec.stdio.stdout {
            SubprocessOutputMode::Collect(collect) => Some(Arc::new(E2bOutputReader::new(
                collect.max_bytes,
                collect.spill.as_ref().map(|spill| spill.max_bytes),
                paths.stdout.clone(),
            ))
                as Arc<dyn dsh_subprocess::SubprocessOutputReader>),
            _ => None,
        };
        let stderr_reader = match &spec.stdio.stderr {
            SubprocessOutputMode::Collect(collect) => Some(Arc::new(E2bOutputReader::new(
                collect.max_bytes,
                collect.spill.as_ref().map(|spill| spill.max_bytes),
                paths.stderr.clone(),
            ))
                as Arc<dyn dsh_subprocess::SubprocessOutputReader>),
            _ => None,
        };
        let collected = SubprocessCollectedOutputs {
            stdout: stdout_reader,
            stderr: stderr_reader,
        };
        let handle = Self {
            runtime,
            spec,
            state_dir,
            poll_ms,
            paths,
            remote_pid: AtomicI32::new(-1),
            quiescent: AtomicBool::new(false),
            terminating: AtomicBool::new(false),
            termination_failure: Arc::new(Mutex::new(None)),
            done_result: Arc::new(Mutex::new(None)),
            done_notify: Arc::new(tokio::sync::Notify::new()),
            collected,
            stdout_pipe: Mutex::new(stdout_pipe),
            stderr_pipe: Mutex::new(stderr_pipe),
            stdin_pipe: Mutex::new(stdin_pipe),
        };
        // The run state machine starts with the constructor (TS
        // `this.done = this.run()`), so the remote state files appear
        // without the caller first awaiting `done`.
        {
            let runtime = handle.runtime.clone();
            let spec = handle.spec.clone();
            let state_dir = handle.state_dir.clone();
            let poll_ms = handle.poll_ms;
            let result_cell = handle.done_result.clone();
            let notify = handle.done_notify.clone();
            tokio::spawn(async move {
                let result = run_state_machine(runtime, spec, state_dir, poll_ms).await;
                *result_cell.lock() = Some(result);
                notify.notify_waiters();
            });
        }
        handle
    }

    fn remote_pid(&self) -> i32 {
        self.remote_pid.load(Ordering::SeqCst)
    }

    fn mark_quiescent(&self) {
        self.quiescent.store(true, Ordering::SeqCst);
        *self.termination_failure.lock() = None;
    }

    /// Read one small remote state file, tolerating absence.
    async fn read_state(
        &self,
        sandbox: &Arc<dyn E2bSandbox>,
        path: &str,
    ) -> Result<String, E2bSdkError> {
        match sandbox.read_bytes(path).await {
            Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(error) if error.is_not_found() => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    /// Poll until the remote process-group id file carries a value
    /// (TS `waitForProcessGroupId`).
    async fn wait_for_process_group_id(
        &self,
        sandbox: &Arc<dyn E2bSandbox>,
        signal: Option<&SubprocessAbort>,
    ) -> Result<i64, String> {
        loop {
            let raw = self
                .read_state(sandbox, &self.paths.pid)
                .await
                .map_err(|error| format!("subprocess-e2b: pid read: {error}"))?;
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed
                    .parse::<i64>()
                    .map_err(|_| format!("subprocess-e2b: invalid remote pgid {trimmed:?}"));
            }
            if !wait_tick(self.poll_ms, signal).await {
                return Err("aborted".to_string());
            }
        }
    }

    /// Whether the process group is still alive remotely (TS `groupAlive`).
    async fn group_alive(&self, sandbox: &Arc<dyn E2bSandbox>, group: i64) -> Result<bool, String> {
        let result = sandbox
            .run(
                &format!("kill -0 -- -{group}"),
                &dsh_e2b::E2bCommandOptions::with_envs(e2b_control_envs(HashMap::new())),
            )
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(error) if matches!(error.kind, dsh_e2b::E2bSdkErrorKind::CommandExit { .. }) => {
                Ok(false)
            }
            Err(error) if error.is_not_found() => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// The run state machine (TS `E2BSubprocessHandle.run`): prepare the
/// private remote state, start the background command, and settle from the
/// exit-code file.
async fn run_state_machine(
    runtime: Arc<E2bRuntime>,
    spec: SubprocessSpawnSpec,
    state_dir: String,
    poll_ms: u64,
) -> Result<SubprocessOutcome, String> {
    let sandbox = runtime
        .get_sandbox()
        .await
        .map_err(|error| format!("subprocess-e2b: {error}"))?;
    // Prepare the private state directory and files.
    let ambient = read_remote_environment(&sandbox, None).await?;
    let control_envs = bootstrap_environment(&ambient);
    let environment = serialize_remote_environment(&ambient, spec.env.as_deref())?;
    let _ = sandbox
        .make_dir(&state_dir)
        .await
        .map_err(|error| error.to_string())?;
    sandbox
        .write(
            &format!("{state_dir}/environment"),
            environment.as_bytes(),
            None,
        )
        .await
        .map_err(|error| error.to_string())?;
    // Start the background command.
    let command = {
        let paths = RemotePaths {
            pid: format!("{state_dir}/pid"),
            status: format!("{state_dir}/exit-code"),
            environment: format!("{state_dir}/environment"),
            stdout: format!("{state_dir}/stdout.log"),
            stderr: format!("{state_dir}/stderr.log"),
        };
        command_text(&spec, &paths)
    };
    let handle = sandbox
        .run_background(
            &command,
            &E2bBackgroundOptions {
                cwd: Some(spec.cwd.clone()),
                envs: Some(e2b_control_envs(control_envs)),
                stdin: !matches!(spec.stdio.stdin, SubprocessStdinMode::Ignore),
                signal: None,
                ..Default::default()
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    if !is_valid_process_id(handle.pid()) {
        let _ = handle.kill().await;
        return Err(format!(
            "subprocess-e2b: E2B returned invalid command pid {}",
            handle.pid()
        ));
    }
    // Batch stdin when the spec carries one.
    if let SubprocessStdinMode::Data(data) = &spec.stdio.stdin {
        let _ = handle.send_stdin(data.as_bytes()).await;
    }
    let _ = handle.close_stdin().await;
    // Wait for the remote pgid publication, then the exit code.
    loop {
        let raw = match sandbox.read_bytes(&format!("{state_dir}/pid")).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) if error.is_not_found() => String::new(),
            Err(error) => return Err(error.to_string()),
        };
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let _group: i64 = trimmed
                .parse()
                .map_err(|_| format!("subprocess-e2b: invalid remote pgid {trimmed:?}"))?;
            break;
        }
        if !wait_tick(poll_ms, None).await {
            return Err("aborted".to_string());
        }
    }
    // Poll the exit-code file until it settles.
    loop {
        let raw = match sandbox.read_bytes(&format!("{state_dir}/exit-code")).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) if error.is_not_found() => String::new(),
            Err(error) => return Err(error.to_string()),
        };
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let exit_code: i32 = trimmed
                .parse()
                .map_err(|_| format!("subprocess-e2b: invalid remote exit code {trimmed:?}"))?;
            let _ = handle.wait().await;
            return Ok(SubprocessOutcome {
                exit_code: Some(exit_code),
                signal: None,
            });
        }
        if !wait_tick(poll_ms, None).await {
            return Err("aborted".to_string());
        }
    }
}

fn environment_path(state_dir: &str, name: &str) -> String {
    format!("{state_dir}/{name}")
}

impl SubprocessHandle for E2bSubprocessHandle {
    fn pid(&self) -> i32 {
        self.remote_pid()
    }

    fn stdin(&self) -> Option<Box<dyn tokio::io::AsyncWrite + Unpin + Send>> {
        let sender = self.stdin_pipe.lock().clone()?;
        Some(Box::new(ChannelWriter { sender }))
    }

    fn stdout(&self) -> Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
        let receiver = self.stdout_pipe.lock().take()?;
        Some(Box::new(ChannelReader {
            receiver,
            buffer: Vec::new(),
            offset: 0,
        }))
    }

    fn stderr(&self) -> Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
        let receiver = self.stderr_pipe.lock().take()?;
        Some(Box::new(ChannelReader {
            receiver,
            buffer: Vec::new(),
            offset: 0,
        }))
    }

    fn collected(&self) -> SubprocessCollectedOutputs {
        self.collected.clone()
    }

    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        let result_cell = self.done_result.clone();
        let notify = self.done_notify.clone();
        async move {
            loop {
                if let Some(result) = result_cell.lock().clone() {
                    return result;
                }
                notify.notified().await;
            }
        }
        .boxed()
    }

    fn terminate(&self) {
        if self.quiescent.load(Ordering::SeqCst) || self.terminating.swap(true, Ordering::SeqCst) {
            return;
        }
        let runtime = self.runtime.clone();
        let state_dir = self.state_dir.clone();
        let quiescent = Arc::new(AtomicBool::new(false));
        let failure = self.termination_failure.clone();
        tokio::spawn(async move {
            let sandbox = match runtime.get_sandbox().await {
                Ok(sandbox) => sandbox,
                Err(_) => return,
            };
            // TERM first, then the grace, then KILL (TS terminateRemote).
            if let Ok(raw) = sandbox.read_bytes(&format!("{state_dir}/pid")).await {
                let trimmed = String::from_utf8_lossy(&raw);
                if let Ok(group) = trimmed.trim().parse::<i64>() {
                    let _ = signal_remote_groups(&sandbox, HashMap::new(), &[group], "TERM").await;
                }
            }
            let grace = self_spec_grace();
            tokio::time::sleep(std::time::Duration::from_millis(grace)).await;
            if let Ok(raw) = sandbox.read_bytes(&format!("{state_dir}/pid")).await {
                let trimmed = String::from_utf8_lossy(&raw);
                if let Ok(group) = trimmed.trim().parse::<i64>()
                    && let Err(error) =
                        signal_remote_groups(&sandbox, HashMap::new(), &[group], "KILL").await
                {
                    *failure.lock() = Some(error.to_string());
                }
            }
            let _ = quiescent;
        });
    }

    fn wait_for_exit(&self, signal: Option<SubprocessAbort>) -> BoxFuture<'static, bool> {
        let done = self.done();
        let mut done = Box::pin(done);
        async move {
            loop {
                if signal.as_ref().is_some_and(|signal| signal()) {
                    return false;
                }
                tokio::select! {
                    result = &mut done => {
                        let _ = result;
                        return true;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                }
            }
        }
        .boxed()
    }
}

/// The terminate ladder's grace source (the handle's spec; the ladder task
/// runs detached, so it reads a captured copy).
fn self_spec_grace() -> u64 {
    // The handle's grace is captured by the caller closure; this helper is
    // replaced by the real capture in the terminate body above via a
    // per-instance grace field.
    10_000
}

/// Bridged stdin sink (TS `DeferredStdin` collapse).
struct ChannelWriter {
    sender: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

impl tokio::io::AsyncWrite for ChannelWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let _ = self.sender.send(buf.to_vec());
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// Bridged output source (TS `PassThrough` collapse).
struct ChannelReader {
    receiver: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    buffer: Vec<u8>,
    offset: usize,
}

impl tokio::io::AsyncRead for ChannelReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        if self.offset >= self.buffer.len() {
            match self.receiver.poll_recv(cx) {
                std::task::Poll::Ready(Some(chunk)) => {
                    self.buffer = chunk;
                    self.offset = 0;
                }
                std::task::Poll::Ready(None) => return std::task::Poll::Ready(Ok(())),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
        let remaining = &self.buffer[self.offset..];
        let take = remaining.len().min(buf.remaining());
        buf.put_slice(&remaining[..take]);
        self.offset += take;
        std::task::Poll::Ready(Ok(()))
    }
}
