//! Owner-scoped interactive UU terminals. A forced-new, handshaken live CLI
//! connection owns input; remote session IDs are never guessed.
#[path = "uu_terminal_protocol.rs"]
mod protocol;

use crate::uu_cli::{self, TerminalShell};
use crate::uu_devices::{Bridge, TerminalTarget};
use cordis::Context;
use dsh_jobs::{JobHooks, JobOutcome, JobOutcomeStatus, JobRegistry, JobStart};
use dsh_llm::ContentBlock;
use dsh_subprocess::{SubprocessRuntime, SubprocessTerminalHandle, SubprocessTerminalSpawnSpec};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRunContext, ToolRuntime};
use dsh_user_approval::{ApprovalOutcome, ApprovalRequest, ApprovalService};
use futures::{StreamExt, future::BoxFuture};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

type Abort = Arc<dyn Fn() -> bool + Send + Sync>;
type Approval = Arc<
    dyn Fn(&str, &TerminalTarget, Option<&str>) -> BoxFuture<'static, Result<(), String>>
        + Send
        + Sync,
>;
type Track = Arc<dyn Fn(Arc<Slot>) -> Result<String, String> + Send + Sync>;
const MAX_OUTPUT: usize = 256 * 1024;
const MAX_READ: usize = 32 * 1024;
const MAX_INPUT: usize = 8 * 1024;

fn numeric_argument(
    args: &Value,
    key: &str,
    default: Option<u64>,
    minimum: u64,
    maximum: u64,
) -> Result<Option<u64>, String> {
    let Some(value) = args.get(key) else {
        return Ok(default);
    };
    let number = value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|number| {
                number.is_finite()
                    && number.fract() == 0.0
                    && *number >= 0.0
                    && *number <= 9_007_199_254_740_991.0
            })
            .map(|number| number as u64)
    });
    number
        .filter(|number| *number >= minimum && *number <= maximum)
        .map(Some)
        .ok_or_else(|| format!("{key} 超出整数范围 {minimum}..={maximum}"))
}

trait Access: Send + Sync {
    fn target(
        &self,
        device: Option<String>,
        signal: Abort,
    ) -> BoxFuture<'static, Result<TerminalTarget, String>>;
    fn validate(
        &self,
        target: TerminalTarget,
        binding: bool,
        signal: Abort,
    ) -> BoxFuture<'static, Result<(), String>>;
}

struct BridgeAccess(Arc<Bridge>);
impl Access for BridgeAccess {
    fn target(
        &self,
        device: Option<String>,
        signal: Abort,
    ) -> BoxFuture<'static, Result<TerminalTarget, String>> {
        let bridge = self.0.clone();
        Box::pin(async move {
            bridge
                .bound_terminal_target(device.as_deref(), Some(signal))
                .await
        })
    }
    fn validate(
        &self,
        target: TerminalTarget,
        binding: bool,
        signal: Abort,
    ) -> BoxFuture<'static, Result<(), String>> {
        let bridge = self.0.clone();
        Box::pin(async move {
            bridge
                .validate_terminal_target(&target, binding, Some(signal))
                .await
        })
    }
}

trait Transport: Send + Sync {
    fn preferred_target(&self, target: &TerminalTarget) -> TerminalTarget {
        target.clone()
    }
    fn compatibility_target(&self, _target: &TerminalTarget) -> Option<TerminalTarget> {
        None
    }
    fn remember_compatible(&self, _primary: &TerminalTarget, _selected: &TerminalTarget) {}
    fn sessions(
        &self,
        target: TerminalTarget,
        signal: Abort,
    ) -> BoxFuture<'static, Result<BTreeSet<String>, String>>;
    fn spawn(
        &self,
        target: TerminalTarget,
        shell: TerminalShell,
        cwd: String,
        signal: Abort,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>>;
}

type CliKey = (std::path::PathBuf, String, String);
type CliStamp = (u64, std::time::SystemTime);
struct CompatibleCli {
    primary: CliStamp,
    selected: CliStamp,
    path: std::path::PathBuf,
}
struct NativeTransport {
    runtime: Arc<dyn SubprocessRuntime>,
    compatible: Mutex<HashMap<CliKey, CompatibleCli>>,
}
fn cli_stamp(path: &std::path::Path) -> Option<CliStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some((metadata.len(), metadata.modified().ok()?))
}
fn cli_key(target: &TerminalTarget) -> CliKey {
    (
        target.cli.clone(),
        target.account.clone(),
        target.device_id.clone(),
    )
}
fn installed_backup_cli(primary: &std::path::Path) -> Option<std::path::PathBuf> {
    if !primary
        .file_name()?
        .to_str()?
        .eq_ignore_ascii_case("uuyc-cli.exe")
    {
        return None;
    }
    let parent = primary.parent()?;
    let root = if parent.file_name()?.to_str()?.eq_ignore_ascii_case("bin") {
        parent.parent()?
    } else {
        parent
    };
    let root = root.canonicalize().ok()?;
    let backup = root.join("bak/uuyc-cli.exe").canonicalize().ok()?;
    if !backup.starts_with(&root)
        || backup == primary.canonicalize().ok()?
        || cli_stamp(&backup).is_none()
    {
        return None;
    }
    // Do not retry an identical copy, or select by a version-string allowlist.
    use sha2::{Digest, Sha256};
    (Sha256::digest(std::fs::read(primary).ok()?) != Sha256::digest(std::fs::read(&backup).ok()?))
        .then_some(backup)
}
async fn terminal_query(
    target: &TerminalTarget,
    arguments: Vec<String>,
    signal: Abort,
) -> Result<String, String> {
    dsh_native_command::run_native_command_bounded(
        &target.cli.to_string_lossy(),
        &arguments,
        Some(signal),
        dsh_native_command::NativeCommandLimits {
            // UU 4.x may take several seconds to wake its remote-terminal
            // broker after the desktop client reconnects. Keep the query
            // bounded, but do not turn a normal broker wake-up into a
            // misleading terminal timeout.
            timeout: Duration::from_secs(30),
            stdout_bytes: 256 * 1024,
            stderr_bytes: 8192,
        },
    )
    .await
    .map(|output| output.stdout)
    .map_err(|error| {
        let detail = protocol::startup_failure(&error.stderr)
            .or_else(|| protocol::startup_failure(&error.stdout))
            .map(str::to_owned)
            .unwrap_or_else(|| uu_cli::failure_message(error.code.as_deref()));
        format!("UU 终端会话列表查询失败：{}", detail)
    })
}
impl Transport for NativeTransport {
    fn preferred_target(&self, target: &TerminalTarget) -> TerminalTarget {
        let choices = self.compatible.lock();
        let mut selected = target.clone();
        if let Some(choice) = choices.get(&cli_key(target)) {
            if cli_stamp(&target.cli) == Some(choice.primary)
                && cli_stamp(&choice.path) == Some(choice.selected)
            {
                selected.cli = choice.path.clone();
            }
        }
        selected
    }
    fn compatibility_target(&self, target: &TerminalTarget) -> Option<TerminalTarget> {
        let mut selected = target.clone();
        selected.cli = installed_backup_cli(&target.cli)?;
        Some(selected)
    }
    fn remember_compatible(&self, primary: &TerminalTarget, selected: &TerminalTarget) {
        if primary.cli == selected.cli {
            self.compatible.lock().remove(&cli_key(primary));
            return;
        }
        if let (Some(primary_stamp), Some(selected_stamp)) =
            (cli_stamp(&primary.cli), cli_stamp(&selected.cli))
        {
            self.compatible.lock().insert(
                cli_key(primary),
                CompatibleCli {
                    primary: primary_stamp,
                    selected: selected_stamp,
                    path: selected.cli.clone(),
                },
            );
        }
    }
    fn sessions(
        &self,
        target: TerminalTarget,
        signal: Abort,
    ) -> BoxFuture<'static, Result<BTreeSet<String>, String>> {
        Box::pin(async move {
            let output = terminal_query(
                &target,
                protocol::list_arguments(&target.device_id)?,
                signal,
            )
            .await?;
            protocol::parse_sessions(&output)
        })
    }
    fn spawn(
        &self,
        target: TerminalTarget,
        shell: TerminalShell,
        cwd: String,
        signal: Abort,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let mut argv = vec![target.cli.to_string_lossy().into_owned()];
            argv.extend(uu_cli::new_terminal_arguments(&target.device_id, shell)?);
            runtime
                .spawn_terminal(SubprocessTerminalSpawnSpec {
                    argv,
                    cwd,
                    env: Some(vec![("TERM".into(), "xterm-256color".into())]),
                    rows: 40,
                    cols: 120,
                    grace_ms: 3000,
                    signal: Some(signal),
                })
                .await
        })
    }
}

async fn bounded<T>(
    future: impl std::future::Future<Output = Result<T, String>>,
    signal: Abort,
    timeout: Duration,
) -> Result<T, String> {
    tokio::pin!(future);
    let cancellation = async move {
        loop {
            if signal() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    tokio::pin!(cancellation);
    tokio::select! {
        biased;
        result = &mut future => result,
        _ = &mut cancellation => Err("UU 远程终端操作已取消".into()),
        _ = tokio::time::sleep(timeout) => Err("UU 远程终端操作超时".into()),
    }
}

#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
    start: u64,
    end: u64,
}
impl Output {
    fn append(&mut self, bytes: &[u8]) {
        self.end = self.end.saturating_add(bytes.len() as u64);
        if bytes.len() >= MAX_OUTPUT {
            self.bytes.clear();
            self.bytes.extend(&bytes[bytes.len() - MAX_OUTPUT..]);
            self.start = self.end.saturating_sub(MAX_OUTPUT as u64);
            return;
        }
        self.bytes.extend(bytes);
        let remove = self.bytes.len().saturating_sub(MAX_OUTPUT);
        self.bytes.drain(..remove);
        self.start = self.start.saturating_add(remove as u64);
    }
    fn page(&self, cursor: Option<u64>, limit: usize) -> Result<(String, u64, bool), String> {
        let wanted = cursor.unwrap_or(self.start);
        if wanted > self.end {
            return Err("UU 终端输出游标超出范围".into());
        }
        let from = wanted.max(self.start);
        let offset = (from - self.start) as usize;
        let limit = if limit == 0 {
            0
        } else {
            limit.max(4).min(MAX_READ)
        };
        let mut bytes: Vec<_> = self
            .bytes
            .iter()
            .skip(offset)
            .take(limit)
            .copied()
            .collect();
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_none() {
                bytes.truncate(error.valid_up_to());
            }
        }
        Ok((
            String::from_utf8_lossy(&bytes).into_owned(),
            from + bytes.len() as u64,
            wanted < self.start,
        ))
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.iter().copied().collect::<Vec<_>>()).into_owned()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Starting,
    Ready,
    Closing,
    Closed,
    CleanupUnconfirmed,
}
impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Closing => "closing",
            Self::Closed => "closed",
            Self::CleanupUnconfirmed => "cleanup-unconfirmed",
        }
    }
    fn terminal(self) -> bool {
        matches!(self, Self::Closed | Self::CleanupUnconfirmed)
    }
}
struct SlotState {
    phase: Phase,
    client: Option<Arc<dyn SubprocessTerminalHandle>>,
    spawned_client: bool,
    before: Option<BTreeSet<String>>,
    inventory_error: Option<String>,
    remote_id: Option<String>,
    job_id: Option<String>,
    initialization_failure: Option<String>,
    failure: Option<String>,
    output: Output,
    initial_prompt: Option<String>,
    has_user_input: bool,
    read_prompt_end: Option<u64>,
    unsubmitted_input: bool,
    exit_requested: bool,
    exit_observed: bool,
    diagnostic_tail: String,
    process_exit_code: Option<i32>,
    process_signal: Option<String>,
    output_drain_finished: bool,
    owned_connection: bool,
}
impl SlotState {
    fn reserves_connection(&self) -> bool {
        self.phase != Phase::Closed
            && (self.phase != Phase::CleanupUnconfirmed
                || self.owned_connection
                || self.client.is_some())
    }
}
struct Slot {
    id: String,
    owner: String,
    target: TerminalTarget,
    access: Arc<dyn Access>,
    transport: Arc<dyn Transport>,
    state: Mutex<SlotState>,
    operation: tokio::sync::Mutex<()>,
    changed: tokio::sync::Notify,
    finished: tokio::sync::Notify,
    client_exited: AtomicBool,
    output_done: AtomicBool,
    cancelled: AtomicBool,
    ownership_lost: AtomicBool,
    shell: TerminalShell,
    job_cursor: Mutex<u64>,
}

impl Slot {
    fn snapshot(&self, cursor: Option<u64>, limit: usize) -> Result<Value, String> {
        let state = self.state.lock();
        let (output, cursor, truncated) = state.output.page(cursor, limit)?;
        Ok(
            json!({"terminalId":self.id,"deviceId":self.target.device_id,"deviceName":self.target.device_name,
            "cliPath":self.target.cli,
            "remoteSessionId":state.remote_id,"jobId":state.job_id,"state":state.phase.name(),
            "connectionExited":self.client_exited.load(Ordering::Acquire),"output":output,"cursor":cursor,"truncated":truncated,
            "cleanupConfirmed":state.phase==Phase::Closed,"error":state.initialization_failure.as_ref().or(state.failure.as_ref()),
            "cleanupError":state.failure,
            "processExitCode":state.process_exit_code,"processSignal":state.process_signal,
            "inventoryBaselineAvailable":state.before.is_some(),"inventoryError":state.inventory_error,
            "ownership":if state.owned_connection {"new-cli-connection"} else {"unconfirmed"},
            "ownershipLost":self.ownership_lost.load(Ordering::Acquire),
            "writable":state.phase==Phase::Ready && !self.client_exited.load(Ordering::Acquire) && !self.ownership_lost.load(Ordering::Acquire),
            "canCloseGracefully":self.can_exit(&state),"exitRequested":state.exit_requested,"remoteExitObserved":state.exit_observed}),
        )
    }
    fn can_exit(&self, state: &SlotState) -> bool {
        state.owned_connection
            && !state.unsubmitted_input
            && !self.client_exited.load(Ordering::Acquire)
            && !self.ownership_lost.load(Ordering::Acquire)
            && state.read_prompt_end == Some(state.output.end)
            && state.initial_prompt.is_some()
            && protocol::shell_prompt(&state.output.text(), self.shell) == state.initial_prompt
    }
    fn observe_read(&self, cursor: Option<u64>, limit: usize) -> Result<(), String> {
        let mut state = self.state.lock();
        let (visible, end, _) = state.output.page(cursor, limit)?;
        state.read_prompt_end = if end == state.output.end
            && !state.unsubmitted_input
            && state.initial_prompt.is_some()
            && protocol::shell_prompt(&visible, self.shell) == state.initial_prompt
        {
            Some(state.output.end)
        } else {
            None
        };
        Ok(())
    }
    fn connection_error(&self, fallback: String) -> String {
        if self.ownership_lost.load(Ordering::Acquire) {
            "UU CLI 终端已被另一窗口接入（code 2001）；已停止输入，未重新附着或关闭未知远端会话"
                .into()
        } else {
            fallback
        }
    }
    async fn wait_output_end(&self) -> Result<(), String> {
        bounded(
            async {
                loop {
                    let notified = self.changed.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    if self.output_done.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    notified.await;
                }
            },
            Arc::new(|| false),
            Duration::from_secs(3),
        )
        .await
    }
    fn own_signal(self: &Arc<Self>, caller: Abort) -> Abort {
        let slot = Arc::downgrade(self);
        Arc::new(move || {
            caller()
                || slot.upgrade().is_none_or(|slot| {
                    slot.cancelled.load(Ordering::Acquire)
                        || slot.ownership_lost.load(Ordering::Acquire)
                })
        })
    }
    async fn initialize(
        self: &Arc<Self>,
        shell: TerminalShell,
        cwd: String,
        signal: Abort,
    ) -> Result<(), String> {
        let _operation = self.operation.lock().await;
        let before = bounded(
            self.transport.sessions(self.target.clone(), signal.clone()),
            signal.clone(),
            Duration::from_secs(35),
        )
        .await;
        if signal() {
            return Err("UU 终端建立已取消".into());
        }
        {
            let mut state = self.state.lock();
            match before {
                Ok(before) => state.before = Some(before),
                Err(error) => state.inventory_error = Some(error),
            }
        }
        // A captured inventory query can time out while the interactive
        // connection would explain that the desktop is locked. The inventory
        // is only a cleanup baseline; input still requires the forced-new
        // connection's own handshake and binding checks.
        let client = bounded(
            self.transport
                .spawn(self.target.clone(), shell, cwd, signal.clone()),
            signal.clone(),
            Duration::from_secs(30),
        )
        .await?;
        {
            let mut state = self.state.lock();
            state.client = Some(client.clone());
            state.spawned_client = true;
        }
        let mut stream = client.output();
        let output_slot = self.clone();
        tokio::spawn(async move {
            while let Some(bytes) = stream.next().await {
                let mut state = output_slot.state.lock();
                let fresh = String::from_utf8_lossy(&bytes);
                let diagnostic = format!("{}{}", state.diagnostic_tail, fresh);
                if protocol::connection_taken_over(&diagnostic) {
                    output_slot.ownership_lost.store(true, Ordering::Release);
                }
                let start = diagnostic
                    .char_indices()
                    .rev()
                    .nth(511)
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                state.diagnostic_tail = diagnostic[start..].to_string();
                state.output.append(&bytes);
                if state.initial_prompt.is_none() && !state.has_user_input {
                    let text = state.output.text();
                    if protocol::ready(&text) {
                        state.initial_prompt = protocol::shell_prompt(&text, output_slot.shell);
                    }
                }
                drop(state);
                output_slot.changed.notify_waiters();
            }
            output_slot.output_done.store(true, Ordering::Release);
            output_slot.changed.notify_waiters();
        });
        let exit_slot = self.clone();
        tokio::spawn(async move {
            let outcome = client.done().await;
            if let Ok(outcome) = &outcome {
                let mut state = exit_slot.state.lock();
                state.process_exit_code = outcome.exit_code;
                state.process_signal = outcome.signal.clone();
            }
            exit_slot.client_exited.store(true, Ordering::Release);
            // ConPTY may keep its output pipe open after the child exits.
            // Closing that already-exited local PTY drains the final bytes.
            let _ = client.terminate().await;
            let output_drained = exit_slot.wait_output_end().await.is_ok();
            exit_slot.state.lock().output_drain_finished = true;
            if output_drained
                && outcome
                    .as_ref()
                    .is_ok_and(|value| value.exit_code == Some(0) && value.signal.is_none())
            {
                let mut state = exit_slot.state.lock();
                if state.exit_requested && !exit_slot.ownership_lost.load(Ordering::Acquire) {
                    state.exit_observed = true;
                }
            }
            exit_slot.changed.notify_waiters();
            // An ended local client is not proof that its remote shell ended.
            // The cleanup path only confirms absence; it never kills a stale ID.
            let _ = exit_slot.close_owned(false).await;
        });
        let ready = async {
            loop {
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let text = self.state.lock().output.text();
                if self.ownership_lost.load(Ordering::Acquire) {
                    return Err(self.connection_error("UU 终端连接已转移".into()));
                }
                if let Some(error) = protocol::startup_failure(&text) {
                    return Err(error.into());
                }
                if self.client_exited.load(Ordering::Acquire) {
                    // Process exit and the final PTY read arrive independently.
                    // Wait for the bounded drain before classifying startup.
                    if !self.state.lock().output_drain_finished {
                        notified.await;
                        continue;
                    }
                    let state = self.state.lock();
                    if self.ownership_lost.load(Ordering::Acquire) {
                        return Err(self.connection_error("UU 终端连接已转移".into()));
                    }
                    if let Some(error) = protocol::startup_failure(&state.output.text()) {
                        return Err(error.into());
                    }
                    let detail = state
                        .process_exit_code
                        .map(|code| {
                            if code == 0 {
                                "：进程正常结束，但未收到终端握手（进程退出码 0）".into()
                            } else {
                                format!(
                                    "：{}（进程退出码 {code}）",
                                    uu_cli::failure_message(Some(&code.to_string()))
                                )
                            }
                        })
                        .unwrap_or_default();
                    return Err(format!("UU 终端连接在建立前退出{detail}"));
                }
                if protocol::ready(&text) {
                    // The vendor handshake establishes the interactive
                    // connection. A customized Windows prompt is still a
                    // usable shell; recognizing a prompt is only needed to
                    // authorize automatic graceful cleanup.
                    if let Some(prompt) = protocol::shell_prompt(&text, shell) {
                        self.state.lock().initial_prompt = Some(prompt);
                    }
                    return Ok(());
                }
                notified.await;
            }
        };
        bounded(ready, signal.clone(), Duration::from_secs(30)).await?;
        // Listing through another CLI while this PTY is attached can displace it.
        // The explicitly forced-new, live, handshaken connection owns this I/O.
        self.access
            .validate(self.target.clone(), true, signal.clone())
            .await?;
        if signal() || self.client_exited.load(Ordering::Acquire) {
            return Err(self.connection_error("UU 终端建立已取消或连接退出".into()));
        }
        let mut state = self.state.lock();
        state.owned_connection = true;
        state.phase = Phase::Ready;
        Ok(())
    }

    async fn close_owned(self: &Arc<Self>, approved_close: bool) -> Result<(), String> {
        let _operation = self.operation.lock().await;
        {
            let state = self.state.lock();
            if state.phase == Phase::Closed {
                return Ok(());
            }
            // Process-exit and cancellation observers may arrive after an
            // earlier close has finished. Repeating their inventory query
            // could displace a subsequently opened terminal on this device.
            // Only an explicit close may retry unconfirmed cleanup.
            if state.phase == Phase::CleanupUnconfirmed && !approved_close {
                return Err(state
                    .failure
                    .clone()
                    .unwrap_or_else(|| "UU 远端清理未确认".into()));
            }
        }
        let (client, before, spawned_client, may_exit) = {
            let mut state = self.state.lock();
            let may_exit = approved_close && self.can_exit(&state);
            state.phase = Phase::Closing;
            (
                state.client.clone(),
                state.before.clone(),
                state.spawned_client,
                may_exit,
            )
        };
        let no_abort: Abort = Arc::new(|| false);
        let authorization = if spawned_client && approved_close {
            bounded(
                self.access
                    .validate(self.target.clone(), false, no_abort.clone()),
                no_abort.clone(),
                Duration::from_secs(15),
            )
            .await
        } else {
            Ok(())
        };
        let mut graceful_error = None;
        if authorization.is_ok() && may_exit && self.can_exit(&self.state.lock()) {
            if let Some(client) = client.as_ref() {
                let signal = self.own_signal(no_abort.clone());
                if !signal() {
                    self.state.lock().exit_requested = true;
                    let exit = async {
                        client.write("exit\r").await?;
                        let outcome = client.done().await?;
                        let _ = client.terminate().await;
                        self.wait_output_end().await?;
                        if outcome.exit_code != Some(0) || outcome.signal.is_some() {
                            return Err("UU CLI 未正常确认远端 Shell 结束".into());
                        }
                        if self.ownership_lost.load(Ordering::Acquire) {
                            return Err(self.connection_error("连接归属变化".into()));
                        }
                        self.state.lock().exit_observed = true;
                        self.client_exited.store(true, Ordering::Release);
                        Ok(())
                    };
                    graceful_error = bounded(exit, signal, Duration::from_secs(8)).await.err();
                }
            }
        }
        // Stop only the owned local transport before inventory verification;
        // never query or kill an opaque remote ID through an active CLI.
        let local_result = if let Some(client) = client.as_ref() {
            bounded(client.terminate(), no_abort.clone(), Duration::from_secs(8)).await
        } else {
            Ok(())
        };
        if local_result.is_ok() {
            self.client_exited.store(true, Ordering::Release);
        }
        let remote_result = async {
            authorization?;
            if !spawned_client {
                return Ok(());
            }
            // Cancellation and exit observers only own the local transport.
            // Starting another remote CLI query here can take over a newer
            // connection; an explicit, admitted close performs verification.
            if !approved_close {
                return Err(self.connection_error(
                    "仅关闭自有本地终端连接；未执行远端 exit 或库存核验，远端清理未确认".into(),
                ));
            }
            let baseline = before
                .as_ref()
                .ok_or("未获得启动前终端库存，无法确认远端库存清理；未操作任何未知会话")?;
            if local_result.is_err() && !self.client_exited.load(Ordering::Acquire) {
                return Err("本地终端未关闭，跳过可能中断连接的列表查询；远端清理未确认".into());
            }
            let current = bounded(
                self.transport
                    .sessions(self.target.clone(), no_abort.clone()),
                no_abort.clone(),
                Duration::from_secs(15),
            )
            .await?;
            if &current != baseline {
                return Err("远端终端库存尚未恢复到启动前基线；未关闭任何未知会话".into());
            }
            let state = self.state.lock();
            if state.owned_connection && (!state.exit_requested || !state.exit_observed) {
                return Err(self.connection_error(
                    "仅本地连接已结束，缺少已批准 exit 的远端结束确认；清理未确认".into(),
                ));
            }
            if self.ownership_lost.load(Ordering::Acquire) {
                return Err(self.connection_error("终端连接归属已丢失；远端清理未确认".into()));
            }
            if let Some(error) = graceful_error {
                return Err(error);
            }
            Ok(())
        }
        .await;
        let local_cleaned = local_result.is_ok();
        let result = remote_result.and(local_result);
        {
            let mut state = self.state.lock();
            if local_cleaned {
                state.client = None;
            }
            state.phase = if result.is_ok() {
                Phase::Closed
            } else {
                Phase::CleanupUnconfirmed
            };
            state.failure = result.as_ref().err().cloned();
        }
        self.finished.notify_waiters();
        result
    }
    fn cancel(self: &Arc<Self>) {
        self.cancelled.store(true, Ordering::Release);
        self.changed.notify_waiters();
        let slot = self.clone();
        tokio::spawn(async move {
            let _ = slot.close_owned(false).await;
        });
    }
}

struct RemoteJob(Arc<Slot>);
impl JobHooks for RemoteJob {
    fn cancel(&self, _reason: Option<String>) {
        self.0.cancel();
    }
    fn done(&self) -> BoxFuture<'static, JobOutcome> {
        let slot = self.0.clone();
        Box::pin(async move {
            loop {
                let notified = slot.finished.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let ended = {
                    let state = slot.state.lock();
                    state.phase.terminal().then(|| JobOutcome {
                        status: if state.phase == Phase::CleanupUnconfirmed {
                            JobOutcomeStatus::Failed
                        } else if slot.cancelled.load(Ordering::Acquire) {
                            JobOutcomeStatus::Killed
                        } else {
                            JobOutcomeStatus::Completed
                        },
                        detail: Some(
                            state
                                .failure
                                .clone()
                                .unwrap_or_else(|| "owned UU remote terminal closed".into()),
                        ),
                        output: None,
                    })
                };
                if let Some(outcome) = ended {
                    return outcome;
                }
                notified.await;
            }
        })
    }
    fn read_output(&self) -> Option<String> {
        let mut cursor = self.0.job_cursor.lock();
        let state = self.0.state.lock();
        let (text, next, truncated) = state.output.page(Some(*cursor), MAX_READ).ok()?;
        *cursor = next;
        Some(if truncated {
            format!("[earlier UU terminal output truncated]\n{text}")
        } else {
            text
        })
    }
}

struct CleanupOnDrop(Option<Arc<Slot>>);
impl Drop for CleanupOnDrop {
    fn drop(&mut self) {
        if let Some(slot) = self.0.take() {
            slot.cancel();
        }
    }
}
struct Caller {
    owner: String,
    signal: Abort,
    approve: Approval,
    track: Track,
    cwd: String,
}
pub(crate) struct RemoteTerminals {
    access: Arc<dyn Access>,
    transport: Arc<dyn Transport>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    admission: tokio::sync::Mutex<()>,
    closing: AtomicBool,
}
impl RemoteTerminals {
    fn find(&self, owner: &str, id: &str) -> Result<Arc<Slot>, String> {
        self.slots
            .lock()
            .get(id)
            .filter(|slot| slot.owner == owner)
            .cloned()
            .ok_or_else(|| "UU 终端不存在或不属于当前会话".into())
    }
    fn list(&self, owner: &str) -> Value {
        let slots: Vec<_> = self
            .slots
            .lock()
            .values()
            .filter(|slot| slot.owner == owner)
            .cloned()
            .collect();
        json!({"terminals":slots.iter().filter_map(|slot|slot.snapshot(None,0).ok()).collect::<Vec<_>>()})
    }
    async fn open(
        &self,
        caller: &Caller,
        device: Option<String>,
        shell: TerminalShell,
    ) -> Result<Value, String> {
        let _admission = self.admission.lock().await;
        if self.closing.load(Ordering::Acquire) || (caller.signal)() {
            return Err("UU 终端服务正在关闭或操作已取消".into());
        }
        {
            let mut slots = self.slots.lock();
            if slots.len() >= 32 {
                slots.retain(|_, slot| slot.state.lock().reserves_connection());
            }
            if slots.len() >= 32
                || slots
                    .values()
                    .filter(|slot| {
                        slot.owner == caller.owner && slot.state.lock().reserves_connection()
                    })
                    .count()
                    >= 3
            {
                return Err("UU 终端数量超过限制，请先关闭已有终端".into());
            }
        }
        let target = self.access.target(device, caller.signal.clone()).await?;
        if self.slots.lock().values().any(|slot| {
            slot.target.account == target.account
                && slot.target.device_id == target.device_id
                && slot.state.lock().client.is_some()
                && !slot.client_exited.load(Ordering::Acquire)
        }) {
            return Err(
                "该设备已有活跃 UU 终端连接，请先结束其控制；不通过新列表查询中断已有连接".into(),
            );
        }
        (caller.approve)("open", &target, None).await?;
        let primary = target;
        let target = self.transport.preferred_target(&primary);
        self.access
            .validate(target.clone(), true, caller.signal.clone())
            .await?;
        match self.open_target(caller, target.clone(), shell).await {
            Ok(value) => Ok(value),
            Err((error, compatible)) => {
                if !compatible || (caller.signal)() {
                    return Err(error);
                }
                let backup = if target.cli != primary.cli {
                    self.transport.remember_compatible(&primary, &primary);
                    Some(primary.clone())
                } else {
                    self.transport.compatibility_target(&target)
                };
                let Some(backup) = backup else {
                    return Err(error);
                };
                self.access
                    .validate(backup.clone(), true, caller.signal.clone())
                    .await?;
                match self.open_target(caller, backup.clone(), shell).await {
                    Ok(mut value) => {
                        self.transport.remember_compatible(&primary, &backup);
                        value["compatibilityFallback"] = json!({"primaryCliPath":primary.cli,"reason":"invalid open response","primaryFailure":error});
                        Ok(value)
                    }
                    Err((fallback, _)) => {
                        Err(format!("{error}; 安装目录兼容 CLI 连接结果：{fallback}"))
                    }
                }
            }
        }
    }
    async fn open_target(
        &self,
        caller: &Caller,
        target: TerminalTarget,
        shell: TerminalShell,
    ) -> Result<Value, (String, bool)> {
        {
            let mut slots = self.slots.lock();
            if slots.len() >= 32 {
                slots.retain(|_, slot| slot.state.lock().reserves_connection());
            }
        }
        let slot = Arc::new(Slot {
            id: format!("uu-terminal-{}", uuid::Uuid::new_v4()),
            owner: caller.owner.clone(),
            target,
            access: self.access.clone(),
            transport: self.transport.clone(),
            state: Mutex::new(SlotState {
                phase: Phase::Starting,
                client: None,
                spawned_client: false,
                before: None,
                inventory_error: None,
                remote_id: None,
                job_id: None,
                initialization_failure: None,
                process_exit_code: None,
                process_signal: None,
                output_drain_finished: false,
                failure: None,
                output: Output::default(),
                initial_prompt: None,
                has_user_input: false,
                read_prompt_end: None,
                unsubmitted_input: false,
                exit_requested: false,
                exit_observed: false,
                diagnostic_tail: String::new(),
                owned_connection: false,
            }),
            operation: Default::default(),
            changed: Default::default(),
            finished: Default::default(),
            client_exited: AtomicBool::new(false),
            output_done: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            ownership_lost: AtomicBool::new(false),
            shell,
            job_cursor: Mutex::new(0),
        });
        self.slots.lock().insert(slot.id.clone(), slot.clone());
        let mut cleanup = CleanupOnDrop(Some(slot.clone()));
        let signal = slot.own_signal(caller.signal.clone());
        let initialized = slot.initialize(shell, caller.cwd.clone(), signal).await;
        if let Err(error) = initialized {
            slot.state.lock().initialization_failure = Some(slot.connection_error(error.clone()));
            let close_error = slot.close_owned(false).await.err();
            cleanup.0 = None;
            let compatible = error.contains("invalid open response") && {
                let state = slot.state.lock();
                state.client.is_none() && !state.owned_connection && !state.has_user_input
            };
            return Err((
                format!(
                    "{}; terminalId={}{}",
                    slot.connection_error(error),
                    slot.id,
                    close_error
                        .map(|error| format!("; {error}"))
                        .unwrap_or_default()
                ),
                compatible,
            ));
        }
        // The lifecycle job represents an established remote terminal. A
        // failed inventory query or handshake must not publish a spurious
        // "completed terminal" notification for a shell that never opened.
        let job_id = match (caller.track)(slot.clone()) {
            Ok(job_id) => job_id,
            Err(error) => {
                slot.state.lock().initialization_failure = Some(error.clone());
                let close_error = slot.close_owned(false).await.err();
                cleanup.0 = None;
                return Err((
                    format!(
                        "{error}; terminalId={}{}",
                        slot.id,
                        close_error
                            .map(|error| format!("; {error}"))
                            .unwrap_or_default()
                    ),
                    false,
                ));
            }
        };
        slot.state.lock().job_id = Some(job_id);
        cleanup.0 = None;
        slot.snapshot(None, MAX_READ)
            .map_err(|error| (error, false))
    }
    async fn write(
        &self,
        caller: &Caller,
        id: &str,
        text: &str,
        submit: bool,
        wait_ms: u64,
    ) -> Result<Value, String> {
        if text.len() > MAX_INPUT || text.contains('\0') || wait_ms > 5000 {
            return Err("UU 终端输入或等待参数超出范围".into());
        }
        let slot = self.find(&caller.owner, id)?;
        if slot.state.lock().phase != Phase::Ready
            || slot.client_exited.load(Ordering::Acquire)
            || slot.ownership_lost.load(Ordering::Acquire)
        {
            return Err(slot.connection_error("UU 远端终端尚未就绪或连接已退出".into()));
        }
        self.access
            .validate(slot.target.clone(), true, caller.signal.clone())
            .await?;
        (caller.approve)("write", &slot.target, Some(text)).await?;
        self.access
            .validate(slot.target.clone(), true, caller.signal.clone())
            .await?;
        let _operation = slot.operation.lock().await;
        let client = {
            let state = slot.state.lock();
            if state.phase != Phase::Ready
                || slot.client_exited.load(Ordering::Acquire)
                || slot.ownership_lost.load(Ordering::Acquire)
            {
                return Err(slot.connection_error("UU 远端终端尚未就绪或连接已退出".into()));
            }
            state.client.clone().ok_or("UU 本地终端连接不可用")?
        };
        let mut cleanup = CleanupOnDrop(Some(slot.clone()));
        let signal = slot.own_signal(caller.signal.clone());
        if signal() {
            return Err("UU 终端输入已取消".into());
        }
        let mut input = text.to_string();
        if submit {
            input.push('\r');
        }
        {
            let mut state = slot.state.lock();
            state.read_prompt_end = None;
            state.has_user_input = true;
            state.unsubmitted_input = !submit;
        }
        bounded(client.write(&input), signal.clone(), Duration::from_secs(8))
            .await
            .map_err(|error| slot.connection_error(error))?;
        if wait_ms > 0 {
            bounded(
                async {
                    tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                    Ok(())
                },
                signal,
                Duration::from_millis(wait_ms + 100),
            )
            .await?;
        }
        cleanup.0 = None;
        let mut value = slot.snapshot(None, MAX_READ)?;
        value["submitted"] = json!(submit);
        value["waitReason"] = json!("output-window");
        Ok(value)
    }
    async fn close(&self, caller: &Caller, id: &str) -> Result<Value, String> {
        let _admission = self.admission.lock().await;
        let slot = self.find(&caller.owner, id)?;
        if slot.state.lock().phase == Phase::Closed {
            return slot.snapshot(None, 0);
        }
        if self.slots.lock().values().any(|other| {
            other.id != slot.id
                && other.target.account == slot.target.account
                && other.target.device_id == slot.target.device_id
                && other.state.lock().client.is_some()
                && !other.client_exited.load(Ordering::Acquire)
        }) {
            return Err("该设备已有其他活跃终端连接，暂不核验旧终端记录，以免中断新连接".into());
        }
        (caller.approve)("close", &slot.target, None).await?;
        if (caller.signal)() {
            return Err("UU 终端关闭已取消".into());
        }
        let mut cleanup = CleanupOnDrop(Some(slot.clone()));
        let result = slot.close_owned(true).await;
        cleanup.0 = None;
        result?;
        slot.snapshot(None, 0)
    }
    async fn dispose(&self) {
        self.closing.store(true, Ordering::Release);
        let slots: Vec<_> = self.slots.lock().values().cloned().collect();
        futures::future::join_all(slots.into_iter().map(|slot| async move {
            slot.cancelled.store(true, Ordering::Release);
            let _ = slot.close_owned(false).await;
        }))
        .await;
    }
}

pub(crate) fn install(ctx: &Context, bridge: Arc<Bridge>) -> Result<Arc<RemoteTerminals>, String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|value| value.as_ref().clone())
        .ok_or("uu-terminal requires tools")?;
    let jobs = ctx
        .get_typed::<Arc<dyn JobRegistry>>("jobs", false)
        .map(|value| value.as_ref().clone())
        .ok_or("uu-terminal requires jobs")?;
    let approval = ctx
        .get_typed::<Arc<ApprovalService>>("approval", false)
        .map(|value| value.as_ref().clone())
        .ok_or("uu-terminal requires approval")?;
    let subprocess = ctx
        .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
        .map(|value| value.as_ref().clone())
        .ok_or("uu-terminal requires subprocess")?;
    let service = Arc::new(RemoteTerminals {
        access: Arc::new(BridgeAccess(bridge)),
        transport: Arc::new(NativeTransport {
            runtime: subprocess,
            compatible: Default::default(),
        }),
        slots: Default::default(),
        admission: Default::default(),
        closing: AtomicBool::new(false),
    });
    let controller = jobs.attach_controller(ctx, "uu-terminal");
    let execution_service = service.clone();
    tools.register(ctx,ToolDefinition{
        name:"uu_terminal".into(),
        description:"Open/read/write/close/list owner-isolated interactive terminals on the currently bound UU REMOTE Windows device. Commands execute remotely, not in the local workspace. Open/write/close use approval; read/list only inspect owned local records. Input belongs to the live forced-new CLI connection; remoteSessionId may be null and must not be guessed. Read after commands. Close sends the documented exit only at a freshly read original shell prompt with no unsubmitted input, then verifies client exit and the remote inventory baseline; otherwise it detaches and reports unconfirmed cleanup. Code 2001 means ownership was lost: stop, never reattach automatically. Never supply OS unlock credentials.".into(),
        parameters:json!({"type":"object","additionalProperties":false,"properties":{"action":{"type":"string","enum":["open","read","write","close","list"]},"deviceId":{"type":"string"},"terminalId":{"type":"string"},"shell":{"type":"string","enum":["powershell","cmd","zsh","bash"]},"text":{"type":"string"},"submit":{"type":"boolean"},"cursor":{"type":"integer"},"limitBytes":{"type":"integer"},"waitMs":{"type":"integer"}},"required":["action"]}),
        output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![ContentBlock::Text{text:serde_json::to_string(value).map_err(|error|error.to_string())?}])),presentation_meta:None},
        timeout_ms:Some(120_000),is_concurrency_safe:Some(Arc::new(|args|matches!(args["action"].as_str(),Some("read"|"list")))),
        execute:Arc::new(move|args,run:&ToolRunContext|{
            let args=args.clone();let service=execution_service.clone();let agent=run.execution.agent.clone();let signal=run.execution.signal.lock().clone();let approval=approval.clone();let jobs=jobs.clone();let call_id=run.execution.call_id.to_string();
            Box::pin(async move{
                let agent=agent.ok_or_else(||ToolBodyError::plain("uu_terminal requires an initiating agent"))?;
                let owner=agent.id().to_string();let approval_agent=agent.clone();let approval_signal=signal.clone();
                let approve:Approval=Arc::new(move|action,target,text|{
                    let approval=approval.clone();let agent=approval_agent.clone();let signal=approval_signal.clone();let call_id=call_id.clone();let action=action.to_string();let target=target.clone();let text=text.map(str::to_string);
                    Box::pin(async move{
                        let reason=format!("UU 远端终端 {action}：{} ({}){}",target.device_name,target.device_id,text.map(|text|format!("\n输入：{text}")).unwrap_or_default());
                        match approval.request(&ApprovalRequest{agent,tool_name:"uu_terminal".into(),call_id:Some(call_id),reason:Some(reason),grant_key:None,rememberable:false,signal:Some(signal)}).await?{
                            ApprovalOutcome::AllowedOnce|ApprovalOutcome::AllowedAlways=>Ok(()),
                            _=>Err("UU 远端终端操作未获批准或已取消".into()),
                        }
                    })
                });
                let job_agent=agent.clone();let track:Track=Arc::new(move|slot|{
                    let hook=Arc::new(RemoteJob(slot.clone()));jobs.start(JobStart{kind:"uu-terminal".into(),label:format!("UU remote terminal: {} ({})",slot.target.device_name,slot.target.device_id),output_limit_bytes:Some(MAX_READ as u64),owner:Some(job_agent.clone()),run:Arc::new(move||hook.clone())}).map(|id|id.to_string())
                });
                let cwd=agent.session().header().cwd.clone().unwrap_or_else(||std::env::temp_dir().to_string_lossy().into_owned());
                let caller=Caller{owner,signal,approve,track,cwd};
                let action=args["action"].as_str().ok_or_else(||ToolBodyError::plain("action is required"))?;
                let cursor=numeric_argument(&args,"cursor",None,0,u64::MAX).map_err(ToolBodyError::plain)?;
                let limit=numeric_argument(&args,"limitBytes",Some(MAX_READ as u64),4,MAX_READ as u64).map_err(ToolBodyError::plain)?.unwrap() as usize;
                let wait=numeric_argument(&args,"waitMs",Some(250),0,5000).map_err(ToolBodyError::plain)?.unwrap();
                let result=match action{
                    "list"=>Ok(service.list(&caller.owner)),
                    "open"=>service.open(&caller,args["deviceId"].as_str().map(str::to_string),TerminalShell::parse(args["shell"].as_str().unwrap_or("powershell")).map_err(ToolBodyError::plain)?).await,
                    "read"|"write"|"close"=>{
                        let id=args["terminalId"].as_str().filter(|id|!id.is_empty()).ok_or_else(||ToolBodyError::plain("terminalId is required"))?;
                        match action{
                            "read"=>service.find(&caller.owner,id).and_then(|slot|{slot.observe_read(cursor,limit)?;slot.snapshot(cursor,limit)}),
                            "write"=>service.write(&caller,id,args["text"].as_str().ok_or_else(||ToolBodyError::plain("text is required"))?,args["submit"].as_bool().unwrap_or(true),wait).await,
                            _=>service.close(&caller,id).await,
                        }
                    }
                    _=>Err("unknown uu_terminal action".into()),
                };
                result.map_err(|error|ToolBodyError::coded(error,"UuTerminalError","UU_TERMINAL"))
            })
        }),finalize_content:None,present_call:None,present_result:None,
    })?;
    let cleanup = service.clone();
    let _ = ctx.effect(
        "UU remote terminal lifecycle",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                let cleanup = cleanup.clone();
                let controller = controller.clone();
                Box::pin(async move {
                    cleanup.dispose().await;
                    controller().await;
                })
            }))
        }),
    );
    Ok(service)
}

#[cfg(test)]
#[path = "uu_terminal_tests.rs"]
mod tests;
