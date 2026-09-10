use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::{Duration, Instant};

use cordis::Context;
use dsh_sandbox::{ConfinedSandboxMode, SandboxMode, SandboxPolicy, SandboxProvider};
use dsh_sandbox_policy::{SandboxPolicyRequest, SandboxPolicyService};
use dsh_subprocess::{
    SubprocessRuntime, SubprocessTerminalHandle, SubprocessTerminalSignal,
    SubprocessTerminalSpawnSpec,
};
use dsh_terminal::{
    TerminalBackend, TerminalBackendSession, TerminalBackendSpawnError, TerminalBackendSpawnSpec,
    TerminalErrorCode, TerminalReadRequest, TerminalReadResult, TerminalSendOperation,
    TerminalSendRead, TerminalSendRequest, TerminalSendResult, TerminalSessionService,
    TerminalSessionStatus, TerminalSignal, TerminalSignalResult, TerminalWaitReason,
};
use futures::StreamExt;
use futures::future::{BoxFuture, FutureExt, Shared};
use parking_lot::Mutex;

#[derive(Debug, Clone)]
pub struct Config {
    pub backend_type: String,
    pub shell_path: String,
    pub shell_args: Vec<String>,
    pub rows: u16,
    pub cols: u16,
    pub scrollback_lines: usize,
    pub scrollback_max_bytes: usize,
    pub max_read_bytes: usize,
    pub idle_silence_ms: u64,
    pub timeout_ms: u64,
    pub dispose_grace_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        #[cfg(windows)]
        let (shell_path, shell_args) = (
            windows_command_shell(),
            vec!["/D".to_string(), "/Q".to_string()],
        );
        #[cfg(not(windows))]
        let (shell_path, shell_args) = (
            "/bin/bash".to_string(),
            vec![
                "--noprofile".to_string(),
                "--norc".to_string(),
                "-i".to_string(),
            ],
        );
        Self {
            backend_type: "shell".to_string(),
            shell_path,
            shell_args,
            rows: 40,
            cols: 160,
            scrollback_lines: 10_000,
            scrollback_max_bytes: 4 * 1024 * 1024,
            max_read_bytes: 256 * 1024,
            idle_silence_ms: 250,
            timeout_ms: 30_000,
            dispose_grace_ms: 3_000,
        }
    }
}

#[cfg(windows)]
fn windows_command_shell() -> String {
    let mut candidates = std::env::var_os("ComSpec")
        .map(std::path::PathBuf::from)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        candidates.push(std::path::PathBuf::from(system_root).join(r"System32\cmd.exe"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cmd.exe".to_string())
}

pub struct ShellTerminalBackend {
    subprocess: Arc<dyn SubprocessRuntime>,
    sandbox_policy: Arc<SandboxPolicyService>,
    sandbox: Option<Arc<dyn SandboxProvider>>,
    config: Config,
}

impl ShellTerminalBackend {
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        validate(&config)?;
        let terminals = ctx
            .get_typed::<Arc<TerminalSessionService>>("terminals", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "terminal-bash requires terminals".to_string())?;
        let subprocess = ctx
            .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "terminal-bash requires subprocess".to_string())?;
        let sandbox_policy = ctx
            .get_typed::<Arc<SandboxPolicyService>>("sandboxPolicy", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "terminal-bash requires sandboxPolicy".to_string())?;
        let sandbox = ctx
            .get_typed::<Arc<dyn SandboxProvider>>("sandbox", false)
            .map(|slot| slot.as_ref().clone());
        let backend = Arc::new(Self {
            subprocess,
            sandbox_policy,
            sandbox,
            config,
        });
        terminals
            .register_backend(backend.clone())
            .map_err(|error| error.to_string())?;
        Ok(backend)
    }
}

fn validate(config: &Config) -> Result<(), String> {
    if config.backend_type.is_empty() || config.shell_path.is_empty() {
        return Err("terminal-bash backend type and shell path must be non-empty".to_string());
    }
    if config.rows == 0
        || config.cols == 0
        || config.scrollback_lines == 0
        || config.max_read_bytes == 0
        || config.max_read_bytes > config.scrollback_max_bytes
        || config.timeout_ms == 0
        || config.dispose_grace_ms == 0
    {
        return Err("terminal-bash numeric bounds are invalid".to_string());
    }
    Ok(())
}

fn terminal_argv(
    backend: &ShellTerminalBackend,
    policy: &dsh_sandbox::SandboxExecutionPolicy,
) -> Result<(Vec<String>, Option<dsh_sandbox::SandboxStartup>), String> {
    let mut argv = vec![backend.config.shell_path.clone()];
    argv.extend(backend.config.shell_args.clone());
    let mode = match policy.mode {
        SandboxMode::DangerFullAccess => return Ok((argv, None)),
        SandboxMode::ReadOnly => ConfinedSandboxMode::ReadOnly,
        SandboxMode::WorkspaceWrite => ConfinedSandboxMode::WorkspaceWrite,
    };
    backend
        .sandbox
        .as_ref()
        .ok_or_else(|| {
            format!(
                "terminal-bash sandbox mode {:?} requires sandbox",
                policy.mode
            )
        })?
        .confine_with_startup(
            &argv,
            &SandboxPolicy {
                read_only_roots: policy.read_only_roots.clone(),
                mode,
                workspace_root: policy.workspace_root.clone(),
                session_id: policy.session_id.clone(),
            },
        )
        .map(|confined| (confined.argv, confined.startup))
        .map_err(|error| error.to_string())
}

fn terminal_cwd(value: String) -> String {
    if cfg!(windows) {
        if let Some(network) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{network}");
        }
        if let Some(local) = value.strip_prefix(r"\\?\") {
            return local.to_string();
        }
    }
    value
}

impl TerminalBackend for ShellTerminalBackend {
    fn type_(&self) -> String {
        self.config.backend_type.clone()
    }

    fn spawn(
        &self,
        spec: TerminalBackendSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>>
    {
        let subprocess = self.subprocess.clone();
        let policy = self.sandbox_policy.resolve(&SandboxPolicyRequest {
            session: Some(Arc::new(spec.owner.session().clone())),
            mode: None,
        });
        let argv = terminal_argv(self, &policy);
        let sandbox = self.sandbox.clone();
        let config = self.config.clone();
        Box::pin(async move {
            if spec.signal.as_ref().is_some_and(|signal| signal()) {
                return Err(TerminalBackendSpawnError::spawn("terminal spawn aborted"));
            }
            let (argv, startup) = argv.map_err(TerminalBackendSpawnError::spawn)?;
            if let Some(sandbox) = sandbox {
                sandbox.prepare(&policy).await.map_err(|message| {
                    let code = if message.starts_with("[SANDBOX_SETUP_TIMEOUT]") {
                        TerminalErrorCode::SandboxSetupTimeout
                    } else {
                        TerminalErrorCode::SandboxSetupFailed
                    };
                    TerminalBackendSpawnError::coded(message, code)
                })?;
            }
            if spec.signal.as_ref().is_some_and(|signal| signal()) {
                return Err(TerminalBackendSpawnError::spawn("terminal spawn aborted"));
            }
            let cwd = terminal_cwd(spec.cwd.unwrap_or(policy.workspace_root));
            let cancellation = spec.signal.clone();
            let terminal = subprocess
                .spawn_terminal(SubprocessTerminalSpawnSpec {
                    argv,
                    cwd,
                    env: Some(vec![
                        ("TERM".to_string(), "xterm-256color".to_string()),
                        ("PAGER".to_string(), "cat".to_string()),
                        ("GIT_PAGER".to_string(), "cat".to_string()),
                        ("PS1".to_string(), "dsh> ".to_string()),
                        ("DSH_SHELL".to_string(), "1".to_string()),
                        (
                            "DSH_SESSION_ID".to_string(),
                            spec.owner.id().as_str().to_string(),
                        ),
                        (
                            "DSH_PTY_SESSION_ID".to_string(),
                            spec.session_id.as_str().to_string(),
                        ),
                    ]),
                    rows: config.rows,
                    cols: config.cols,
                    grace_ms: config.dispose_grace_ms,
                    signal: spec.signal,
                })
                .await
                .map_err(TerminalBackendSpawnError::spawn)?;
            let session = Arc::new(LocalPtySession::new(terminal, config));
            if let Err(mut error) = session.initialize(startup, cancellation).await {
                return match session.close("PTY startup failed").await {
                    Ok(()) => Err(error),
                    Err(cleanup) => {
                        error.cleanup_error = Some(cleanup);
                        Err(error)
                    }
                };
            }
            Ok(session as Arc<dyn TerminalBackendSession>)
        })
    }
}

#[derive(Default)]
struct BoundedText {
    value: String,
    dropped: bool,
    read_offset: usize,
}

impl BoundedText {
    fn append(&mut self, text: &str, max_bytes: usize, max_lines: Option<usize>) {
        if text.is_empty() {
            return;
        }
        self.value.push_str(text);
        if let Some(max_lines) = max_lines {
            while self.value.lines().count() > max_lines {
                let remove = self
                    .value
                    .find('\n')
                    .map(|index| index + 1)
                    .unwrap_or(self.value.len());
                self.value.drain(..remove);
                self.read_offset = self.read_offset.saturating_sub(remove);
                self.dropped = true;
            }
        }
        while self.value.len() > max_bytes {
            let remove = self
                .value
                .char_indices()
                .nth(1)
                .map(|(index, _)| index)
                .unwrap_or(self.value.len());
            self.value.drain(..remove);
            self.read_offset = self.read_offset.saturating_sub(remove);
            self.dropped = true;
        }
    }

    fn consume(&mut self) -> TerminalSendRead {
        self.read_offset = self.read_offset.min(self.value.len());
        let delta = self.value[self.read_offset..].to_string();
        self.read_offset = self.value.len();
        let truncated = self.dropped;
        self.dropped = false;
        TerminalSendRead { delta, truncated }
    }

    fn snapshot(&self) -> (String, bool) {
        (self.value.clone(), self.dropped)
    }
}

fn drain_utf8(buffer: &mut Vec<u8>, final_chunk: bool) -> String {
    let mut output = String::new();
    loop {
        match std::str::from_utf8(buffer) {
            Ok(text) => {
                output.push_str(text);
                buffer.clear();
                break;
            }
            Err(failure) => {
                let valid = failure.valid_up_to();
                if valid > 0 {
                    output.push_str(
                        std::str::from_utf8(&buffer[..valid])
                            .expect("valid_up_to always delimits valid UTF-8"),
                    );
                    buffer.drain(..valid);
                }
                if let Some(length) = failure.error_len() {
                    output.push('\u{fffd}');
                    buffer.drain(..length.min(buffer.len()));
                    continue;
                }
                if final_chunk {
                    output.push_str(&String::from_utf8_lossy(buffer));
                    buffer.clear();
                }
                break;
            }
        }
    }
    output
}

struct SendState {
    output: Mutex<BoundedText>,
    settled: AtomicBool,
    cancelled: AtomicBool,
}

struct LocalSendOperation {
    state: Arc<SendState>,
    done: Shared<BoxFuture<'static, TerminalSendResult>>,
}

impl TerminalSendOperation for LocalSendOperation {
    fn done(&self) -> BoxFuture<'static, TerminalSendResult> {
        self.done.clone().boxed()
    }

    fn read_output(&self) -> TerminalSendRead {
        self.state.output.lock().consume()
    }

    fn cancel(&self) -> bool {
        !self.state.settled.load(SeqCst) && !self.state.cancelled.swap(true, SeqCst)
    }
}

struct LocalPtySession {
    terminal: Arc<dyn SubprocessTerminalHandle>,
    status: Arc<Mutex<TerminalSessionStatus>>,
    output: Arc<Mutex<BoundedText>>,
    active: Arc<Mutex<Option<Arc<SendState>>>>,
    output_notify: Arc<tokio::sync::Notify>,
    last_output: Arc<Mutex<Instant>>,
    config: Config,
    closing: Arc<AtomicBool>,
}

impl LocalPtySession {
    fn new(terminal: Arc<dyn SubprocessTerminalHandle>, config: Config) -> Self {
        let status = Arc::new(Mutex::new(TerminalSessionStatus::Running));
        let output = Arc::new(Mutex::new(BoundedText::default()));
        let active: Arc<Mutex<Option<Arc<SendState>>>> = Arc::new(Mutex::new(None));
        let output_notify = Arc::new(tokio::sync::Notify::new());
        let last_output = Arc::new(Mutex::new(Instant::now()));
        let closing = Arc::new(AtomicBool::new(false));
        let mut stream = terminal.output();
        let output_task = output.clone();
        let active_task = active.clone();
        let notify_task = output_notify.clone();
        let last_output_task = last_output.clone();
        let max_bytes = config.scrollback_max_bytes;
        let max_lines = config.scrollback_lines;
        let max_read = config.max_read_bytes;
        tokio::spawn(async move {
            let mut undecoded = Vec::new();
            while let Some(chunk) = stream.next().await {
                // Retain CR and ANSI/VT sequences. Interactive Web terminals
                // need the original byte order to update cursor position,
                // colours and progress lines correctly.
                undecoded.extend_from_slice(&chunk);
                let text = drain_utf8(&mut undecoded, false);
                output_task.lock().append(&text, max_bytes, Some(max_lines));
                if let Some(active) = active_task.lock().clone() {
                    active.output.lock().append(&text, max_read, None);
                }
                *last_output_task.lock() = Instant::now();
                notify_task.notify_waiters();
            }
            let tail = drain_utf8(&mut undecoded, true);
            if !tail.is_empty() {
                output_task.lock().append(&tail, max_bytes, Some(max_lines));
                if let Some(active) = active_task.lock().clone() {
                    active.output.lock().append(&tail, max_read, None);
                }
            }
            notify_task.notify_waiters();
        });
        let status_task = status.clone();
        let notify_exit = output_notify.clone();
        let done = terminal.done();
        tokio::spawn(async move {
            let status_value = match done.await {
                Ok(outcome) => TerminalSessionStatus::Exited {
                    exit_code: outcome.exit_code,
                    signal: outcome.signal,
                },
                Err(_) => TerminalSessionStatus::Exited {
                    exit_code: None,
                    signal: None,
                },
            };
            *status_task.lock() = status_value;
            notify_exit.notify_waiters();
        });
        Self {
            terminal,
            status,
            output,
            active,
            output_notify,
            last_output,
            config,
            closing,
        }
    }

    async fn initialize(
        &self,
        startup: Option<dsh_sandbox::SandboxStartup>,
        cancellation: Option<dsh_subprocess::SubprocessAbort>,
    ) -> Result<(), TerminalBackendSpawnError> {
        if let Some(startup) = startup {
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                if cancellation.as_ref().is_some_and(|signal| signal()) {
                    return Err(TerminalBackendSpawnError::coded(
                        "terminal startup cancelled",
                        TerminalErrorCode::Aborted,
                    ));
                }
                if startup.is_ready().map_err(|message| {
                    TerminalBackendSpawnError::coded(message, TerminalErrorCode::SandboxSetupFailed)
                })? {
                    break;
                }
                if matches!(self.status(), TerminalSessionStatus::Exited { .. }) {
                    return Err(TerminalBackendSpawnError::coded(
                        format!(
                            "Sandbox runner exited before command readiness: {}",
                            startup_tail(&self.output.lock().snapshot().0)
                        ),
                        TerminalErrorCode::SandboxSetupFailed,
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(TerminalBackendSpawnError::coded(
                        "Sandbox startup exceeded 120 seconds before command readiness was confirmed",
                        TerminalErrorCode::SandboxSetupTimeout,
                    ));
                }
                tokio::select! {
                    _ = self.output_notify.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
        }
        let deadline = Instant::now() + Duration::from_millis(self.config.timeout_ms.min(20_000));
        let silence = Duration::from_millis(self.config.idle_silence_ms);
        loop {
            if cancellation.as_ref().is_some_and(|signal| signal()) {
                return Err(TerminalBackendSpawnError::coded(
                    "terminal startup cancelled",
                    TerminalErrorCode::Aborted,
                ));
            }
            if matches!(self.status(), TerminalSessionStatus::Exited { .. }) {
                #[cfg(target_os = "linux")]
                {
                    let output = self.output.lock().snapshot().0;
                    let tail = output
                        .chars()
                        .rev()
                        .take(2048)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<String>();
                    return Err(TerminalBackendSpawnError::coded(
                        format!(
                            "PTY shell exited during startup ({:?}): {}",
                            self.status(),
                            tail.trim()
                        ),
                        TerminalErrorCode::StartupFailed,
                    ));
                }
                #[cfg(not(target_os = "linux"))]
                return Err(TerminalBackendSpawnError::coded(
                    format!(
                        "PTY shell exited during startup: {}",
                        startup_tail(&self.output.lock().snapshot().0)
                    ),
                    TerminalErrorCode::StartupFailed,
                ));
            }
            let output = self.output.lock().snapshot().0;
            let quiet = self.last_output.lock().elapsed() >= silence;
            #[cfg(windows)]
            if visible_terminal_text(&output).contains('>') && quiet {
                return Ok(());
            }
            #[cfg(not(windows))]
            if output.contains("dsh> ") && quiet {
                return Ok(());
            }
            if Instant::now() >= deadline {
                let output = startup_tail(&output);
                return Err(TerminalBackendSpawnError::coded(
                    format!("PTY shell did not reach startup prompt: {output:?}"),
                    TerminalErrorCode::StartupTimeout,
                ));
            }
            tokio::select! {
                _ = self.output_notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
    }
}

fn startup_tail(value: &str) -> String {
    value
        .chars()
        .rev()
        .take(2048)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[cfg(windows)]
fn visible_terminal_text(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::new();
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'[') {
            index += 2;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if (0x40..=0x7e).contains(&byte) {
                    break;
                }
            }
            continue;
        }
        if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b']') {
            index += 2;
            while index < bytes.len() {
                if bytes[index] == 0x07 {
                    index += 1;
                    break;
                }
                if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        let tail = &value[index..];
        let Some(ch) = tail.chars().next() else {
            break;
        };
        if !ch.is_control() {
            output.push(ch);
        }
        index += ch.len_utf8();
    }
    output
}

impl TerminalBackendSession for LocalPtySession {
    fn motd(&self) -> String {
        self.output.lock().snapshot().0
    }
    fn pid(&self) -> Option<u32> {
        Some(self.terminal.pid())
    }
    fn start_send(&self, request: &TerminalSendRequest) -> Arc<dyn TerminalSendOperation> {
        let state = Arc::new(SendState {
            output: Mutex::new(BoundedText::default()),
            settled: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        });
        *self.active.lock() = Some(state.clone());
        *self.last_output.lock() = Instant::now();

        let terminal = self.terminal.clone();
        let status = self.status.clone();
        let active = self.active.clone();
        let output_notify = self.output_notify.clone();
        let last_output = self.last_output.clone();
        let closing = self.closing.clone();
        let state_for_done = state.clone();
        let signal = request.signal.clone();
        let mut input = request.text.clone();
        if request.submit {
            #[cfg(windows)]
            input.push_str("\r\n");
            #[cfg(not(windows))]
            input.push('\r');
        }
        let idle_silence = Duration::from_millis(self.config.idle_silence_ms);
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let done = async move {
            let started = Instant::now();
            if !input.is_empty() {
                terminal
                    .write(&input)
                    .await
                    .unwrap_or_else(|error| panic!("terminal write failed: {error}"));
            }
            let wait_reason = loop {
                let current_status = status.lock().clone();
                if matches!(current_status, TerminalSessionStatus::Exited { .. })
                    || closing.load(SeqCst)
                {
                    break TerminalWaitReason::SessionExit;
                }
                let caller_cancelled = signal.as_ref().is_some_and(|abort| abort());
                if caller_cancelled || state_for_done.cancelled.load(SeqCst) {
                    state_for_done.cancelled.store(true, SeqCst);
                    let _ = terminal
                        .signal_foreground(SubprocessTerminalSignal::SigInt)
                        .await;
                }
                let has_output = !state_for_done.output.lock().snapshot().0.is_empty();
                if has_output && last_output.lock().elapsed() >= idle_silence {
                    break TerminalWaitReason::InferredIdle;
                }
                if started.elapsed() >= timeout {
                    break TerminalWaitReason::Timeout;
                }
                tokio::select! {
                    _ = output_notify.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            };
            state_for_done.settled.store(true, SeqCst);
            {
                let mut slot = active.lock();
                if slot
                    .as_ref()
                    .is_some_and(|candidate| Arc::ptr_eq(candidate, &state_for_done))
                {
                    *slot = None;
                }
            }
            let (viewport, truncated) = state_for_done.output.lock().snapshot();
            let session_status = status.lock().clone();
            TerminalSendResult {
                viewport,
                wait_reason,
                session_status,
                truncated,
            }
        }
        .boxed()
        .shared();
        Arc::new(LocalSendOperation { state, done })
    }
    fn write_input(&self, data: &str) -> BoxFuture<'static, Result<(), String>> {
        self.terminal.write(data)
    }
    fn resize(&self, rows: u16, cols: u16) -> BoxFuture<'static, Result<(), String>> {
        self.terminal.resize(rows, cols)
    }
    fn read(&self, request: &TerminalReadRequest) -> TerminalReadResult {
        let (text, inherited_truncation) = self.output.lock().snapshot();
        let lines: Vec<&str> = if text.is_empty() {
            Vec::new()
        } else {
            text.lines().collect()
        };
        let total = lines.len() as u64;
        let offset = request.offset.unwrap_or(0).min(total);
        let count = request.count.unwrap_or(500).max(1);
        let end = total.saturating_sub(offset) as usize;
        let start = end.saturating_sub(count as usize);
        let mut page = lines[start..end].join("\n");
        let mut truncated = false;
        while page.len() > self.config.max_read_bytes {
            let next = page
                .char_indices()
                .nth(1)
                .map(|(index, _)| index)
                .unwrap_or(page.len());
            page.drain(..next);
            truncated = true;
        }
        TerminalReadResult {
            text: page,
            total_lines: total,
            line_begin: offset,
            line_end: offset + (end - start) as u64,
            truncated: inherited_truncation || truncated,
        }
    }
    fn signal(
        &self,
        signal: TerminalSignal,
    ) -> BoxFuture<'static, Result<TerminalSignalResult, String>> {
        let terminal = self.terminal.clone();
        Box::pin(async move {
            let mapped = match signal {
                TerminalSignal::SigInt => SubprocessTerminalSignal::SigInt,
                TerminalSignal::SigTerm => SubprocessTerminalSignal::SigTerm,
                TerminalSignal::SigKill => SubprocessTerminalSignal::SigKill,
                TerminalSignal::SigTstp => SubprocessTerminalSignal::SigTstp,
                TerminalSignal::SigHup => SubprocessTerminalSignal::SigHup,
            };
            terminal
                .signal_foreground(mapped)
                .await
                .map(|target_pgid| TerminalSignalResult {
                    delivered: true,
                    target_pgid,
                })
        })
    }
    fn status(&self) -> TerminalSessionStatus {
        self.status.lock().clone()
    }
    fn close(&self, _reason: &str) -> BoxFuture<'static, Result<(), String>> {
        self.closing.store(true, SeqCst);
        self.terminal.terminate()
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::{terminal_cwd, visible_terminal_text};

    #[test]
    fn startup_probe_ignores_device_queries_and_waits_for_prompt() {
        assert_eq!(
            visible_terminal_text("\u{1b}[6n\u{1b}]0;PowerShell\u{7}\u{1b}[?25h"),
            ""
        );
        assert!(visible_terminal_text("\u{1b}[32mPS C:\\repo>\u{1b}[0m ").contains('>'));
    }

    #[test]
    fn cmd_cwd_drops_the_windows_verbatim_prefix() {
        assert_eq!(
            terminal_cwd(r"\\?\D:\workspace".to_string()),
            r"D:\workspace"
        );
        assert_eq!(
            terminal_cwd(r"\\?\UNC\server\share".to_string()),
            r"\\server\share"
        );
    }
}

#[cfg(test)]
mod utf8_tests {
    use super::drain_utf8;

    #[test]
    fn terminal_output_keeps_codepoints_split_across_pty_chunks() {
        let encoded = "你".as_bytes();
        let mut pending = encoded[..2].to_vec();
        assert_eq!(drain_utf8(&mut pending, false), "");
        pending.extend_from_slice(&encoded[2..]);
        assert_eq!(drain_utf8(&mut pending, false), "你");
        assert!(pending.is_empty());

        let mut invalid = b"a\xffb".to_vec();
        assert_eq!(drain_utf8(&mut invalid, true), "a\u{fffd}b");
    }
}

#[cfg(test)]
mod tests {
    use super::BoundedText;

    #[test]
    fn bounded_scrollback_preserves_utf8_boundaries() {
        let mut text = BoundedText::default();
        text.append("甲乙丙丁", 7, None);
        let (value, truncated) = text.snapshot();
        assert!(truncated);
        assert!(value.is_char_boundary(value.len()));
        assert_eq!(value, "丙丁");
    }

    #[test]
    fn independent_send_cursor_only_returns_new_output() {
        let mut text = BoundedText::default();
        text.append("first\n", 1024, None);
        assert_eq!(text.consume().delta, "first\n");
        assert!(text.consume().delta.is_empty());
        text.append("second\n", 1024, None);
        assert_eq!(text.consume().delta, "second\n");
    }

    #[test]
    fn line_cap_keeps_the_newest_complete_lines() {
        let mut text = BoundedText::default();
        text.append("one\ntwo\nthree\n", 1024, Some(2));
        let (value, truncated) = text.snapshot();
        assert!(truncated);
        assert_eq!(value, "two\nthree\n");
    }
}
