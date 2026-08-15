//! Vocabulary for the subprocess Service Definition: fully-specified spawn
//! requests with per-stream stdio modes, bounded collected output with spill
//! recovery, raw piped streams, and tree-scoped termination. Rust port of
//! `packages/subprocess/subprocess/src/types.ts`. Command defaulting, shell
//! semantics, protocol framing, and presentation belong to consumers such as
//! the bash executor seam.
//!
//! # Deviations
//!
//! - `AbortSignal` collapses into the seam-wide cancellation predicate
//!   ([`SubprocessAbort`]).
//! - Node streams collapse into tokio byte streams
//!   (`BoxStream<'static, Vec<u8>>` for output; boxed `AsyncRead`/`AsyncWrite`
//!   for piped stdio).
//! - The explicit-env tombstones (`undefined` removes an ambient entry) are
//!   `(String, Option<String>)` pairs.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use tokio::io::{AsyncRead, AsyncWrite};

/// Namespace prefix reserved for DeepSeek Harness-managed child environment
/// facts.
pub const DSH_ENV_PREFIX: &str = "DSH_";

/// One environment key inside the managed [`DSH_ENV_PREFIX`] namespace (TS
/// `DshEnvironmentKey`, the `` `${DSH_ENV_PREFIX}${string}` `` template
/// literal). The `DSH_` prefix requirement is a type-level fact in TS; the
/// Rust alias documents it (callers of the seam validate at merge time).
pub type DshEnvironmentKey = String;

/// A managed `DSH_*` snapshot merged last onto the child environment (TS
/// `DshEnvironment`). Kept as an ordered pair list so later entries override
/// earlier ones without a map's ordering loss.
pub type DshEnvironment = Vec<(DshEnvironmentKey, String)>;

/// The abort/cancellation predicate (the TS `AbortSignal` collapse).
pub type SubprocessAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// One captured stream: the (possibly truncated) text plus recovery info
/// (TS `CollectedOutput`).
#[derive(Debug, Clone, PartialEq)]
pub struct CollectedOutput {
    /// Collected text — the TAIL of the stream when truncated.
    pub text: String,
    /// True when bytes were dropped from `text`.
    pub truncated: bool,
    /// Path to a file holding the COMPLETE stream, when truncated and
    /// available.
    pub spill_path: Option<String>,
}

/// stdin disposition (TS `SubprocessStdinMode`). `Ignore` leaves fd 0 on
/// `/dev/null`; `Pipe` exposes the handle's stdin for the caller's ongoing
/// protocol writes; `Data` writes the bytes and closes (the batch shape).
#[derive(Debug, Clone, PartialEq)]
pub enum SubprocessStdinMode {
    Ignore,
    Pipe,
    Data(String),
}

/// Full-stream spill configuration (the TS inline `spill` shape).
#[derive(Debug, Clone, PartialEq)]
pub struct SubprocessSpill {
    /// Whole-stream byte cap; a larger stream discards its now-incomplete
    /// spill.
    pub max_bytes: u64,
}

/// Bounded in-memory collection for one output stream, with an optional
/// full-stream spill file (TS `SubprocessCollect`).
#[derive(Debug, Clone, PartialEq)]
pub struct SubprocessCollect {
    /// In-memory cap in bytes; overflow keeps the TAIL.
    pub max_bytes: u64,
    /// Full-stream spill file; absent disables spilling entirely.
    pub spill: Option<SubprocessSpill>,
}

/// stdout/stderr disposition (TS `SubprocessOutputMode`).
#[derive(Debug, Clone, PartialEq)]
pub enum SubprocessOutputMode {
    Pipe,
    Inherit,
    Collect(SubprocessCollect),
}

/// Per-stream stdio dispositions, all explicit — this seam applies no
/// defaults (TS `SubprocessStdio`).
#[derive(Debug, Clone, PartialEq)]
pub struct SubprocessStdio {
    pub stdin: SubprocessStdinMode,
    pub stdout: SubprocessOutputMode,
    pub stderr: SubprocessOutputMode,
}

/// A fully-specified spawn request (TS `SubprocessSpawnSpec`). This seam
/// applies no defaults: every disposition, limit, and directory is explicit.
#[derive(Clone)]
pub struct SubprocessSpawnSpec {
    /// Executable and arguments; `argv[0]` is the program. Never
    /// shell-interpreted here.
    pub argv: Vec<String>,
    /// Working directory for the child.
    pub cwd: String,
    /// Per-stream stdio dispositions.
    pub stdio: SubprocessStdio,
    /// Positive finite grace period in milliseconds for the terminate
    /// escalation and for draining still-open collected pipes after the
    /// process exits.
    pub grace_ms: u64,
    /// Abort signal — starts the terminate escalation on the process tree
    /// when it fires. The caller owns deadlines and cause classification.
    pub signal: Option<SubprocessAbort>,
    /// Explicit environment entries merged onto the implementation's
    /// scrubbed parent base. `None` in a pair is a tombstone that removes an
    /// ordinary ambient entry from the child.
    pub env: Option<Vec<(String, Option<String>)>>,
}

/// Exit facts of one closed process (TS `SubprocessOutcome`). Deliberately
/// carries NO timeout/cancellation classification and NO output.
#[derive(Debug, Clone, PartialEq)]
pub struct SubprocessOutcome {
    /// Exit code; `None` when the process died from a signal.
    pub exit_code: Option<i32>,
    /// Terminating signal (e.g. `SIGTERM`); `None` on normal exit.
    pub signal: Option<String>,
}

/// One incremental [`SubprocessOutputReader::read_from`] read (TS
/// `SubprocessOutputRead`).
#[derive(Debug, Clone, PartialEq)]
pub struct SubprocessOutputRead {
    /// Stream text from the requested offset (the whole retained tail when
    /// lossy).
    pub text: String,
    /// Whole-stream offset to resume from on the next read.
    pub next_offset: u64,
    /// True when the requested offset slid out of the in-memory tail
    /// window.
    pub lossy: bool,
    /// Path to the full-stream spill file, when one was created and remains
    /// intact.
    pub spill_path: Option<String>,
}

/// Cursor-free incremental access to one collected output stream (TS
/// `SubprocessOutputReader`). Offsets are whole-stream byte coordinates
/// owned by the caller, so independent readers cannot consume one another's
/// output.
pub trait SubprocessOutputReader: Send + Sync {
    /// Read everything captured since `from_byte`. When that offset has slid
    /// out of the in-memory tail window the read is `lossy` — it returns the
    /// whole retained tail and the gap is only recoverable from the spill
    /// file.
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead;
}

/// Offset-based readers for the streams spawned in collect mode (TS
/// `SubprocessCollectedOutputs`).
#[derive(Clone, Default)]
pub struct SubprocessCollectedOutputs {
    /// Present iff stdout is a [`SubprocessCollect`].
    pub stdout: Option<Arc<dyn SubprocessOutputReader>>,
    /// Present iff stderr is a [`SubprocessCollect`].
    pub stderr: Option<Arc<dyn SubprocessOutputReader>>,
}

/// A live child process rooted in its own process tree (TS
/// `SubprocessHandle`). Collected output remains readable after exit; piped
/// streams belong to the caller. Termination is tree-scoped everywhere.
pub trait SubprocessHandle: Send + Sync {
    /// Process id (tree root); -1 when the spawn itself failed.
    fn pid(&self) -> i32;
    /// The child's stdin, present iff spawned with `stdin: 'pipe'`.
    fn stdin(&self) -> Option<Box<dyn AsyncWrite + Unpin + Send>>;
    /// The child's raw stdout, present iff spawned with `stdout: 'pipe'`.
    fn stdout(&self) -> Option<Box<dyn AsyncRead + Unpin + Send>>;
    /// The child's raw stderr, present iff spawned with `stderr: 'pipe'`.
    fn stderr(&self) -> Option<Box<dyn AsyncRead + Unpin + Send>>;
    /// Offset-based readers for collect-mode streams (also readable after
    /// exit).
    fn collected(&self) -> SubprocessCollectedOutputs;
    /// Resolves at process close with exit facts; rejects only for
    /// spawn-level failures.
    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>>;
    /// Begin the SIGTERM → `graceMs` → SIGKILL escalation on the process
    /// tree — the seam's only termination verb. Idempotent, a no-op once the
    /// tree is gone, and also triggered by the spec's abort signal.
    fn terminate(&self);
    /// Wait until the process tree has exited — the tree, not just the
    /// direct child.
    fn wait_for_exit(&self, signal: Option<SubprocessAbort>) -> BoxFuture<'static, bool>;
}

/// Signals supported by the terminal-process primitive (TS
/// `SubprocessTerminalSignal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubprocessTerminalSignal {
    SigInt,
    SigTerm,
    SigKill,
    SigTstp,
    SigHup,
}

impl SubprocessTerminalSignal {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubprocessTerminalSignal::SigInt => "SIGINT",
            SubprocessTerminalSignal::SigTerm => "SIGTERM",
            SubprocessTerminalSignal::SigKill => "SIGKILL",
            SubprocessTerminalSignal::SigTstp => "SIGTSTP",
            SubprocessTerminalSignal::SigHup => "SIGHUP",
        }
    }
}

/// A fully specified terminal-process spawn (TS
/// `SubprocessTerminalSpawnSpec`).
#[derive(Clone)]
pub struct SubprocessTerminalSpawnSpec {
    /// Executable and arguments; `argv[0]` is the program.
    pub argv: Vec<String>,
    /// Working directory in this subprocess provider's execution world.
    pub cwd: String,
    /// Explicit environment layered after the provider's ambient scrub.
    pub env: Option<Vec<(String, String)>>,
    /// Initial terminal row count.
    pub rows: u16,
    /// Initial terminal column count.
    pub cols: u16,
    /// TERM-to-KILL cleanup grace for the complete terminal session.
    pub grace_ms: u64,
    /// Cancellation of terminal allocation; a published handle owns its
    /// later lifetime.
    pub signal: Option<SubprocessAbort>,
}

/// Current foreground process-group facts for one terminal (TS
/// `SubprocessTerminalForeground`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubprocessTerminalForeground {
    /// Foreground process-group id published by the terminal driver.
    pub process_group_id: u32,
    /// Whether the provider can currently prove that group is waiting on
    /// terminal input.
    pub input_waiting: bool,
}

/// One live terminal process and its owned OS session (TS
/// `SubprocessTerminalHandle`).
pub trait SubprocessTerminalHandle: Send + Sync {
    /// Top-level terminal process id.
    fn pid(&self) -> u32;
    /// UTF-8 terminal output bytes in delivery order; ends after queued
    /// output when the terminal exits.
    fn output(&self) -> BoxStream<'static, Vec<u8>>;
    /// Resolves when the top-level process exits; rejects only for a live
    /// transport failure.
    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>>;
    /// Write text to the terminal input.
    fn write(&self, data: &str) -> BoxFuture<'static, Result<(), String>>;
    /// Inspect the current foreground process group.
    fn inspect_foreground(
        &self,
    ) -> BoxFuture<'static, Result<Option<SubprocessTerminalForeground>, String>>;
    /// Deliver a signal to the current foreground process group.
    fn signal_foreground(
        &self,
        signal: SubprocessTerminalSignal,
    ) -> BoxFuture<'static, Result<u32, String>>;
    /// Idempotently terminate every terminal-session member the provider can
    /// still observe and await quiescence.
    fn terminate(&self) -> BoxFuture<'static, Result<(), String>>;
}
