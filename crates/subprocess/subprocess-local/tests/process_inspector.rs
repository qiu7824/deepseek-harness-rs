//! Rust port of the core
//! `packages/subprocess/subprocess-local/tests/process-inspector.spec.ts`
//! behaviors: /proc stat parsing, zombie-group liveness, rooted process
//! trees, exact-identity signalling, Linux syscall-based stdin-wait
//! detection, and the macOS ps-table inspector.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

use dsh_subprocess::SubprocessTerminalSignal;
use dsh_subprocess_local::{
    ProcessIdentity, ProcessInspectorInternals, TerminalKillSignal, create_process_inspector,
    linux_process_group_has_live_members, parse_proc_stat,
};
use parking_lot::Mutex;

/// Build a `/proc/<pid>/stat` line (TS `stat` helper).
fn stat(
    pid: u32,
    pgrp: u32,
    session: u32,
    tpgid: i64,
    started: &str,
    parent_pid: u32,
    state: &str,
) -> String {
    let mut rest: Vec<String> = vec![
        state.to_string(),
        parent_pid.to_string(),
        pgrp.to_string(),
        session.to_string(),
        "99".to_string(),
        tpgid.to_string(),
    ];
    while rest.len() < 19 {
        rest.push("0".to_string());
    }
    rest.push(started.to_string());
    format!("{pid} (command with space) {}", rest.join(" "))
}

/// Build a syscall line (TS `syscall` helper).
fn syscall(number: u32, args: &[u32]) -> String {
    let mut six: Vec<u32> = args.to_vec();
    while six.len() < 6 {
        six.push(0);
    }
    format!(
        "{number} {}",
        six.iter()
            .take(6)
            .map(|value| format!("0x{value:x}"))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

/// A fake internals boundary (TS `fakeInternals`).
struct FakeInternals {
    files: Mutex<HashMap<String, String>>,
    dirs: Mutex<HashMap<String, Vec<String>>>,
    memories: Mutex<HashMap<String, Vec<u8>>>,
    fds: Mutex<HashMap<u32, String>>,
    kills: Mutex<Vec<(i32, String)>>,
    next_fd: AtomicU32,
    ps: Mutex<String>,
    tpgid: Mutex<String>,
}

impl FakeInternals {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            files: Mutex::new(HashMap::new()),
            dirs: Mutex::new(HashMap::new()),
            memories: Mutex::new(HashMap::new()),
            fds: Mutex::new(HashMap::new()),
            kills: Mutex::new(Vec::new()),
            next_fd: AtomicU32::new(10),
            ps: Mutex::new(String::new()),
            tpgid: Mutex::new("0".to_string()),
        })
    }
}

impl ProcessInspectorInternals for FakeInternals {
    fn read_file(&self, path: &str) -> Result<String, String> {
        self.files
            .lock()
            .get(path)
            .cloned()
            .ok_or_else(|| format!("missing {path}"))
    }

    fn read_dir(&self, path: &str) -> Result<Vec<String>, String> {
        self.dirs
            .lock()
            .get(path)
            .cloned()
            .ok_or_else(|| format!("missing {path}"))
    }

    fn open(&self, path: &str) -> Result<u32, String> {
        if !self.memories.lock().contains_key(path) {
            return Err(format!("missing {path}"));
        }
        let fd = self.next_fd.fetch_add(1, SeqCst);
        self.fds.lock().insert(fd, path.to_string());
        Ok(fd)
    }

    fn read(&self, fd: u32, length: usize, position: usize) -> Result<Vec<u8>, String> {
        let path = self
            .fds
            .lock()
            .get(&fd)
            .cloned()
            .ok_or_else(|| "bad fd".to_string())?;
        let source = self
            .memories
            .lock()
            .get(&path)
            .cloned()
            .ok_or_else(|| "missing memory".to_string())?;
        let end = (position + length).min(source.len());
        Ok(source[position..end].to_vec())
    }

    fn close(&self, fd: u32) {
        self.fds.lock().remove(&fd);
    }

    fn exec(&self, _file: &str, args: &[&str]) -> Result<String, String> {
        if args.iter().any(|arg| arg.starts_with("tpgid=")) {
            return Ok(self.tpgid.lock().clone());
        }
        Ok(self.ps.lock().clone())
    }

    fn kill(&self, pid: i32, signal: &str) {
        self.kills.lock().push((pid, signal.to_string()));
    }
}

// ---- Linux ----

#[test]
fn treats_zombie_only_groups_as_quiescent_and_fails_closed_when_unobservable() {
    let fake = FakeInternals::new();
    assert_eq!(
        linux_process_group_has_live_members(77, fake.as_ref()),
        None
    );

    fake.dirs.lock().insert(
        "/proc".to_string(),
        vec!["self".into(), "10".into(), "11".into(), "12".into()],
    );
    fake.files
        .lock()
        .insert("/proc/10/stat".into(), stat(10, 77, 10, -1, "500", 1, "Z"));
    fake.files
        .lock()
        .insert("/proc/11/stat".into(), stat(11, 77, 10, -1, "501", 1, "X"));
    fake.files
        .lock()
        .insert("/proc/12/stat".into(), stat(12, 88, 12, -1, "502", 1, "S"));
    assert_eq!(
        linux_process_group_has_live_members(77, fake.as_ref()),
        Some(false)
    );
    assert_eq!(
        linux_process_group_has_live_members(99, fake.as_ref()),
        None
    );

    fake.files
        .lock()
        .insert("/proc/11/stat".into(), stat(11, 77, 10, -1, "501", 1, "S"));
    assert_eq!(
        linux_process_group_has_live_members(77, fake.as_ref()),
        Some(true)
    );
}

#[test]
fn parses_stat_safely_captures_only_the_rooted_tree_and_signals_identities() {
    assert!(parse_proc_stat("bad").is_none());
    assert!(parse_proc_stat("1 () ").is_none());
    assert!(parse_proc_stat("1 () S").is_none());
    assert!(parse_proc_stat(&stat(10, 20, 30, 40, "500", 1, "SS")).is_none());
    assert_eq!(
        parse_proc_stat(&stat(10, 20, 30, 40, "500", 1, "S")),
        Some(dsh_subprocess_local::ProcStat {
            pid: 10,
            parent_pid: 1,
            pgrp: 20,
            session: 30,
            state: 'S',
            tpgid: 40,
            started: "500".to_string(),
        })
    );

    let fake = FakeInternals::new();
    fake.dirs.lock().insert(
        "/proc".to_string(),
        vec![
            "x".into(),
            "10".into(),
            "11".into(),
            "12".into(),
            "13".into(),
            "14".into(),
        ],
    );
    fake.files
        .lock()
        .insert("/proc/10/stat".into(), stat(10, 20, 30, 40, "500", 1, "S"));
    fake.files
        .lock()
        .insert("/proc/11/stat".into(), stat(11, 21, 30, -1, "501", 1, "S"));
    fake.files
        .lock()
        .insert("/proc/12/stat".into(), stat(12, 22, 30, -1, "502", 10, "S"));
    fake.files
        .lock()
        .insert("/proc/13/stat".into(), stat(13, 23, 30, -1, "503", 12, "S"));
    let inspector = create_process_inspector("linux", "x64", fake.clone()).expect("inspector");

    assert_eq!(inspector.foreground_pgid(10), Some(40));
    assert_eq!(inspector.foreground_pgid(11), None);
    assert_eq!(inspector.foreground_pgid(99), None);
    assert_eq!(
        inspector.process_tree(10),
        vec![
            ProcessIdentity {
                pid: 13,
                started: "503".to_string()
            },
            ProcessIdentity {
                pid: 12,
                started: "502".to_string()
            },
            ProcessIdentity {
                pid: 10,
                started: "500".to_string()
            },
        ]
    );
    assert!(inspector.process_tree(99).is_empty());
    assert_eq!(
        inspector.process_session(30),
        vec![
            ProcessIdentity {
                pid: 10,
                started: "500".to_string()
            },
            ProcessIdentity {
                pid: 11,
                started: "501".to_string()
            },
            ProcessIdentity {
                pid: 12,
                started: "502".to_string()
            },
            ProcessIdentity {
                pid: 13,
                started: "503".to_string()
            },
        ]
    );
    assert!(inspector.process_session(99).is_empty());
    assert!(inspector.is_alive(&ProcessIdentity {
        pid: 10,
        started: "500".to_string()
    }));
    assert!(!inspector.is_alive(&ProcessIdentity {
        pid: 10,
        started: "old".to_string()
    }));
    inspector.signal_group(40, SubprocessTerminalSignal::SigInt);
    inspector.signal_process(
        &ProcessIdentity {
            pid: 10,
            started: "500".to_string(),
        },
        TerminalKillSignal::SigTerm,
    );
    inspector.signal_process(
        &ProcessIdentity {
            pid: 10,
            started: "old".to_string(),
        },
        TerminalKillSignal::SigKill,
    );
    assert_eq!(
        *fake.kills.lock(),
        vec![(-40, "SIGINT".to_string()), (10, "SIGTERM".to_string())]
    );
    fake.files
        .lock()
        .insert("/proc/10/stat".into(), stat(10, 20, 30, 40, "500", 1, "Z"));
    assert!(!inspector.is_alive(&ProcessIdentity {
        pid: 10,
        started: "500".to_string()
    }));
    inspector.signal_process(
        &ProcessIdentity {
            pid: 10,
            started: "500".to_string(),
        },
        TerminalKillSignal::SigKill,
    );
    assert_eq!(
        *fake.kills.lock(),
        vec![(-40, "SIGINT".to_string()), (10, "SIGTERM".to_string())]
    );
}

#[test]
fn detects_read_select_poll_and_epoll_waits_across_non_leader_threads() {
    let fake = FakeInternals::new();
    fake.dirs
        .lock()
        .insert("/proc".to_string(), vec!["100".into(), "101".into()]);
    fake.files
        .lock()
        .insert("/proc/100/stat".into(), stat(100, 77, 100, 77, "1", 1, "S"));
    fake.files
        .lock()
        .insert("/proc/101/stat".into(), stat(101, 77, 100, 77, "2", 1, "S"));
    fake.dirs
        .lock()
        .insert("/proc/100/task".to_string(), vec!["100".into()]);
    fake.dirs.lock().insert(
        "/proc/101/task".to_string(),
        vec!["101".into(), "102".into()],
    );
    let inspector = create_process_inspector("linux", "x64", fake.clone()).expect("inspector");

    fake.files
        .lock()
        .insert("/proc/100/task/100/syscall".into(), "running".to_string());
    fake.files
        .lock()
        .insert("/proc/101/task/101/syscall".into(), "-1 0x0".to_string());
    fake.files
        .lock()
        .insert("/proc/101/task/102/syscall".into(), syscall(0, &[0]));
    assert!(inspector.is_stdin_waiting(77));

    fake.files.lock().insert(
        "/proc/101/task/102/syscall".into(),
        syscall(270, &[1, 0x10]),
    );
    let mut fd_set = vec![0u8; 0x11];
    fd_set[0x10] = 1;
    fake.memories.lock().insert("/proc/101/mem".into(), fd_set);
    assert!(inspector.is_stdin_waiting(77));

    let mut poll = vec![0u8; 8];
    poll[0..4].copy_from_slice(&0i32.to_le_bytes());
    poll[4..6].copy_from_slice(&1i16.to_le_bytes());
    fake.files
        .lock()
        .insert("/proc/101/task/102/syscall".into(), syscall(7, &[0x20, 1]));
    let mut memory = vec![0u8; 0x20];
    memory.extend(poll);
    fake.memories.lock().insert("/proc/101/mem".into(), memory);
    assert!(inspector.is_stdin_waiting(77));

    fake.files.lock().insert(
        "/proc/101/task/102/syscall".into(),
        syscall(232, &[5, 0, 1]),
    );
    fake.files.lock().insert(
        "/proc/101/fdinfo/5".into(),
        "pos: 0\ntfd: 0 events: 19\n".to_string(),
    );
    assert!(inspector.is_stdin_waiting(77));
}

#[test]
fn fails_closed_on_unsupported_malformed_unreadable_or_non_stdin_waits() {
    let fake = FakeInternals::new();
    fake.dirs
        .lock()
        .insert("/proc".to_string(), vec!["100".into()]);
    fake.files
        .lock()
        .insert("/proc/100/stat".into(), stat(100, 77, 100, 77, "1", 1, "S"));
    fake.dirs
        .lock()
        .insert("/proc/100/task".to_string(), vec!["100".into()]);
    fake.files
        .lock()
        .insert("/proc/100/task/100/syscall".into(), syscall(0, &[2]));
    assert!(
        !create_process_inspector("linux", "mips", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );

    fake.files
        .lock()
        .insert("/proc/100/task/100/syscall".into(), syscall(270, &[1, 0]));
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );
    fake.files
        .lock()
        .insert("/proc/100/task/100/syscall".into(), syscall(7, &[0, 0]));
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );
    fake.files
        .lock()
        .insert("/proc/100/task/100/syscall".into(), syscall(7, &[0, 1]));
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );
    fake.files
        .lock()
        .insert("/proc/100/task/100/syscall".into(), syscall(7, &[0x20, 1]));
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );
    fake.files.lock().insert(
        "/proc/100/task/100/syscall".into(),
        syscall(232, &[9, 0, 1]),
    );
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );
    fake.files
        .lock()
        .insert("/proc/100/task/100/syscall".into(), syscall(999, &[]));
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );

    fake.files.lock().insert(
        "/proc/100/task/100/syscall".into(),
        "not-a-number 0x0".to_string(),
    );
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );
    fake.dirs.lock().remove("/proc/100/task");
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );
    fake.dirs
        .lock()
        .insert("/proc".to_string(), vec!["100".into(), "200".into()]);
    fake.files
        .lock()
        .insert("/proc/200/stat".into(), stat(200, 88, 200, 88, "2", 1, "S"));
    assert!(
        !create_process_inspector("linux", "x64", fake.clone())
            .expect("inspector")
            .is_stdin_waiting(77)
    );
}

#[test]
fn contains_unreadable_syscall_memory_and_fdinfo_boundaries() {
    let fake = FakeInternals::new();
    fake.dirs
        .lock()
        .insert("/proc".to_string(), vec!["100".into()]);
    fake.files
        .lock()
        .insert("/proc/100/stat".into(), stat(100, 77, 100, 77, "1", 1, "S"));
    fake.dirs
        .lock()
        .insert("/proc/100/task".to_string(), vec!["100".into()]);
    let inspector = create_process_inspector("linux", "x64", fake.clone()).expect("inspector");
    assert!(!inspector.is_stdin_waiting(77));

    fake.files.lock().insert(
        "/proc/100/task/100/syscall".into(),
        syscall(270, &[1, 0x10]),
    );
    assert!(!inspector.is_stdin_waiting(77));
    fake.files.lock().insert(
        "/proc/100/task/100/syscall".into(),
        syscall(232, &[5, 0, 1]),
    );
    assert!(!inspector.is_stdin_waiting(77));

    let mut no_stdin_poll = vec![0u8; 0x28];
    no_stdin_poll[0x20..0x24].copy_from_slice(&2i32.to_le_bytes());
    no_stdin_poll[0x24..0x26].copy_from_slice(&1i16.to_le_bytes());
    fake.memories
        .lock()
        .insert("/proc/100/mem".into(), no_stdin_poll);
    fake.files
        .lock()
        .insert("/proc/100/task/100/syscall".into(), syscall(7, &[0x20, 1]));
    assert!(!inspector.is_stdin_waiting(77));
}

// ---- macOS ----

#[test]
fn reads_tpgid_and_trees_contains_cycles_and_identity_fences_signals() {
    let fake = FakeInternals::new();
    *fake.tpgid.lock() = "55\n".to_string();
    *fake.ps.lock() = " 10 1 Mon Jul 21 10:00:00 2026\n 11 10 Mon Jul 21 10:00:01 2026\n 12 11 Mon Jul 21 10:00:02 2026\n 13 99 Mon Jul 21 10:00:03 2026\nmalformed\n".to_string();
    let inspector = create_process_inspector("darwin", "arm64", fake.clone()).expect("inspector");
    assert_eq!(inspector.foreground_pgid(10), Some(55));
    assert!(!inspector.is_stdin_waiting(55));
    assert_eq!(
        inspector.process_tree(10),
        vec![
            ProcessIdentity {
                pid: 12,
                started: "Mon Jul 21 10:00:02 2026".to_string()
            },
            ProcessIdentity {
                pid: 11,
                started: "Mon Jul 21 10:00:01 2026".to_string()
            },
            ProcessIdentity {
                pid: 10,
                started: "Mon Jul 21 10:00:00 2026".to_string()
            },
        ]
    );
    assert!(inspector.process_tree(99).is_empty());
    assert!(inspector.process_session(10).is_empty());
    assert!(inspector.is_alive(&ProcessIdentity {
        pid: 11,
        started: "Mon Jul 21 10:00:01 2026".to_string()
    }));
    inspector.signal_group(55, SubprocessTerminalSignal::SigTstp);
    inspector.signal_process(
        &ProcessIdentity {
            pid: 11,
            started: "Mon Jul 21 10:00:01 2026".to_string(),
        },
        TerminalKillSignal::SigKill,
    );
    inspector.signal_process(
        &ProcessIdentity {
            pid: 12,
            started: "missing".to_string(),
        },
        TerminalKillSignal::SigTerm,
    );
    assert_eq!(
        *fake.kills.lock(),
        vec![(-55, "SIGTSTP".to_string()), (11, "SIGKILL".to_string())]
    );

    // A two-entry cycle stays bounded by the visited set.
    *fake.ps.lock() =
        " 10 11 Mon Jul 21 10:00:00 2026\n 11 10 Mon Jul 21 10:00:01 2026\n".to_string();
    assert_eq!(
        inspector.process_tree(10),
        vec![
            ProcessIdentity {
                pid: 11,
                started: "Mon Jul 21 10:00:01 2026".to_string()
            },
            ProcessIdentity {
                pid: 10,
                started: "Mon Jul 21 10:00:00 2026".to_string()
            },
        ]
    );
}

#[test]
fn returns_undefined_for_missing_or_invalid_foreground_groups_and_rejects_unsupported_platforms() {
    let fake = FakeInternals::new();
    *fake.tpgid.lock() = "-1".to_string();
    assert_eq!(
        create_process_inspector("darwin", "arm64", fake.clone())
            .expect("inspector")
            .foreground_pgid(1),
        None
    );
    // exec throws: foreground resolution fails closed.
    *fake.tpgid.lock() = "gone-parse".to_string();
    assert_eq!(
        create_process_inspector("darwin", "arm64", fake.clone())
            .expect("inspector")
            .foreground_pgid(1),
        None
    );
    let error = create_process_inspector("win32", "x64", fake.clone())
        .err()
        .expect("unsupported");
    assert!(error.contains("unsupported on platform win32"), "{error}");
}
