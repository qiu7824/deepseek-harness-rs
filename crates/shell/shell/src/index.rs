//! Service Definition for the `ctx.shell` capability seam, covering
//! foreground commands and background process handles. Job ids, ownership,
//! polling, and notices belong to `dsh-jobs`, keeping executors independent
//! of sessions. Rust port of `packages/shell/shell/src/index.ts`.

use std::sync::{Arc, OnceLock};

use cordis::Service;
use dsh_sandbox::SandboxMode;
use dsh_settings::{SettingsNamespace, settings_namespace};
use futures::future::BoxFuture;

use crate::types::{ShellExecRequest, ShellExecSpec, ShellProcess, ShellRunResult};

pub use crate::render::{ParsedExitStatus, parse_exit_status};
pub use crate::types::{DSH_ENV_PREFIX, DshEnvironment, DshEnvironmentKey};
pub use dsh_subprocess::CollectedOutput;

/// Settings namespace of this capability, owned here rather than by either
/// executor family because it names the capability, not an implementation
/// (TS `SHELL_SETTINGS_NAMESPACE`).
pub fn shell_settings_namespace() -> &'static SettingsNamespace {
    static NAMESPACE: OnceLock<SettingsNamespace> = OnceLock::new();
    NAMESPACE.get_or_init(|| settings_namespace("shell").expect("valid shell namespace"))
}

/// Abstract bash execution service (TS `ShellExecutor`). Subclass, implement
/// the abstract methods, and register the subclass as `ctx.shell` (one
/// implementation per context; registering a second panics, which is cordis'
/// standard duplicate-service behavior).
///
/// Implementations must honor these semantics:
/// - `run` returns `Err` only for infrastructure failures. Nonzero exits,
///   timeout kills, and abort kills resolve with a
///   [`ShellRunResult`].
/// - `start` returns immediately; no timeout applies to background
///   processes. `done` settles at process close and never rejects; spawn
///   failures settle as `killed` with the error on stderr.
/// - [`ShellProcess::read_output`] is incremental: consecutive reads never
///   repeat output. Lossy reads report truncation and available spill files.
/// - A still-running background process is stopped and awaited when its
///   owning composition tears down. With the subprocess seam that boundary
///   is `ctx.subprocess` disposal, so a background process survives an
///   executor-only reload.
pub trait ShellExecutor: Send + Sync + 'static {
    /// The sandbox mode this executor applies by default, or `None` when it
    /// does not sandbox commands.
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        None
    }

    /// Apply implementation-owned defaults and caps to a request before
    /// execution.
    fn resolve(&self, request: ShellExecRequest) -> ShellExecSpec;

    /// Run a command in the foreground; resolves when it finishes.
    fn run(&self, spec: ShellExecSpec) -> BoxFuture<'static, Result<ShellRunResult, String>>;

    /// Start a background process and return its handle immediately.
    fn start(&self, spec: ShellExecSpec) -> Arc<dyn ShellProcess>;
}

impl Service for dyn ShellExecutor {
    fn service_name(&self) -> &'static str {
        "shell"
    }
}
