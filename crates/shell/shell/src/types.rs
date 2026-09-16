//! Execution types for the bash executor seam. Background job semantics
//! belong to `dsh-jobs`; this seam exposes only process handles. The
//! managed-environment and captured-output vocabulary is owned by the
//! subprocess seam and re-exported here so bash consumers keep one import
//! root. Rust port of `packages/shell/shell/src/types.ts`.
//!
//! # Deviations
//!
//! - `AbortSignal` collapses into the repo-wide cancellation predicate
//!   ([`ShellAbort`]).
//! - `DshEnvironment` is the ordered `Vec<(DshEnvironmentKey, String)>`
//!   shape (the TS `Readonly<Record<...>>` has no order guarantee worth
//!   keeping; last-write-wins merge stays explicit).

use std::sync::Arc;

use dsh_sandbox::{SandboxEnforcement, SandboxExecutionPolicy, SandboxMode};
pub use dsh_subprocess::{CollectedOutput, DSH_ENV_PREFIX, DshEnvironment, DshEnvironmentKey};

/// The abort/cancellation predicate (the TS `AbortSignal` collapse).
pub type ShellAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// Sandbox facts for one run, present iff a sandboxing executor handled it.
/// Facts are reported independently of process exit status so callers can
/// distinguish command failures from policy denials and runner failures (TS
/// `ShellSandboxInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShellSandboxInfo {
    /// The mode the command actually ran under.
    pub mode: SandboxMode,
    /// Whether the sandbox denied a file operation.
    pub denied: bool,
    /// How completely the selected runner enforced the requested mode.
    pub enforcement: Option<SandboxEnforcement>,
    /// Whether runner setup or teardown failed; inspect its diagnostic before
    /// retrying because teardown failures can follow an executed command.
    pub runner_failed: Option<bool>,
}

impl ShellSandboxInfo {
    /// Recognize a possible COM error in application text. This is a diagnostic
    /// clue, never confirmation of a desktop, registry or file denial.
    pub fn com_access_denied(stderr: &str) -> bool {
        let text = stderr.to_ascii_lowercase();
        (text.contains("80070005") || text.contains("e_accessdenied"))
            && (text.contains("clsid")
                || text.contains("class factory")
                || text.contains("comobject"))
    }
    pub fn observe(
        mode: SandboxMode,
        confined: &dsh_sandbox::ConfinedArgv,
        _exit_code: Option<i32>,
        _stderr: &str,
    ) -> Self {
        // stderr is an untrusted application stream. Even a runner prefix plus
        // its usual exit code can be printed by the program being executed.
        // Confirmed setup failures are returned by prepare/readiness, never
        // inferred from this stream. Keep the enforcement fact independently.
        Self {
            mode,
            denied: false,
            enforcement: Some(confined.enforcement),
            runner_failed: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ShellSandboxInfo;
    #[test]
    fn com_activation_is_not_classified_as_file_sandbox_denial() {
        let stderr = "CLSID ... 80070005 E_ACCESSDENIED ... COMObject";
        assert!(ShellSandboxInfo::com_access_denied(stderr));
    }
    #[test]
    fn ordinary_access_denial_does_not_imply_a_com_error() {
        assert!(!ShellSandboxInfo::com_access_denied(
            "Access is denied reading file"
        ));
    }
}

/// A caller's execution REQUEST: `workdir` and `timeout_ms` are optional and
/// filled by the executor's `resolve` from the implementation's config. This
/// is the model-/plugin-facing shape (TS `ShellExecRequest`). Carries a
/// cancellation predicate, so no `Debug`.
#[derive(Clone)]
pub struct ShellExecRequest {
    pub command: String,
    /// Native argv bypasses shell parsing. The first element is the executable.
    pub native_argv: Option<Vec<String>>,
    /// Immutable selected executable for this shell adapter (never a language switch).
    pub shell_path: Option<String>,
    pub execution_context_id: Option<String>,
    /// Working directory override (default: implementation-configured).
    pub workdir: Option<String>,
    /// Timeout override in milliseconds (implementations cap it).
    pub timeout_ms: Option<u64>,
    /// Foreground stdout capture budget in bytes. Absent uses the executor's
    /// default output cap.
    pub stdout_max_bytes: Option<u64>,
    /// Abort signal — implementations kill the command when it fires.
    pub signal: Option<ShellAbort>,
    /// Bytes to write to the command's stdin, then close it.
    pub stdin: Option<String>,
    /// Ordinary environment entries, merged after the credential scrub.
    pub env: Option<Vec<(String, String)>>,
    /// Harness-owned `DSH_*` variables for this execution.
    pub dsh_env: Option<DshEnvironment>,
    /// Fully resolved per-call sandbox policy; sandboxing executors default
    /// it.
    pub sandbox_policy: Option<SandboxExecutionPolicy>,
}

impl ShellExecRequest {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            native_argv: None,
            shell_path: None,
            execution_context_id: None,
            workdir: None,
            timeout_ms: None,
            stdout_max_bytes: None,
            signal: None,
            stdin: None,
            env: None,
            dsh_env: None,
            sandbox_policy: None,
        }
    }
}

/// A resolved execution spec. The executor's `resolve` fills and caps the
/// required fields; `start` ignores `timeout_ms` because background processes
/// have no executor timeout (TS `ShellExecSpec`). Carries a cancellation
/// predicate, so no `Debug`.
#[derive(Clone)]
pub struct ShellExecSpec {
    pub command: String,
    pub native_argv: Option<Vec<String>>,
    pub shell_path: Option<String>,
    pub execution_context_id: Option<String>,
    pub workdir: String,
    pub timeout_ms: u64,
    /// Resolved foreground stdout capture budget in bytes. `run()` uses it
    /// for stdout; background jobs and stderr keep the executor's own output
    /// cap.
    pub stdout_max_bytes: u64,
    /// Abort signal — implementations kill the command when it fires.
    pub signal: Option<ShellAbort>,
    /// Bytes to write to stdin before closing it; absent means no stdin.
    pub stdin: Option<String>,
    /// Ordinary environment entries carried through from the request;
    /// `dsh_env` still merges after them.
    pub env: Option<Vec<(String, String)>>,
    /// Managed `DSH_*` snapshot; merges after `env`.
    pub dsh_env: Option<DshEnvironment>,
    /// Resolved sandbox policy; ignored by executors that do not confine.
    pub sandbox_policy: Option<SandboxExecutionPolicy>,
}

/// The outcome of one completed (or killed) foreground run (TS
/// `ShellRunResult`).
#[derive(Debug, Clone)]
pub struct ShellRunResult {
    pub execution_context_id: Option<String>,
    pub executable: String,
    pub stdout_total_bytes: u64,
    pub stderr_total_bytes: u64,
    /// Exit code; `None` when the process died from a signal.
    pub exit_code: Option<i32>,
    /// Terminating signal (e.g. `SIGTERM`); `None` on normal exit.
    pub signal: Option<String>,
    /// True when the executor's own timeout was the FIRST cause to cut the
    /// command short. Mutually exclusive with `aborted`.
    pub timed_out: bool,
    /// True when the caller's abort signal was the FIRST cause to kill the
    /// command (and it was not the executor's own timeout).
    pub aborted: bool,
    /// The effective timeout applied to this run (after defaulting/capping).
    pub timeout_ms: u64,
    pub stdout: CollectedOutput,
    pub stderr: CollectedOutput,
    /// Sandbox execution facts, absent for an unsandboxed executor.
    pub sandbox: Option<ShellSandboxInfo>,
}

/// Lifecycle of a background process (TS `ShellProcessStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellProcessStatus {
    Running,
    Completed,
    Killed,
}

impl ShellProcessStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ShellProcessStatus::Running => "running",
            ShellProcessStatus::Completed => "completed",
            ShellProcessStatus::Killed => "killed",
        }
    }
}

/// One incremental [`ShellProcess::read_output`] read (TS
/// `ShellProcessRead`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellProcessRead {
    /// Output produced since the previous read (stderr in a marked section).
    pub delta: String,
    /// True when truncation dropped unread bytes the delta cannot include.
    pub lossy: bool,
    /// Full stdout spill file, when stdout truncation occurred and a safe
    /// path is available.
    pub stdout_spill_path: Option<String>,
    /// Full stderr spill file, when stderr truncation occurred and a safe
    /// path is available.
    pub stderr_spill_path: Option<String>,
}

/// A background process handle returned by the executor's `start`. It is the
/// only access path; buffered output remains readable after exit (TS
/// `ShellProcess`).
pub trait ShellProcess: Send + Sync + 'static {
    /// Process lifecycle state (settled exactly once).
    fn status(&self) -> ShellProcessStatus;
    /// Exit code once finished (None = killed by signal / still running).
    fn exit_code(&self) -> Option<i32>;
    /// Terminating signal name, when signal-killed.
    fn signal(&self) -> Option<String>;
    /// Resolves when the underlying process closes (never rejects — a spawn
    /// failure settles as `killed` with the error on stderr).
    fn done(&self) -> futures::future::BoxFuture<'static, ()>;
    /// Sandbox facts, stamped once a confined process settles.
    fn sandbox(&self) -> Option<ShellSandboxInfo>;
    /// Read output produced since the previous read (consuming — consecutive
    /// reads never re-deliver). Lossy reads flag `lossy` and point at
    /// full-stream spill files when available.
    fn read_output(&self) -> ShellProcessRead;
    /// Kill the process group. Returns false when it had already finished
    /// (no-op); idempotent.
    fn kill(&self) -> bool;
}
