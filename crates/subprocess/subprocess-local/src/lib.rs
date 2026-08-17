//! Local implementation of the subprocess capability seam: detached
//! process-tree spawn, tail-keep collection with spill files, tree-scoped
//! signalling, the SIGTERM→SIGKILL escalation, and the platform
//! terminal-process inspection/handle layer. Rust port of
//! `packages/subprocess/subprocess-local` (spawn.ts, index.ts,
//! process-inspector.ts, terminal.ts).

pub mod index;
pub mod invariant;
pub mod process_inspector;
pub mod spawn;
pub mod terminal;

pub use index::LocalSubprocessRuntime;
pub use process_inspector::{
    LinuxProcessInspector, MacProcessInspector, ProcStat, ProcessIdentity,
    ProcessInspector, ProcessInspectorInternals, ProcessTreeEntry, TerminalKillSignal,
    create_process_inspector, linux_process_group_has_live_members, parse_proc_stat,
    process_tree,
};
pub use spawn::{
    CollectorReader, LocalHandle, LocalSubprocessHandle, OutputCollector, SpawnInternals,
    child_env, default_linux_group_live, host_platform, spawn_subprocess, taskkill_process_tree,
};
pub use terminal::{LocalTerminalHandle, PtyTerminal};
