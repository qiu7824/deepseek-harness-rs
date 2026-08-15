//! Service Definition for the same-world process-confinement capability
//! seam: wrap exact subprocess argv under a host-path file policy. Rust port
//! of `packages/sandbox/sandbox/src/index.ts`. Containers, microVMs, and
//! remote execution replace the surrounding capability seam instead; this
//! service shares the host kernel and filesystem.

use std::sync::Arc;

use cordis::Service;
use dsh_llm::HarnessError;
use dsh_session::SessionId;

pub use crate::escalation::{
    ESCALATION_TARGETS, WIDER_MODES, approve_escalation, escalation_hint_marker,
    sandbox_denial_marker, validate_escalation_args,
};
pub use crate::roots::{canonical_path, writable_roots};

/// File-effect policy for confined processes (TS `SandboxMode`).
/// `read-only` permits only required sinks such as `/dev/null`;
/// `workspace-write` also permits the workspace and a backend-defined temp
/// area; `danger-full-access` bypasses confinement. Network and process
/// visibility are outside this vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl SandboxMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "read-only",
            SandboxMode::WorkspaceWrite => "workspace-write",
            SandboxMode::DangerFullAccess => "danger-full-access",
        }
    }
}

/// A confining (non-`danger-full-access`) mode — the modes a
/// [`SandboxPolicy`] can carry (TS `ConfinedSandboxMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfinedSandboxMode {
    ReadOnly,
    WorkspaceWrite,
}

impl ConfinedSandboxMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConfinedSandboxMode::ReadOnly => "read-only",
            ConfinedSandboxMode::WorkspaceWrite => "workspace-write",
        }
    }
}

/// The complete file-effect policy resolved for one capability call (TS
/// `SandboxExecutionPolicy`). The root is carried even under modes that do
/// not consume it so callers can resolve policy once before choosing the
/// enforcement path.
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxExecutionPolicy {
    /// The file-effect mode this execution runs under.
    pub mode: SandboxMode,
    /// Absolute root directory `workspace-write` may write under.
    pub workspace_root: String,
    /// Opaque identity of the calling session; backends key per-session
    /// state off it; absent for agentless calls, which fall back to
    /// per-call backend state.
    pub session_id: Option<SessionId>,
}

/// What one confined execution is allowed to touch — carried PER CALL, not
/// fixed on the provider (TS `SandboxPolicy`). The provider treats the
/// policy as fully specified.
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxPolicy {
    /// The file-effect mode this execution runs under.
    pub mode: ConfinedSandboxMode,
    /// Absolute root directory `workspace-write` may write under.
    pub workspace_root: String,
    /// Opaque identity of the calling session.
    pub session_id: Option<SessionId>,
}

/// Enforcement completeness for this host (TS `SandboxEnforcement`).
/// `partial` means an active backend or older kernel ABI cannot govern every
/// promised file effect; callers requiring an absolute boundary must not
/// treat it as `full`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxEnforcement {
    Full,
    Partial,
}

impl SandboxEnforcement {
    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxEnforcement::Full => "full",
            SandboxEnforcement::Partial => "partial",
        }
    }
}

/// Evidence that identifies a sandbox runner failing before it executes the
/// wrapped command (TS `RunnerFailureRule`). A consumer first applies
/// `allowed_exit_codes` when present, removes `informational_lines` by
/// case-insensitive exact line equality, then matches `fatal_signatures`
/// case-insensitively within each remaining stderr line. Exit status alone
/// never proves runner failure.
#[derive(Debug, Clone, PartialEq)]
pub struct RunnerFailureRule {
    /// Nonzero process exit codes on which this rule may match; omitted
    /// permits any nonzero exit.
    pub allowed_exit_codes: Option<Vec<i32>>,
    /// Non-empty substrings identifying a fatal runner diagnostic on one
    /// stderr line.
    pub fatal_signatures: Vec<String>,
    /// Benign stderr lines excluded by exact full-line equality before fatal
    /// matching.
    pub informational_lines: Option<Vec<String>>,
}

/// A [`SandboxProvider::confine`] result: the argv to spawn in place of the
/// caller's own, plus the enforcement completeness the selected backend
/// achieves for it (TS `ConfinedArgv`).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfinedArgv {
    /// The wrapped argv (runner, profile, separator, then the caller's
    /// argv).
    pub argv: Vec<String>,
    /// How completely the selected backend enforces the policy's file
    /// effects.
    pub enforcement: SandboxEnforcement,
    /// The selected backend's denial DIALECT: the case-insensitive stderr
    /// substrings a file effect denied by THIS backend produces.
    pub denial_signatures: Vec<String>,
    /// Structured runner-failure evidence rules.
    pub runner_failure_rules: Vec<RunnerFailureRule>,
}

/// Error code for a requested confined mode when no backend is usable. The
/// provider fails closed, and [`HarnessError`] carries the code through
/// `tool/result` so callers can distinguish missing confinement from command
/// failure.
pub const SANDBOX_UNAVAILABLE: &str = "SANDBOX_UNAVAILABLE";

/// Thrown when [`SandboxProvider::confine`] cannot enforce the requested
/// mode (TS `SandboxUnavailableError`). Carries [`SANDBOX_UNAVAILABLE`]
/// through the structured error channel.
pub struct SandboxUnavailableError {
    pub error: HarnessError,
}

impl SandboxUnavailableError {
    pub fn new(mode: ConfinedSandboxMode, detail: Option<&str>) -> Self {
        let message = format!(
            "sandbox mode \"{}\" is requested but no sandbox backend is usable on this host; \
             refusing to run the command unconfined. Install bubblewrap or run a Landlock-enforcing \
             kernel (Linux), ensure sandbox-exec is usable (macOS), or ensure the ACL \
             restricted-token runner can start (Windows) — otherwise switch the consumer to \
             danger-full-access.{}",
            mode.as_str(),
            match detail {
                Some(detail) => format!(" Runner failure: {detail}"),
                None => String::new(),
            }
        );
        Self { error: HarnessError::new(message, SANDBOX_UNAVAILABLE) }
    }

    /// The stable machine-routable failure class.
    pub fn code(&self) -> &str {
        &self.error.code
    }
}

impl std::fmt::Debug for SandboxUnavailableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxUnavailableError")
            .field("code", &self.error.code)
            .field("message", &self.error.message)
            .finish()
    }
}

impl std::fmt::Display for SandboxUnavailableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error.message)
    }
}

impl std::error::Error for SandboxUnavailableError {}

/// Abstract process-sandbox service (TS `SandboxProvider`). `confine` must
/// return enforcing argv or fail closed at wrap or runner-execution time;
/// silent unconfined passthrough is forbidden. Functional probes arbitrate
/// multi-runner chains and may be skipped for a sole candidate, whose own
/// refusal remains the fail-closed end.
pub trait SandboxProvider: Send + Sync + 'static {
    /// Wrap `argv` so it executes confined under `policy` on this host; the
    /// caller spawns the returned argv in place of its own.
    ///
    /// `argv` is the exact argv the caller is about to spawn (program plus
    /// arguments), NOT a shell string — a shell-shaped consumer passes
    /// `['bash', '-c', command]`.
    fn confine(&self, argv: &[String], policy: &SandboxPolicy) -> Result<ConfinedArgv, SandboxUnavailableError>;
}

impl Service for dyn SandboxProvider {
    fn service_name(&self) -> &'static str {
        "sandbox"
    }
}

/// A borrowed boxed provider handle (convenience alias for consumers that
/// resolve `ctx.sandbox` once).
pub type SandboxProviderRef = Arc<dyn SandboxProvider>;
