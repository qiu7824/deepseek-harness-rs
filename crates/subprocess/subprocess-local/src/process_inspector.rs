//! Platform process-table inspection for terminal readiness, signals, and
//! teardown. Rust port of `packages/subprocess/subprocess-local/src/
//! process-inspector.ts`.
//!
//! # Deviations
//!
//! - The real OS bindings (`DEFAULT_INTERNALS`) are not wired yet: no
//!   terminal backend consumes them until the PTY milestone; the injected
//!   boundary is fully exercised by unit tests instead.
//! - Signals ride the seam's `SubprocessTerminalSignal` (group) and a
//!   dedicated `TerminalKillSignal` (exact identity: TERM/KILL).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dsh_subprocess::SubprocessTerminalSignal;

/// PID plus start identity, preventing teardown escalation after PID reuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started: String,
}

/// The exact-identity kill vocabulary (TS `'SIGTERM' | 'SIGKILL'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKillSignal {
    SigTerm,
    SigKill,
}

impl TerminalKillSignal {
    pub fn as_str(&self) -> &'static str {
        match self {
            TerminalKillSignal::SigTerm => "SIGTERM",
            TerminalKillSignal::SigKill => "SIGKILL",
        }
    }
}

/// Injectable OS process operations used by one local PTY session (TS
/// `ProcessInspectorInternals`).
pub trait ProcessInspectorInternals: Send + Sync {
    fn read_file(&self, path: &str) -> Result<String, String>;
    fn read_dir(&self, path: &str) -> Result<Vec<String>, String>;
    fn open(&self, path: &str) -> Result<u32, String>;
    fn read(&self, fd: u32, length: usize, position: usize) -> Result<Vec<u8>, String>;
    fn close(&self, fd: u32);
    fn exec(&self, file: &str, args: &[&str]) -> Result<String, String>;
    fn kill(&self, pid: i32, signal: &str);
}

/// Injectable OS process operations used by one local PTY session (TS
/// `ProcessInspector`).
pub trait ProcessInspector: Send + Sync {
    fn foreground_pgid(&self, shell_pid: u32) -> Option<u32>;
    fn is_stdin_waiting(&self, pgid: u32) -> bool;
    /// Return the root and its current transitive descendants, children
    /// first.
    fn process_tree(&self, root_pid: u32) -> Vec<ProcessIdentity>;
    /// Return current members of one POSIX process session when the
    /// platform exposes them.
    fn process_session(&self, session_id: u32) -> Vec<ProcessIdentity>;
    /// Return whether the exact identity remains a non-quiescent process.
    fn is_alive(&self, identity: &ProcessIdentity) -> bool;
    fn signal_group(&self, pgid: u32, signal: SubprocessTerminalSignal);
    fn signal_process(&self, identity: &ProcessIdentity, signal: TerminalKillSignal);
}

/// The shared POSIX signalling body (TS `PosixProcessInspector`).
fn posix_signal_group(
    internals: &dyn ProcessInspectorInternals,
    pgid: u32,
    signal: SubprocessTerminalSignal,
) {
    internals.kill(-(pgid as i32), signal.as_str());
}

fn posix_signal_process(
    inspector: &dyn ProcessInspector,
    internals: &dyn ProcessInspectorInternals,
    identity: &ProcessIdentity,
    signal: TerminalKillSignal,
) {
    if inspector.is_alive(identity) {
        internals.kill(identity.pid as i32, signal.as_str());
    }
}

/// Parsed Linux `/proc/<pid>/stat` fields (TS `ProcStat`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProcStat {
    pub pid: u32,
    pub parent_pid: u32,
    pub pgrp: u32,
    pub session: u32,
    pub state: char,
    pub tpgid: i64,
    pub started: String,
}

/// Parse fields used from Linux `/proc/<pid>/stat`, including parenthesized
/// comm text (TS `parseProcStat`).
pub fn parse_proc_stat(text: &str) -> Option<ProcStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if open == 0 || close <= open {
        return None;
    }
    let pid: u32 = text[..open].trim().parse().ok()?;
    let rest: Vec<&str> = text[close + 2..].split_whitespace().collect();
    let state_field = rest.first()?;
    if state_field.chars().count() != 1 {
        return None;
    }
    let state = state_field.chars().next()?;
    if rest.len() <= 19 {
        return None;
    }
    let parent_pid: u32 = rest.get(1)?.parse().ok()?;
    let pgrp: u32 = rest.get(2)?.parse().ok()?;
    let session: u32 = rest.get(3)?.parse().ok()?;
    let tpgid: i64 = rest.get(5)?.parse().ok()?;
    let started = (*rest.get(19)?).to_string();
    Some(ProcStat {
        pid,
        parent_pid,
        pgrp,
        session,
        state,
        tpgid,
        started,
    })
}

fn read_linux_stat(internals: &dyn ProcessInspectorInternals, pid: u32) -> Option<ProcStat> {
    parse_proc_stat(&internals.read_file(&format!("/proc/{pid}/stat")).ok()?)
}

/// Report whether a Linux process group has an executing member (TS
/// `linuxProcessGroupHasLiveMembers`).
pub fn linux_process_group_has_live_members(
    process_group_id: u32,
    internals: &dyn ProcessInspectorInternals,
) -> Option<bool> {
    let entries = internals.read_dir("/proc").ok()?;
    let mut matched = false;
    for entry in entries {
        if !entry.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = entry.parse::<u32>() else {
            continue;
        };
        let Some(stat) = read_linux_stat(internals, pid) else {
            continue;
        };
        if stat.pgrp != process_group_id {
            continue;
        }
        matched = true;
        if !matches!(stat.state, 'Z' | 'X' | 'x') {
            return Some(true);
        }
    }
    if matched { Some(false) } else { None }
}

fn numeric_entries(internals: &dyn ProcessInspectorInternals, path: &str) -> Vec<u32> {
    internals
        .read_dir(path)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|entry| entry.parse::<u32>().ok())
        .collect()
}

struct SyscallInfo {
    number: u32,
    args: Vec<u32>,
}

fn read_syscall(
    internals: &dyn ProcessInspectorInternals,
    pid: u32,
    tid: u32,
) -> Option<SyscallInfo> {
    let text = internals
        .read_file(&format!("/proc/{pid}/task/{tid}/syscall"))
        .ok()?;
    let text = text.trim();
    if text == "running" || text.starts_with("-1 ") {
        return None;
    }
    let fields: Vec<&str> = text.split_whitespace().collect();
    let number: u32 = fields.first()?.parse().ok()?;
    let args: Vec<u32> = fields
        .iter()
        .skip(1)
        .take(6)
        .map(|field| u32::from_str_radix(field.trim_start_matches("0x"), 16).unwrap_or(u32::MAX))
        .collect();
    if args.iter().any(|value| *value == u32::MAX) {
        return None;
    }
    Some(SyscallInfo { number, args })
}

fn read_memory(
    internals: &dyn ProcessInspectorInternals,
    pid: u32,
    address: u32,
    length: usize,
) -> Option<Vec<u8>> {
    let fd = internals.open(&format!("/proc/{pid}/mem")).ok()?;
    let result = internals.read(fd, length, address as usize).ok();
    internals.close(fd);
    result
}

fn fd_set_has_stdin(internals: &dyn ProcessInspectorInternals, pid: u32, address: u32) -> bool {
    if address == 0 {
        return false;
    }
    read_memory(internals, pid, address, 8)
        .and_then(|bytes| bytes.first().copied())
        .is_some_and(|first| first % 2 == 1)
}

fn poll_has_stdin(
    internals: &dyn ProcessInspectorInternals,
    pid: u32,
    address: u32,
    count: u32,
) -> bool {
    if address == 0 || count == 0 {
        return false;
    }
    let Some(memory) = read_memory(internals, pid, address, count.min(1024) as usize * 8) else {
        return false;
    };
    let mut offset = 0;
    while offset + 8 <= memory.len() {
        let events = i32::from_le_bytes(memory[offset..offset + 4].try_into().expect("4 bytes"));
        let revents =
            i16::from_le_bytes(memory[offset + 4..offset + 6].try_into().expect("2 bytes"));
        if events == 0 && (revents & 0x001) != 0 {
            return true;
        }
        offset += 8;
    }
    false
}

fn epoll_has_stdin(internals: &dyn ProcessInspectorInternals, pid: u32, epfd: u32) -> bool {
    internals
        .read_file(&format!("/proc/{pid}/fdinfo/{epfd}"))
        .map(|text| text.split('\n').any(|line| line.trim().starts_with("tfd:")))
        .unwrap_or(false)
}

#[derive(Clone, Copy)]
struct SyscallTable {
    read: u32,
    select: Option<u32>,
    pselect: u32,
    poll: Option<u32>,
    ppoll: u32,
    epoll_wait: Option<u32>,
    epoll_pwait: u32,
}

fn syscall_table(arch: &str) -> Option<SyscallTable> {
    match arch {
        "x64" => Some(SyscallTable {
            read: 0,
            select: Some(23),
            pselect: 270,
            poll: Some(7),
            ppoll: 271,
            epoll_wait: Some(232),
            epoll_pwait: 281,
        }),
        "arm64" => Some(SyscallTable {
            read: 63,
            select: None,
            pselect: 72,
            poll: None,
            ppoll: 73,
            epoll_wait: None,
            epoll_pwait: 22,
        }),
        _ => None,
    }
}

fn syscall_waits_on_stdin(
    internals: &dyn ProcessInspectorInternals,
    pid: u32,
    syscall: &SyscallInfo,
    table: &SyscallTable,
) -> bool {
    let a0 = syscall.args.first().copied().unwrap_or(0);
    let a1 = syscall.args.get(1).copied().unwrap_or(0);
    let a2 = syscall.args.get(2).copied().unwrap_or(0);
    if syscall.number == table.read {
        return a0 == 0;
    }
    if Some(syscall.number) == table.select || syscall.number == table.pselect {
        return a0 >= 1 && fd_set_has_stdin(internals, pid, a1);
    }
    if Some(syscall.number) == table.poll || syscall.number == table.ppoll {
        return a1 >= 1 && poll_has_stdin(internals, pid, a0, a1);
    }
    if Some(syscall.number) == table.epoll_wait || syscall.number == table.epoll_pwait {
        return a2 >= 1 && epoll_has_stdin(internals, pid, a0);
    }
    false
}

/// One rooted process-tree entry (pid + parent + start identity).
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessTreeEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub started: String,
}

/// Fold the table into one root's transitive descendants, children first
/// (TS `processTree`).
pub fn process_tree(entries: &[ProcessTreeEntry], root_pid: u32) -> Vec<ProcessIdentity> {
    let by_pid: HashMap<u32, &ProcessTreeEntry> =
        entries.iter().map(|entry| (entry.pid, entry)).collect();
    if !by_pid.contains_key(&root_pid) {
        return Vec::new();
    }
    let mut by_parent: HashMap<u32, Vec<&ProcessTreeEntry>> = HashMap::new();
    for entry in entries {
        by_parent.entry(entry.parent_pid).or_default().push(entry);
    }
    let mut visited: HashSet<u32> = HashSet::new();
    let mut result: Vec<ProcessIdentity> = Vec::new();
    fn visit(
        entry: &ProcessTreeEntry,
        by_parent: &HashMap<u32, Vec<&ProcessTreeEntry>>,
        visited: &mut HashSet<u32>,
        result: &mut Vec<ProcessIdentity>,
    ) {
        if !visited.insert(entry.pid) {
            return;
        }
        if let Some(children) = by_parent.get(&entry.pid) {
            for child in children {
                visit(child, by_parent, visited, result);
            }
        }
        result.push(ProcessIdentity {
            pid: entry.pid,
            started: entry.started.clone(),
        });
    }
    let root = by_pid[&root_pid];
    visit(root, &by_parent, &mut visited, &mut result);
    result
}

/// The Linux inspector (TS `LinuxProcessInspector`).
pub struct LinuxProcessInspector {
    arch: String,
    internals: Arc<dyn ProcessInspectorInternals>,
}

impl LinuxProcessInspector {
    pub fn new(arch: &str, internals: Arc<dyn ProcessInspectorInternals>) -> Self {
        Self {
            arch: arch.to_string(),
            internals,
        }
    }
}

impl ProcessInspector for LinuxProcessInspector {
    fn foreground_pgid(&self, shell_pid: u32) -> Option<u32> {
        let tpgid = read_linux_stat(self.internals.as_ref(), shell_pid)?.tpgid;
        (tpgid > 0).then_some(tpgid as u32)
    }

    fn is_stdin_waiting(&self, pgid: u32) -> bool {
        let Some(table) = syscall_table(&self.arch) else {
            return false;
        };
        for pid in numeric_entries(self.internals.as_ref(), "/proc") {
            let Some(stat) = read_linux_stat(self.internals.as_ref(), pid) else {
                continue;
            };
            if stat.pgrp != pgid {
                continue;
            }
            for tid in numeric_entries(self.internals.as_ref(), &format!("/proc/{pid}/task")) {
                if let Some(syscall) = read_syscall(self.internals.as_ref(), pid, tid) {
                    if syscall_waits_on_stdin(self.internals.as_ref(), pid, &syscall, &table) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn process_tree(&self, root_pid: u32) -> Vec<ProcessIdentity> {
        let entries: Vec<ProcessTreeEntry> = numeric_entries(self.internals.as_ref(), "/proc")
            .into_iter()
            .filter_map(|pid| {
                let stat = read_linux_stat(self.internals.as_ref(), pid)?;
                Some(ProcessTreeEntry {
                    pid,
                    parent_pid: stat.parent_pid,
                    started: stat.started,
                })
            })
            .collect();
        process_tree(&entries, root_pid)
    }

    fn process_session(&self, session_id: u32) -> Vec<ProcessIdentity> {
        numeric_entries(self.internals.as_ref(), "/proc")
            .into_iter()
            .filter_map(|pid| {
                let stat = read_linux_stat(self.internals.as_ref(), pid)?;
                (stat.session == session_id).then_some(ProcessIdentity {
                    pid,
                    started: stat.started,
                })
            })
            .collect()
    }

    fn is_alive(&self, identity: &ProcessIdentity) -> bool {
        let Some(stat) = read_linux_stat(self.internals.as_ref(), identity.pid) else {
            return false;
        };
        stat.started == identity.started && !matches!(stat.state, 'Z' | 'X' | 'x')
    }

    fn signal_group(&self, pgid: u32, signal: SubprocessTerminalSignal) {
        posix_signal_group(self.internals.as_ref(), pgid, signal);
    }

    fn signal_process(&self, identity: &ProcessIdentity, signal: TerminalKillSignal) {
        posix_signal_process(self, self.internals.as_ref(), identity, signal);
    }
}

fn mac_process_table(internals: &dyn ProcessInspectorInternals) -> Vec<ProcessTreeEntry> {
    internals
        .exec("/bin/ps", &["-axo", "pid=,ppid=,lstart="])
        .unwrap_or_default()
        .split('\n')
        .filter_map(|line| {
            let line = line.trim_start();
            let mut parts = line.splitn(3, char::is_whitespace);
            let pid = parts.next()?.parse::<u32>().ok()?;
            let parent_pid = parts.next()?.parse::<u32>().ok()?;
            let started = parts.next()?.trim_end().to_string();
            if started.is_empty() {
                return None;
            }
            Some(ProcessTreeEntry {
                pid,
                parent_pid,
                started,
            })
        })
        .collect()
}

/// The macOS inspector (TS `MacProcessInspector`).
pub struct MacProcessInspector {
    internals: Arc<dyn ProcessInspectorInternals>,
}

impl MacProcessInspector {
    pub fn new(internals: Arc<dyn ProcessInspectorInternals>) -> Self {
        Self { internals }
    }
}

impl ProcessInspector for MacProcessInspector {
    fn foreground_pgid(&self, shell_pid: u32) -> Option<u32> {
        let value = self
            .internals
            .exec("/bin/ps", &["-o", "tpgid=", "-p", &shell_pid.to_string()])
            .ok()?;
        let value = value.trim().parse::<i64>().ok()?;
        (value > 0).then_some(value as u32)
    }

    fn is_stdin_waiting(&self, _pgid: u32) -> bool {
        false
    }

    fn process_tree(&self, root_pid: u32) -> Vec<ProcessIdentity> {
        let entries = mac_process_table(self.internals.as_ref());
        process_tree(&entries, root_pid)
    }

    fn process_session(&self, _session_id: u32) -> Vec<ProcessIdentity> {
        Vec::new()
    }

    fn is_alive(&self, identity: &ProcessIdentity) -> bool {
        mac_process_table(self.internals.as_ref())
            .iter()
            .any(|entry| entry.pid == identity.pid && entry.started == identity.started)
    }

    fn signal_group(&self, pgid: u32, signal: SubprocessTerminalSignal) {
        posix_signal_group(self.internals.as_ref(), pgid, signal);
    }

    fn signal_process(&self, identity: &ProcessIdentity, signal: TerminalKillSignal) {
        posix_signal_process(self, self.internals.as_ref(), identity, signal);
    }
}

/// Create the supported platform inspector or fail at plugin load (TS
/// `createProcessInspector`).
pub fn create_process_inspector(
    platform: &str,
    arch: &str,
    internals: Arc<dyn ProcessInspectorInternals>,
) -> Result<Arc<dyn ProcessInspector>, String> {
    match platform {
        "linux" => Ok(Arc::new(LinuxProcessInspector::new(arch, internals))),
        "darwin" => Ok(Arc::new(MacProcessInspector::new(internals))),
        other => Err(format!(
            "subprocess-local: terminal inspection is unsupported on platform {other}"
        )),
    }
}
