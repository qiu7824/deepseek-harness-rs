//! Service Definition for the subprocess capability seam
//! (`ctx.subprocess`): execution-world executable lookup, fully specified
//! managed process trees with raw or collected stdio, and one
//! terminal-process primitive. Rust port of
//! `packages/subprocess/subprocess/src/index.ts`. Command defaulting, shell
//! semantics, deadlines, protocol framing, terminal readiness, and
//! presentation belong to consumers. The local implementation lives in
//! `dsh-subprocess-local`.

use std::sync::Arc;

use cordis::Service;
use futures::future::BoxFuture;

use crate::types::{
    SubprocessAbort, SubprocessHandle, SubprocessSpawnSpec, SubprocessTerminalHandle,
    SubprocessTerminalSpawnSpec,
};

/// Credential-shaped environment names are NOT forwarded to children (the
/// harness's own `DEEPSEEK_API_KEY`/secrets must not leak into a spawned
/// process implicitly). One heuristic for every in-repo spawner; a
/// deliberately supplied entry survives because explicit env layers merge
/// after the scrub.
pub fn sensitive_env_pattern() -> &'static regex::Regex {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new("(?i)KEY|PASSWORD|SECRET|TOKEN").expect("pattern"))
}

/// The ambient parent environment minus credential-shaped names and minus
/// all `DSH_*` names — the canonical base every harness child starts from.
/// `PATH`, `HOME`, locale, and proxy variables survive; harness identity
/// never leaks implicitly. Both scrubs match case-insensitively.
pub fn scrubbed_parent_env() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(key, _value)| {
            !sensitive_env_pattern().is_match(key)
                && !key.to_uppercase().starts_with(crate::types::DSH_ENV_PREFIX)
        })
        .collect()
}

/// Abstract subprocess service (TS `SubprocessRuntime`). Subclass, implement
/// [`SubprocessRuntime::spawn`], and register as `ctx.subprocess`.
///
/// Implementations must honor these semantics:
///
/// - Executable paths belong to one execution world shared with the mounted
///   filesystem provider.
/// - `spawn` returns immediately with a live handle; `done` resolves at
///   process close with exit facts and rejects only for spawn-level
///   failures.
/// - Collect-mode readers are offset-based and non-consuming, so independent
///   readers never consume one another's output; lossy reads report
///   truncation and the spill file holding the complete stream when one
///   exists. Piped streams are handed to the caller raw and never buffered
///   here.
/// - [`SubprocessHandle::terminate`] (and the spec's abort signal)
///   escalates SIGTERM→grace→SIGKILL — the only termination verb —
///   tree-scoped on every platform.
/// - Disposal of the service terminates all still-running managed processes
///   and awaits their exit.
pub trait SubprocessRuntime: Send + Sync + 'static {
    /// Resolve one configured executable in this provider's execution world.
    /// Absolute paths are verified; bare names use the provider's scrubbed
    /// PATH plus explicit environment overrides. Relative paths containing
    /// separators are rejected: the resolution base is undefined, so
    /// providers fail loud instead of guessing.
    fn resolve_executable(
        &self,
        command: &str,
        env: Option<&[(String, String)]>,
        signal: Option<SubprocessAbort>,
    ) -> BoxFuture<'static, Result<String, String>>;

    /// Start one managed child process from a fully-specified spec; this
    /// seam applies no defaults.
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String>;

    /// Allocate a real terminal and start one owned process session. This is
    /// the only non-pipe process primitive: implementations own terminal
    /// byte I/O, foreground groups, signals, and complete session-tree
    /// cleanup.
    fn spawn_terminal(
        &self,
        spec: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>>;
}

impl Service for dyn SubprocessRuntime {
    fn service_name(&self) -> &'static str {
        "subprocess"
    }
}
