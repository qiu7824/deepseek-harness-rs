//! Rust port of the core
//! `packages/subprocess/subprocess-local/tests/terminal.spec.ts` behaviors:
//! the local PTY handle's host-exit teardown, identity-fenced adoption and
//! signalling, output/foreground bridging, and the TERM→KILL escalation.
//!
//! # Deviations
//!
//! - `vi.useFakeTimers` collapses to real (short) grace waits.
//! - The TS cached-promise identity assertion collapses to same-result
//!   semantics: both `terminate` calls resolve identically.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};

use dsh_subprocess::{
    SubprocessOutcome, SubprocessRuntime, SubprocessTerminalHandle, SubprocessTerminalSignal,
    SubprocessTerminalSpawnSpec,
};
use dsh_subprocess_local::{
    LocalSubprocessRuntime, LocalTerminalHandle, ProcessIdentity, ProcessInspector, PtyTerminal,
    TerminalKillSignal,
};
use futures::StreamExt;
use parking_lot::Mutex;

/// A scripted PTY terminal (TS `FakePty`).
struct FakePty {
    pid: u32,
    writes: Mutex<Vec<String>>,
    kills: Mutex<Vec<String>>,
    auto_exit_on_kill: AtomicBool,
    throw_kill: AtomicBool,
    on_kill: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    data_listeners: Arc<Mutex<Vec<Arc<dyn Fn(String) + Send + Sync>>>>,
    exit_listeners: Arc<Mutex<Vec<Arc<dyn Fn(Option<i32>, Option<u32>) + Send + Sync>>>>,
}

impl FakePty {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            pid: 123,
            writes: Mutex::new(Vec::new()),
            kills: Mutex::new(Vec::new()),
            auto_exit_on_kill: AtomicBool::new(true),
            throw_kill: AtomicBool::new(false),
            on_kill: Mutex::new(None),
            data_listeners: Arc::new(Mutex::new(Vec::new())),
            exit_listeners: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn emit_data(&self, data: &str) {
        for listener in self.data_listeners.lock().iter() {
            listener(data.to_string());
        }
    }

    fn emit_exit(&self, exit_code: i32, signal: Option<u32>) {
        for listener in self.exit_listeners.lock().iter() {
            listener(Some(exit_code), signal);
        }
    }
}

impl PtyTerminal for FakePty {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn write(&self, data: &str) -> Result<(), String> {
        self.writes.lock().push(data.to_string());
        Ok(())
    }

    fn kill(&self, signal: &str) -> Result<(), String> {
        if self.throw_kill.load(SeqCst) {
            return Err("process raced".to_string());
        }
        self.kills.lock().push(signal.to_string());
        if let Some(on_kill) = self.on_kill.lock().clone() {
            on_kill();
        }
        if self.auto_exit_on_kill.load(SeqCst) {
            self.emit_exit(0, Some(if signal == "SIGKILL" { 9 } else { 15 }));
        }
        Ok(())
    }

    fn on_data(&self, listener: Arc<dyn Fn(String) + Send + Sync>) -> Box<dyn Fn() + Send + Sync> {
        self.data_listeners.lock().push(listener.clone());
        let listeners = self.data_listeners.clone();
        Box::new(move || {
            listeners
                .lock()
                .retain(|entry| !Arc::ptr_eq(entry, &listener));
        })
    }

    fn on_exit(
        &self,
        listener: Arc<dyn Fn(Option<i32>, Option<u32>) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync> {
        self.exit_listeners.lock().push(listener.clone());
        let listeners = self.exit_listeners.clone();
        Box::new(move || {
            listeners
                .lock()
                .retain(|entry| !Arc::ptr_eq(entry, &listener));
        })
    }
}

/// A scripted process inspector (TS `FakeInspector`).
struct FakeInspector {
    pgid: Mutex<Option<u32>>,
    waiting: AtomicBool,
    root: Mutex<Option<ProcessIdentity>>,
    members: Mutex<Vec<ProcessIdentity>>,
    session_members: Mutex<Vec<ProcessIdentity>>,
    alive: Mutex<HashSet<u32>>,
    groups: Mutex<Vec<(u32, SubprocessTerminalSignal)>>,
    processes: Mutex<Vec<(u32, TerminalKillSignal)>>,
    throw_group: AtomicBool,
    throw_process: AtomicBool,
    remove_on_signal: AtomicBool,
    tree_override: Mutex<Option<Arc<dyn Fn() -> Vec<ProcessIdentity> + Send + Sync>>>,
    session_override: Mutex<Option<Arc<dyn Fn() -> Vec<ProcessIdentity> + Send + Sync>>>,
    alive_override: Mutex<Option<Arc<dyn Fn(&ProcessIdentity) -> bool + Send + Sync>>>,
    process_override:
        Mutex<Option<Arc<dyn Fn(&ProcessIdentity, TerminalKillSignal) + Send + Sync>>>,
}

impl FakeInspector {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            pgid: Mutex::new(Some(456)),
            waiting: AtomicBool::new(false),
            root: Mutex::new(Some(ProcessIdentity {
                pid: 123,
                started: "shell".to_string(),
            })),
            members: Mutex::new(Vec::new()),
            session_members: Mutex::new(Vec::new()),
            alive: Mutex::new(HashSet::new()),
            groups: Mutex::new(Vec::new()),
            processes: Mutex::new(Vec::new()),
            throw_group: AtomicBool::new(false),
            throw_process: AtomicBool::new(false),
            remove_on_signal: AtomicBool::new(true),
            tree_override: Mutex::new(None),
            session_override: Mutex::new(None),
            alive_override: Mutex::new(None),
            process_override: Mutex::new(None),
        })
    }
}

impl ProcessInspector for FakeInspector {
    fn foreground_pgid(&self, _shell_pid: u32) -> Option<u32> {
        *self.pgid.lock()
    }

    fn is_stdin_waiting(&self, _pgid: u32) -> bool {
        self.waiting.load(SeqCst)
    }

    fn process_tree(&self, _root_pid: u32) -> Vec<ProcessIdentity> {
        if let Some(override_fn) = self.tree_override.lock().clone() {
            return override_fn();
        }
        match &*self.root.lock() {
            Some(root) => {
                let mut members = vec![root.clone()];
                members.extend(self.members.lock().iter().cloned());
                members
            }
            None => self.members.lock().clone(),
        }
    }

    fn process_session(&self, _session_id: u32) -> Vec<ProcessIdentity> {
        if let Some(override_fn) = self.session_override.lock().clone() {
            return override_fn();
        }
        self.session_members.lock().clone()
    }

    fn is_alive(&self, identity: &ProcessIdentity) -> bool {
        if let Some(override_fn) = self.alive_override.lock().clone() {
            return override_fn(identity);
        }
        self.alive.lock().contains(&identity.pid)
    }

    fn signal_group(&self, pgid: u32, signal: SubprocessTerminalSignal) {
        if self.throw_group.load(SeqCst) {
            panic!("group failed");
        }
        self.groups.lock().push((pgid, signal));
    }

    fn signal_process(&self, identity: &ProcessIdentity, signal: TerminalKillSignal) {
        if let Some(override_fn) = self.process_override.lock().clone() {
            return override_fn(identity, signal);
        }
        if self.throw_process.load(SeqCst) {
            panic!("process raced");
        }
        if !self.is_alive(identity) {
            return;
        }
        self.processes.lock().push((identity.pid, signal));
        if self.remove_on_signal.load(SeqCst) {
            self.alive.lock().remove(&identity.pid);
        }
    }
}

async fn done_of(handle: &Arc<LocalTerminalHandle>) -> SubprocessOutcome {
    handle.done().await.expect("done")
}

// ---- host exit ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_kills_descendants_around_the_shell_during_synchronous_host_exit() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let first = ProcessIdentity {
        pid: 124,
        started: "first".to_string(),
    };
    let late = ProcessIdentity {
        pid: 125,
        started: "late".to_string(),
    };
    inspector.members.lock().push(first.clone());
    inspector.alive.lock().insert(pty.pid);
    inspector.alive.lock().insert(first.pid);
    // Adopt `late` while the shell itself is being signalled; the override
    // preserves the original signal body plus the side effect.
    *inspector.process_override.lock() = Some({
        let inspector = inspector.clone();
        let first = first.clone();
        let late = late.clone();
        Arc::new(move |identity, signal| {
            if inspector.alive.lock().contains(&identity.pid) {
                inspector.processes.lock().push((identity.pid, signal));
                if inspector.remove_on_signal.load(SeqCst) {
                    inspector.alive.lock().remove(&identity.pid);
                }
            }
            if identity.pid == 123 {
                inspector
                    .members
                    .lock()
                    .extend([first.clone(), late.clone()]);
                inspector.alive.lock().insert(late.pid);
            }
        })
    });
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);

    handle.terminate_for_host_exit();
    assert_eq!(
        *inspector.processes.lock(),
        vec![
            (124, TerminalKillSignal::SigKill),
            (123, TerminalKillSignal::SigKill),
            (125, TerminalKillSignal::SigKill),
        ]
    );
    assert!(pty.kills.lock().is_empty());

    pty.emit_exit(0, None);
    handle.terminate_for_host_exit();
    assert!(pty.kills.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uses_captured_identities_and_contains_shell_races_when_final_inspection_fails() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let captured = ProcessIdentity {
        pid: 124,
        started: "captured".to_string(),
    };
    inspector.members.lock().push(captured);
    inspector.alive.lock().insert(pty.pid);
    inspector.alive.lock().insert(124);
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);
    handle.inspect_foreground().await.expect("inspect");
    *inspector.tree_override.lock() = Some(Arc::new(|| panic!("process table unavailable")));
    inspector.throw_process.store(true, SeqCst);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        handle.terminate_for_host_exit();
    }));
    assert!(result.is_ok());
    assert!(inspector.processes.lock().is_empty());
    assert!(pty.kills.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uses_node_pty_only_when_the_shell_start_identity_was_unavailable() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    *inspector.root.lock() = None;
    let handle = LocalTerminalHandle::new(pty.clone(), inspector, 10);

    handle.terminate_for_host_exit();
    assert_eq!(*pty.kills.lock(), vec!["SIGKILL".to_string()]);

    let racing_pty = FakePty::new();
    let racing_inspector = FakeInspector::new();
    *racing_inspector.root.lock() = None;
    racing_pty.throw_kill.store(true, SeqCst);
    let racing_handle = LocalTerminalHandle::new(racing_pty.clone(), racing_inspector, 10);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        racing_handle.terminate_for_host_exit();
    }));
    assert!(result.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn does_not_signal_a_recycled_terminal_root_before_its_delayed_exit_callback() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    inspector.alive.lock().insert(pty.pid);
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);
    *inspector.root.lock() = Some(ProcessIdentity {
        pid: 123,
        started: "recycled".to_string(),
    });
    *inspector.alive_override.lock() = Some(Arc::new(|identity| identity.started == "recycled"));

    handle.terminate_for_host_exit();

    assert!(inspector.processes.lock().is_empty());
    assert!(pty.kills.lock().is_empty());
}

// ---- bridging ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridges_terminal_bytes_foreground_control_and_signalled_exit_facts() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    inspector.waiting.store(true, SeqCst);
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);
    let mut output = handle.output();

    pty.emit_data("hello €");
    handle.write("input\r").await.expect("write");
    assert_eq!(*pty.writes.lock(), vec!["input\r".to_string()]);
    assert_eq!(
        handle.inspect_foreground().await.expect("inspect"),
        Some(dsh_subprocess::SubprocessTerminalForeground {
            process_group_id: 456,
            input_waiting: true,
        })
    );
    assert_eq!(
        handle
            .signal_foreground(SubprocessTerminalSignal::SigInt)
            .await
            .expect("signal"),
        456
    );
    assert_eq!(
        *inspector.groups.lock(),
        vec![(456, SubprocessTerminalSignal::SigInt)]
    );

    pty.emit_exit(7, Some(9));
    pty.emit_exit(0, None);
    assert_eq!(
        done_of(&handle).await,
        SubprocessOutcome {
            exit_code: None,
            signal: Some("SIGKILL".to_string())
        }
    );
    handle.terminate().await.expect("terminate");
    let mut chunks: Vec<u8> = Vec::new();
    while let Some(chunk) = output.next().await {
        chunks.extend(chunk);
    }
    assert_eq!(String::from_utf8_lossy(&chunks), "hello €");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_unsafe_foreground_signals_and_writes_after_exit() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);
    *inspector.pgid.lock() = Some(handle.pid());
    let error = handle
        .signal_foreground(SubprocessTerminalSignal::SigKill)
        .await
        .err()
        .expect("SIGKILL refused");
    assert!(error.contains("terminate the terminal session"), "{error}");
    *inspector.pgid.lock() = None;
    assert!(
        handle
            .inspect_foreground()
            .await
            .expect("inspect")
            .is_none()
    );
    let error = handle
        .signal_foreground(SubprocessTerminalSignal::SigTerm)
        .await
        .err()
        .expect("cannot resolve");
    assert!(error.contains("cannot resolve"), "{error}");

    pty.emit_exit(3, None);
    assert_eq!(
        done_of(&handle).await,
        SubprocessOutcome {
            exit_code: Some(3),
            signal: None
        }
    );
    handle.terminate().await.expect("terminate");
    let error = handle.write("late").await.err().expect("has exited");
    assert!(error.contains("has exited"), "{error}");
}

// ---- teardown ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_the_shell_alive_until_forced_descendants_leave() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    inspector.members.lock().push(ProcessIdentity {
        pid: 124,
        started: "child".to_string(),
    });
    inspector.alive.lock().insert(124);
    inspector.remove_on_signal.store(false, SeqCst);
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 60);

    // Drive termination in the background and assert the intermediate
    // state: descendants escalated to KILL while the shell is untouched.
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    {
        let handle = handle.clone();
        tokio::spawn(async move {
            let _ = handle.terminate().await;
            let _ = done_tx.send(());
        });
    }
    tokio::time::sleep(std::time::Duration::from_millis(70)).await;
    assert!(
        inspector
            .processes
            .lock()
            .contains(&(124, TerminalKillSignal::SigKill))
    );
    assert!(pty.kills.lock().is_empty());

    // The survivor leaves during the KILL grace; cleanup completes through
    // the shell stage.
    inspector.alive.lock().remove(&124);
    tokio::time::timeout(std::time::Duration::from_secs(5), &mut done_rx)
        .await
        .expect("terminate settles")
        .expect("terminate");
    assert_eq!(*pty.kills.lock(), vec!["SIGTERM".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_an_early_exit_wait_pending_through_descendant_cleanup() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    inspector.members.lock().push(ProcessIdentity {
        pid: 124,
        started: "child".to_string(),
    });
    inspector.alive.lock().insert(124);
    inspector.remove_on_signal.store(false, SeqCst);
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 20);
    pty.emit_exit(0, None);
    let waiting = handle.terminate();
    let settled = Arc::new(AtomicBool::new(false));
    {
        let settled = settled.clone();
        let waiting = Box::pin(waiting);
        tokio::spawn(async move {
            let _ = waiting.await;
            settled.store(true, SeqCst);
        });
    }
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(!settled.load(SeqCst));

    inspector.alive.lock().remove(&124);
    handle.terminate().await.expect("terminate");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleans_a_same_session_descendant_after_the_top_level_shell_exits_naturally() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let disowned = ProcessIdentity {
        pid: 124,
        started: "disowned".to_string(),
    };
    *inspector.session_override.lock() = Some({
        let inspector = inspector.clone();
        let disowned = disowned.clone();
        Arc::new(move || {
            if inspector.alive.lock().contains(&disowned.pid) {
                vec![disowned.clone()]
            } else {
                Vec::new()
            }
        })
    });
    inspector.alive.lock().insert(124);
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 20);

    pty.emit_exit(0, None);

    handle.terminate().await.expect("terminate");
    assert_eq!(
        *inspector.processes.lock(),
        vec![(124, TerminalKillSignal::SigTerm)]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retains_an_inspected_descendant_after_it_reparents_away_from_the_shell() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let descendant = ProcessIdentity {
        pid: 124,
        started: "observed".to_string(),
    };
    inspector.members.lock().push(descendant);
    inspector.alive.lock().insert(124);
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 20);

    handle.inspect_foreground().await.expect("inspect");
    inspector.members.lock().clear();
    pty.emit_exit(0, None);

    handle.terminate().await.expect("terminate");
    assert_eq!(
        *inspector.processes.lock(),
        vec![(124, TerminalKillSignal::SigTerm)]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn does_not_adopt_the_children_of_a_recycled_shell_pid() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);

    pty.emit_exit(0, None);
    let imposter_child = ProcessIdentity {
        pid: 999,
        started: "imposter-child".to_string(),
    };
    *inspector.root.lock() = Some(ProcessIdentity {
        pid: 123,
        started: "imposter".to_string(),
    });
    inspector.members.lock().push(imposter_child);
    inspector.alive.lock().insert(999);

    handle.terminate().await.expect("terminate");
    assert!(inspector.processes.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adopts_nothing_when_the_shell_identity_was_never_observable() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    *inspector.root.lock() = None;
    let orphan = ProcessIdentity {
        pid: 321,
        started: "unverifiable".to_string(),
    };
    inspector.members.lock().push(orphan);
    inspector.alive.lock().insert(321);
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);

    handle.terminate().await.expect("terminate");
    assert!(inspector.processes.lock().is_empty());
    assert_eq!(*pty.kills.lock(), vec!["SIGTERM".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rescans_for_descendants_forked_during_term() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let root = ProcessIdentity {
        pid: 123,
        started: "shell".to_string(),
    };
    let reads = Arc::new(std::sync::atomic::AtomicU32::new(0));
    *inspector.tree_override.lock() = Some({
        let inspector = inspector.clone();
        let root = root.clone();
        let reads = reads.clone();
        Arc::new(move || {
            let read = reads.fetch_add(1, SeqCst) + 1;
            match read {
                1 => vec![root.clone()],
                2 => {
                    inspector.alive.lock().insert(124);
                    vec![
                        root.clone(),
                        ProcessIdentity {
                            pid: 124,
                            started: "first".to_string(),
                        },
                    ]
                }
                3 => {
                    inspector.alive.lock().insert(125);
                    vec![
                        root.clone(),
                        ProcessIdentity {
                            pid: 125,
                            started: "late".to_string(),
                        },
                    ]
                }
                _ => Vec::new(),
            }
        })
    });
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);
    handle.terminate().await.expect("terminate");
    assert_eq!(
        *inspector.processes.lock(),
        vec![
            (124, TerminalKillSignal::SigTerm),
            (125, TerminalKillSignal::SigKill)
        ]
    );
    assert_eq!(*pty.kills.lock(), vec!["SIGTERM".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sweeps_a_same_session_descendant_forked_while_the_shell_handles_term() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let late = ProcessIdentity {
        pid: 124,
        started: "shell-term-trap".to_string(),
    };
    *pty.on_kill.lock() = Some({
        let inspector = inspector.clone();
        let late = late.clone();
        Arc::new(move || {
            inspector.session_members.lock().push(late.clone());
            inspector.alive.lock().insert(late.pid);
        })
    });
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);

    handle.terminate().await.expect("terminate");

    assert_eq!(
        *inspector.processes.lock(),
        vec![(late.pid, TerminalKillSignal::SigTerm)]
    );
    assert_eq!(*pty.kills.lock(), vec!["SIGTERM".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_failed_cleanup_after_a_surviving_descendant_leaves() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let late = ProcessIdentity {
        pid: 124,
        started: "shell-term-survivor".to_string(),
    };
    inspector.remove_on_signal.store(false, SeqCst);
    *pty.on_kill.lock() = Some({
        let inspector = inspector.clone();
        let late = late.clone();
        Arc::new(move || {
            inspector.session_members.lock().push(late.clone());
            inspector.alive.lock().insert(late.pid);
        })
    });
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 10);

    let error = handle.terminate().await.err().expect("surviving");
    assert!(error.contains("surviving pids: 124"), "{error}");

    inspector.alive.lock().remove(&late.pid);
    handle.terminate().await.expect("retry");
    assert_eq!(
        *inspector.processes.lock(),
        vec![
            (late.pid, TerminalKillSignal::SigTerm),
            (late.pid, TerminalKillSignal::SigKill),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retains_captured_descendants_after_reparenting() {
    let pty = FakePty::new();
    let inspector = FakeInspector::new();
    let captured = ProcessIdentity {
        pid: 124,
        started: "captured".to_string(),
    };
    let root = ProcessIdentity {
        pid: 123,
        started: "shell".to_string(),
    };
    let reads = Arc::new(std::sync::atomic::AtomicU32::new(0));
    inspector.alive.lock().insert(124);
    *inspector.tree_override.lock() = Some({
        let root = root.clone();
        let captured = captured.clone();
        let reads = reads.clone();
        Arc::new(move || {
            let read = reads.fetch_add(1, SeqCst) + 1;
            match read {
                1 => vec![root.clone()],
                2 => vec![root.clone(), captured.clone()],
                _ => Vec::new(),
            }
        })
    });
    *inspector.process_override.lock() = Some({
        let inspector = inspector.clone();
        Arc::new(move |identity, signal| {
            inspector.processes.lock().push((identity.pid, signal));
            if signal == TerminalKillSignal::SigKill {
                inspector.alive.lock().remove(&identity.pid);
            }
        })
    });
    let handle = LocalTerminalHandle::new(pty.clone(), inspector.clone(), 20);
    handle.terminate().await.expect("terminate");
    assert_eq!(
        *inspector.processes.lock(),
        vec![
            (124, TerminalKillSignal::SigTerm),
            (124, TerminalKillSignal::SigKill)
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_a_top_level_process_that_ignores_escalation() {
    let pty = FakePty::new();
    pty.auto_exit_on_kill.store(false, SeqCst);
    let handle = LocalTerminalHandle::new(pty.clone(), FakeInspector::new(), 10);
    let error = handle.terminate().await.err().expect("surviving shell");
    assert!(error.contains("surviving pid: 123"), "{error}");
    assert_eq!(
        *pty.kills.lock(),
        vec!["SIGTERM".to_string(), "SIGKILL".to_string()]
    );

    pty.emit_exit(0, Some(999));
    assert_eq!(
        done_of(&handle).await,
        SubprocessOutcome {
            exit_code: None,
            signal: None
        }
    );
    handle.terminate().await.expect("terminate");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contains_process_races_while_reporting_surviving_descendants() {
    let pty = FakePty::new();
    pty.throw_kill.store(true, SeqCst);
    let inspector = FakeInspector::new();
    inspector.members.lock().push(ProcessIdentity {
        pid: 124,
        started: "child".to_string(),
    });
    inspector.alive.lock().insert(124);
    inspector.throw_process.store(true, SeqCst);
    let handle = LocalTerminalHandle::new(pty, inspector, 1);
    let error = handle.terminate().await.err().expect("surviving");
    assert!(error.contains("surviving pids: 124"), "{error}");
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_captures_output_from_a_naturally_exiting_real_conpty() {
    let runtime = LocalSubprocessRuntime::new();
    let handle = runtime
        .spawn_terminal(SubprocessTerminalSpawnSpec {
            argv: vec![r"C:\Windows\System32\whoami.exe".to_string()],
            cwd: std::env::current_dir()
                .expect("current directory")
                .to_string_lossy()
                .into_owned(),
            env: None,
            rows: 24,
            cols: 80,
            grace_ms: 1_000,
            signal: None,
        })
        .await
        .expect("real ConPTY allocation");
    let mut output = handle.output();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), handle.done())
        .await
        .expect("cmd exit deadline")
        .expect("cmd outcome");
    assert_eq!(outcome.exit_code, Some(0));
    let captured = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut captured = String::new();
        while let Some(chunk) = output.next().await {
            captured.push_str(&String::from_utf8_lossy(&chunk));
        }
        captured
    })
    .await
    .expect("output EOF deadline");
    handle.terminate().await.expect("cleanup");
    assert!(
        !captured.trim().is_empty(),
        "PTY output missing from naturally exiting child: {captured:?}"
    );
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_allocates_a_real_conpty_and_round_trips_cmd_output() {
    let runtime = LocalSubprocessRuntime::new();
    let handle = runtime
        .spawn_terminal(SubprocessTerminalSpawnSpec {
            argv: vec![
                r"C:\Windows\System32\cmd.exe".to_string(),
                "/Q".to_string(),
                "/K".to_string(),
            ],
            cwd: std::env::current_dir()
                .expect("current directory")
                .to_string_lossy()
                .into_owned(),
            env: None,
            rows: 24,
            cols: 80,
            grace_ms: 1_000,
            signal: None,
        })
        .await
        .expect("real ConPTY allocation");
    assert!(handle.pid() > 0);

    let mut output = handle.output();
    handle
        .write("@echo off\r\n")
        .await
        .expect("disable command echo");
    handle
        .write("echo DSH_REAL_CONPTY_OUTPUT\r\n")
        .await
        .expect("write command through PTY");

    let captured = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut captured = String::new();
        while let Some(chunk) = output.next().await {
            captured.push_str(&String::from_utf8_lossy(&chunk));
            if captured.contains("DSH_REAL_CONPTY_OUTPUT") {
                break;
            }
        }
        captured
    })
    .await
    .expect("cmd output deadline");
    assert!(
        captured.contains("DSH_REAL_CONPTY_OUTPUT"),
        "PTY output did not contain command result: {captured:?}"
    );

    handle.write("exit\r").await.expect("exit shell");
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), handle.done())
        .await
        .expect("cmd exit deadline")
        .expect("cmd outcome");
    assert_eq!(outcome.exit_code, Some(0));
    handle.terminate().await.expect("idempotent cleanup");
}

#[cfg(windows)]
fn windows_process_exists(pid: u32) -> bool {
    let output = std::process::Command::new(r"C:\Windows\System32\tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("tasklist runs");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(windows)]
fn force_kill_windows_tree(pid: u32) {
    let _ = std::process::Command::new(r"C:\Windows\System32\taskkill.exe")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

#[cfg(windows)]
async fn wait_for_windows_process_to_exit(pid: u32) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !windows_process_exists(pid) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    false
}

#[cfg(windows)]
#[test]
fn dropping_runtime_and_handle_quiesces_the_real_conpty_tree() {
    let stage_file = std::env::temp_dir().join(format!(
        "dsh-conpty-drop-stage-{}-{}.txt",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "conpty_drop_child", "--nocapture"])
        .env("DSH_CONPTY_DROP_CHILD", "1")
        .env("DSH_CONPTY_DROP_STAGE", &stage_file)
        .spawn()
        .expect("spawn isolated ConPTY drop child");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    let stage = std::fs::read_to_string(&stage_file).unwrap_or_else(|_| "not-started".to_string());
    if status.is_none() {
        force_kill_windows_tree(child.id());
        let _ = child.wait();
    }
    let _ = std::fs::remove_file(stage_file);
    let status = status.unwrap_or_else(|| panic!("ConPTY drop child hung after stage {stage:?}"));
    assert!(
        status.success(),
        "ConPTY drop child failed after stage {stage:?}: {status}"
    );
    assert_eq!(stage, "done");
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conpty_drop_child() {
    if std::env::var_os("DSH_CONPTY_DROP_CHILD").is_none() {
        return;
    }
    let stage_file =
        std::path::PathBuf::from(std::env::var_os("DSH_CONPTY_DROP_STAGE").expect("stage file"));
    std::fs::write(&stage_file, "starting").expect("starting stage");
    let runtime = LocalSubprocessRuntime::new();
    let handle = runtime
        .spawn_terminal(SubprocessTerminalSpawnSpec {
            argv: vec![
                r"C:\Windows\System32\cmd.exe".to_string(),
                "/Q".to_string(),
                "/K".to_string(),
            ],
            cwd: std::env::current_dir()
                .expect("current directory")
                .to_string_lossy()
                .into_owned(),
            env: None,
            rows: 24,
            cols: 80,
            grace_ms: 1_000,
            signal: None,
        })
        .await
        .expect("real ConPTY allocation");
    let root_pid = handle.pid();
    assert!(windows_process_exists(root_pid));
    std::fs::write(&stage_file, format!("spawned:{root_pid}")).expect("spawned stage");

    drop(handle);
    std::fs::write(&stage_file, "handle-dropped").expect("handle stage");
    drop(runtime);
    std::fs::write(&stage_file, "runtime-dropped").expect("runtime stage");

    assert!(wait_for_windows_process_to_exit(root_pid).await);
    std::fs::write(&stage_file, "done").expect("done stage");
}
