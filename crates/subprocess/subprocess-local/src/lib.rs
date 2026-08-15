//! Local implementation of the subprocess capability seam: detached
//! process-tree spawn, tail-keep collection with spill files, tree-scoped
//! signalling, and the SIGTERM→SIGKILL escalation. Rust port of
//! `packages/subprocess/subprocess-local` (spawn.ts, index.ts).

pub mod index;
pub mod invariant;
pub mod spawn;

pub use index::LocalSubprocessRuntime;
pub use spawn::{
    CollectorReader, LocalHandle, LocalSubprocessHandle, OutputCollector, SpawnInternals,
    child_env, default_linux_group_live, host_platform, spawn_subprocess, taskkill_process_tree,
};
