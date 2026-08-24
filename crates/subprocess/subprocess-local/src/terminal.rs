#![allow(clippy::type_complexity)] // Cleanup state mirrors the shared async terminal lifecycle.

//! Local PTY terminal-process handle for the subprocess seam. Rust port of
//! `packages/subprocess/subprocess-local/src/terminal.ts`.
//!
//! # Deviations
//!
//! - The node-pty `IPty` boundary collapses into the [`PtyTerminal`] trait;
//!   no concrete backend exists yet (deferred to the PTY milestone), the
//!   handle logic is exercised through fakes.
//! - `terminate` returns an idempotent shared future instead of one cached
//!   promise object; the TS identity assertion collapses to
//!   same-result semantics.
//! - Signal names resolve through a fixed common table (the TS
//!   `os.constants.signals` reverse lookup).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

use dsh_subprocess::{
    SubprocessOutcome, SubprocessTerminalForeground, SubprocessTerminalHandle,
    SubprocessTerminalSignal,
};

use futures::future::BoxFuture;
use futures::stream::{BoxStream, StreamExt};
use parking_lot::Mutex;

use crate::process_inspector::{ProcessIdentity, ProcessInspector, TerminalKillSignal};

/// One allocated terminal process (the TS `IPty` collapse).
pub trait PtyTerminal: Send + Sync {
    fn pid(&self) -> u32;
    fn write(&self, data: &str) -> Result<(), String>;
    fn kill(&self, signal: &str) -> Result<(), String>;
    /// Register a data listener; the returned disposer removes it.
    fn on_data(&self, listener: Arc<dyn Fn(String) + Send + Sync>) -> Box<dyn Fn() + Send + Sync>;
    /// Register an exit listener `(exit_code, signal_number)`; the returned
    /// disposer removes it.
    fn on_exit(
        &self,
        listener: Arc<dyn Fn(Option<i32>, Option<u32>) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync>;
}

/// Reverse lookup over the common signal table (TS `signalName`).
fn signal_name(number: Option<u32>) -> Option<String> {
    let name = match number? {
        0 => return None,
        1 => "SIGHUP",
        2 => "SIGINT",
        9 => "SIGKILL",
        15 => "SIGTERM",
        20 => "SIGTSTP",
        _ => return None,
    };
    Some(name.to_string())
}

/// A local terminal whose process-session ownership stays below the PTY
/// backend (TS `LocalTerminalHandle`).
pub struct LocalTerminalHandle {
    terminal: Arc<dyn PtyTerminal>,
    inspector: Arc<dyn ProcessInspector>,
    grace_ms: u64,
    pid: u32,
    root_identity: Option<ProcessIdentity>,
    exited: AtomicBool,
    exit_notify: tokio::sync::Notify,
    outcome: Mutex<Option<SubprocessOutcome>>,
    output_sender: Mutex<Option<futures::channel::mpsc::UnboundedSender<Vec<u8>>>>,
    output_receiver: Mutex<Option<futures::channel::mpsc::UnboundedReceiver<Vec<u8>>>>,
    data_disposable: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    exit_disposable: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    tracked_descendants: Mutex<Vec<ProcessIdentity>>,
    cleanup: Mutex<Option<Arc<tokio::sync::Mutex<Option<Result<(), String>>>>>>,
    self_arc: std::sync::OnceLock<std::sync::Weak<Self>>,
}

impl LocalTerminalHandle {
    /// Allocate the handle around one published PTY (TS constructor).
    pub fn new(
        terminal: Arc<dyn PtyTerminal>,
        inspector: Arc<dyn ProcessInspector>,
        grace_ms: u64,
    ) -> Arc<Self> {
        let pid = terminal.pid();
        let root_identity = inspector
            .process_tree(pid)
            .into_iter()
            .find(|member| member.pid == pid);
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        let data_terminal = terminal.clone();
        let exit_terminal = terminal.clone();
        let handle = Arc::new(Self {
            terminal,
            inspector,
            grace_ms,
            pid,
            root_identity,
            exited: AtomicBool::new(false),
            exit_notify: tokio::sync::Notify::new(),
            outcome: Mutex::new(None),
            output_sender: Mutex::new(Some(sender)),
            output_receiver: Mutex::new(Some(receiver)),
            data_disposable: Mutex::new(None),
            exit_disposable: Mutex::new(None),
            tracked_descendants: Mutex::new(Vec::new()),
            cleanup: Mutex::new(None),
            self_arc: std::sync::OnceLock::new(),
        });
        handle
            .self_arc
            .set(Arc::downgrade(&handle))
            .expect("self-arc once");
        let data_handle = handle.clone();
        let data_disposable = data_terminal.on_data(Arc::new(move |data| {
            if let Some(sender) = data_handle.output_sender.lock().as_ref() {
                let _ = sender.unbounded_send(data.into_bytes());
            }
        }));
        *handle.data_disposable.lock() = Some(data_disposable);
        let exit_handle = handle.clone();
        let exit_disposable = exit_terminal.on_exit(Arc::new(move |exit_code, exit_signal| {
            exit_handle.settle_exit(exit_code, exit_signal);
        }));
        *handle.exit_disposable.lock() = Some(exit_disposable);
        handle
    }

    /// The single-exit callback body (TS `onExit`).
    fn settle_exit(&self, exit_code: Option<i32>, exit_signal: Option<u32>) {
        if self.exited.swap(true, SeqCst) {
            return;
        }
        // End the output stream so readers observe EOF after queued bytes.
        *self.output_sender.lock() = None;
        let outcome = SubprocessOutcome {
            exit_code: if exit_signal.is_none() || exit_signal == Some(0) {
                exit_code
            } else {
                None
            },
            signal: signal_name(exit_signal),
        };
        *self.outcome.lock() = Some(outcome);
        self.exit_notify.notify_waiters();
    }

    /// Wait until the terminal settles or the grace elapses.
    async fn wait_done_or(&self, grace_ms: u64) {
        tokio::select! {
            _ = self.exit_notify.notified() => {}
            _ = tokio::time::sleep(std::time::Duration::from_millis(grace_ms)) => {}
        }
    }

    /// Force-terminate the observable session synchronously during host
    /// exit (TS `terminateForHostExit`).
    pub fn terminate_for_host_exit(&self) {
        self.force_stop_descendants();
        self.force_stop_shell();
        self.force_stop_descendants();
    }

    fn force_stop_shell(&self) {
        if self.exited.load(SeqCst) {
            return;
        }
        if self.root_identity.is_some() {
            if let Some(root) = &self.root_identity {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.inspector
                        .signal_process(root, TerminalKillSignal::SigKill);
                }));
            }
            return;
        }
        let _ = self.terminal.kill("SIGKILL");
    }

    fn survivors(&self, members: &[ProcessIdentity]) -> Vec<ProcessIdentity> {
        members
            .iter()
            .filter(|member| self.inspector.is_alive(member))
            .cloned()
            .collect()
    }

    fn descendants(&self) -> Vec<ProcessIdentity> {
        // Adopt newly scanned members only while the numeric root pid
        // provably still carries the spawned shell's start identity.
        let tree = self.inspector.process_tree(self.pid);
        let root = tree.iter().find(|member| member.pid == self.pid);
        let root_verified = match (&self.root_identity, root) {
            (Some(identity), Some(root)) => root.started == identity.started,
            _ => false,
        };
        let mut groups: Vec<Vec<ProcessIdentity>> = vec![self.tracked_descendants.lock().clone()];
        if root_verified {
            groups.push(tree);
            groups.push(self.inspector.process_session(self.pid));
        }
        let merged = union_members(&groups);
        let filtered: Vec<ProcessIdentity> = merged
            .into_iter()
            .filter(|member| member.pid != self.pid)
            .collect();
        let survivors = self.survivors(&filtered);
        *self.tracked_descendants.lock() = survivors.clone();
        survivors
    }

    async fn wait_for_members(&self, members: &[ProcessIdentity]) -> Vec<ProcessIdentity> {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(self.grace_ms);
        let mut survivors = self.survivors(members);
        while !survivors.is_empty() && std::time::Instant::now() < until {
            let remaining = until
                .saturating_duration_since(std::time::Instant::now())
                .as_millis()
                .clamp(1, 25) as u64;
            tokio::time::sleep(std::time::Duration::from_millis(remaining)).await;
            survivors = self.survivors(members);
        }
        survivors
    }

    fn signal_members(&self, members: &[ProcessIdentity], signal: TerminalKillSignal) {
        for member in members {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.inspector.signal_process(member, signal);
            }));
        }
    }

    fn force_stop_descendants(&self) {
        let mut members = self.tracked_descendants.lock().clone();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.descendants())) {
            Ok(fresh) => members = fresh,
            Err(_) => {
                // Preserve already-captured identities when a final
                // process-table scan fails.
            }
        }
        self.signal_members(&members, TerminalKillSignal::SigKill);
    }

    async fn stop_descendants(&self) -> Vec<ProcessIdentity> {
        let captured = self.descendants();
        self.signal_members(&captured, TerminalKillSignal::SigTerm);
        let captured_survivors = self.wait_for_members(&captured).await;
        let members = union_members(&[captured_survivors, self.descendants()]);
        self.signal_members(&members, TerminalKillSignal::SigKill);
        let survivors = self.wait_for_members(&members).await;
        self.survivors(&union_members(&[survivors, self.descendants()]))
    }

    async fn stop_shell(&self) -> Result<(), String> {
        if !self.exited.load(SeqCst) {
            let _ = self.terminal.kill("SIGTERM");
            self.wait_done_or(self.grace_ms).await;
        }
        if !self.exited.load(SeqCst) {
            let _ = self.terminal.kill("SIGKILL");
            self.wait_done_or(self.grace_ms).await;
        }
        if !self.exited.load(SeqCst) {
            return Err(format!(
                "terminal cleanup failed; surviving pid: {}",
                self.pid
            ));
        }
        Ok(())
    }

    async fn close_once(&self) -> Result<(), String> {
        let mut survivors = self.stop_descendants().await;
        if !survivors.is_empty() {
            return Err(format!(
                "terminal cleanup failed; surviving pids: {}",
                survivors
                    .iter()
                    .map(|member| member.pid.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        self.stop_shell().await?;
        survivors = self.stop_descendants().await;
        if !survivors.is_empty() {
            return Err(format!(
                "terminal cleanup failed; surviving pids: {}",
                survivors
                    .iter()
                    .map(|member| member.pid.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(disposable) = self.data_disposable.lock().take() {
            disposable();
        }
        if let Some(disposable) = self.exit_disposable.lock().take() {
            disposable();
        }
        Ok(())
    }
}

/// One deduplicated member union over `pid:started` identity (TS
/// `unionMembers`).
fn union_members(groups: &[Vec<ProcessIdentity>]) -> Vec<ProcessIdentity> {
    let mut seen: std::collections::HashSet<(u32, String)> = std::collections::HashSet::new();
    let mut members: Vec<ProcessIdentity> = Vec::new();
    for group in groups {
        for member in group {
            let key = (member.pid, member.started.clone());
            if seen.insert(key) {
                members.push(member.clone());
            }
        }
    }
    members
}

impl SubprocessTerminalHandle for LocalTerminalHandle {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn output(&self) -> BoxStream<'static, Vec<u8>> {
        match self.output_receiver.lock().take() {
            Some(receiver) => receiver.boxed(),
            None => futures::stream::empty().boxed(),
        }
    }

    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        let handle = self_arc_of(self);
        Box::pin(async move {
            loop {
                if let Some(outcome) = handle.outcome.lock().clone() {
                    return Ok(outcome);
                }
                handle.exit_notify.notified().await;
            }
        })
    }

    fn write(&self, data: &str) -> BoxFuture<'static, Result<(), String>> {
        let handle = self_arc_of(self);
        let data = data.to_string();
        Box::pin(async move {
            if handle.exited.load(SeqCst) {
                return Err("terminal process has exited".to_string());
            }
            handle.terminal.write(&data)
        })
    }

    fn inspect_foreground(
        &self,
    ) -> BoxFuture<'static, Result<Option<SubprocessTerminalForeground>, String>> {
        let handle = self_arc_of(self);
        Box::pin(async move {
            handle.descendants();
            let Some(process_group_id) = handle.inspector.foreground_pgid(handle.pid) else {
                return Ok(None);
            };
            Ok(Some(SubprocessTerminalForeground {
                process_group_id,
                input_waiting: handle.inspector.is_stdin_waiting(process_group_id),
            }))
        })
    }

    fn signal_foreground(
        &self,
        signal: SubprocessTerminalSignal,
    ) -> BoxFuture<'static, Result<u32, String>> {
        let handle = self_arc_of(self);
        Box::pin(async move {
            let foreground = handle.inspect_foreground().await?.ok_or_else(|| {
                format!(
                    "cannot resolve foreground process group for terminal {}",
                    handle.pid
                )
            })?;
            if signal == SubprocessTerminalSignal::SigKill
                && foreground.process_group_id == handle.pid
            {
                return Err(
                    "refusing to SIGKILL the terminal shell; terminate the terminal session instead"
                        .to_string(),
                );
            }
            handle
                .inspector
                .signal_group(foreground.process_group_id, signal);
            Ok(foreground.process_group_id)
        })
    }

    fn terminate(&self) -> BoxFuture<'static, Result<(), String>> {
        let handle = self_arc_of(self);
        let slot = {
            let mut cleanup = handle.cleanup.lock();
            cleanup
                .get_or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
                .clone()
        };
        Box::pin(async move {
            let mut guard = slot.clone().lock_owned().await;
            if let Some(result) = &*guard {
                return result.clone();
            }
            let result = handle.close_once().await;
            *guard = Some(result.clone());
            if result.is_err() {
                *handle.cleanup.lock() = None;
            }
            result
        })
    }
}

/// Recover the owning `Arc` for one `&self` receiver (the handle is always
/// constructed through [`LocalTerminalHandle::new`]).
fn self_arc_of(handle: &LocalTerminalHandle) -> Arc<LocalTerminalHandle> {
    handle
        .self_arc
        .get()
        .and_then(std::sync::Weak::upgrade)
        .expect("LocalTerminalHandle must be held by an Arc")
}
