//! The sandbox POLICY home (`ctx.sandboxPolicy`): the single owner of the
//! deployment's sandbox fallbacks plus per-session resolution. Rust port of
//! `packages/sandbox/sandbox-policy/src/index.ts`.
//!
//! # Deviations
//!
//! - The TS `systemPrompt.context` contribution (the `sandbox:policy`
//!   request context) is deferred: the Rust system-prompt assembly does not
//!   yet expose the agent/session field the provider narrows; the policy
//!   resolution itself is complete.

use std::sync::Arc;

use cordis::{Context, Service};
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode, canonical_path};
use dsh_session::Session;

use crate::session_mode::effective_sandbox_mode;

/// Plugin config: the deployment's sandbox default. All optional —
/// `mode: read-only` is the fail-safe default; a deployment that wants a
/// workspace-writable agent opts in explicitly.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// File-sandbox mode a session starts from (default: `read-only`).
    pub mode: Option<SandboxMode>,
    /// Fallback root for agentless calls and sessions without a cwd
    /// (default: the process cwd). Normal agent calls use their session cwd
    /// instead.
    pub workspace_root: Option<String>,
}

/// Inputs that select the sandbox policy for one capability call (TS
/// `SandboxPolicyRequest`).
#[derive(Debug, Clone, Default)]
pub struct SandboxPolicyRequest {
    /// Calling session; its immutable cwd becomes the workspace boundary.
    pub session: Option<Arc<Session>>,
    /// Explicit approved mode override, which outranks session policy.
    pub mode: Option<SandboxMode>,
}

/// Resolve filesystem identity before lexical normalization can erase
/// symlink-sensitive components (TS `resolveWorkspaceRoot`).
fn resolve_workspace_root(path: &str) -> String {
    let canonical = canonical_path(path);
    std::path::absolute(&canonical)
        .unwrap_or_else(|_| std::path::PathBuf::from(canonical))
        .to_string_lossy()
        .into_owned()
}

/// The sandbox-policy service (`ctx.sandboxPolicy`). Owns the deployment
/// default mode, fallback workspace root, and current request-time policy
/// section.
pub struct SandboxPolicyService {
    ctx: Context,
    /// The deployment default mode — the fallback beneath a session
    /// override.
    pub default_mode: SandboxMode,
    /// The absolute `workspace-write` fallback root for calls without a
    /// session cwd.
    pub workspace_root: String,
}

impl SandboxPolicyService {
    /// Construct the service, resolve the fallback root absolute, and
    /// register as `ctx.sandboxPolicy`.
    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let default_mode = config.mode.unwrap_or(SandboxMode::ReadOnly);
        let fallback = config
            .workspace_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().map(|cwd| cwd.to_string_lossy().into_owned()).unwrap_or_else(|_| ".".to_string()));
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            default_mode,
            workspace_root: resolve_workspace_root(&fallback),
        });
        ctx.register_service(service.clone());
        service
    }

    /// Resolve the complete policy for one capability call. An approved
    /// explicit mode outranks the session's last `sandbox/mode` event, which
    /// outranks the deployment default. A session cwd is its
    /// workspace-write boundary; the configured root is the fallback for
    /// agentless calls and sessions without a cwd.
    pub fn resolve(&self, request: &SandboxPolicyRequest) -> SandboxExecutionPolicy {
        let session = request.session.as_deref();
        let mode = request
            .mode
            .or_else(|| session.and_then(|session| self.override_of(session)))
            .unwrap_or(self.default_mode);
        let workspace_root = resolve_workspace_root(
            session
                .and_then(|session| session.header().cwd.as_deref())
                .unwrap_or(&self.workspace_root),
        );
        SandboxExecutionPolicy {
            mode,
            workspace_root,
            session_id: session.map(|session| session.header().id.clone()),
        }
    }

    /// Read the session override without applying the deployment default.
    pub fn override_of(&self, session: &Session) -> Option<SandboxMode> {
        effective_sandbox_mode(&session.events())
    }

    /// The service's context.
    pub fn ctx(&self) -> &Context {
        &self.ctx
    }
}

impl Service for SandboxPolicyService {
    fn service_name(&self) -> &'static str {
        "sandboxPolicy"
    }
}
