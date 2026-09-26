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
    // Execution-world URIs are resolved and enforced by their remote provider.
    // Never make a local filesystem path out of a remote workspace identity.
    if path.starts_with("dsh-remote://") {
        return path.into();
    }
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
        let fallback = config.workspace_root.clone().unwrap_or_else(|| {
            std::env::current_dir()
                .map(|cwd| cwd.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_string())
        });
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
        self.try_resolve(request).unwrap_or_else(|error| {
            eprintln!("sandbox policy history unavailable; enforcing read-only access: {error}");
            SandboxExecutionPolicy {
                read_only_roots: Vec::new(),
                mode: SandboxMode::ReadOnly,
                workspace_root: resolve_workspace_root(
                    request
                        .session
                        .as_ref()
                        .and_then(|session| session.header().cwd.as_deref())
                        .unwrap_or(&self.workspace_root),
                ),
                session_id: request.session.as_ref().map(|session| session.id().clone()),
            }
        })
    }

    /// Resolve durable policy and attachment roots with explicit read failures.
    pub fn try_resolve(
        &self,
        request: &SandboxPolicyRequest,
    ) -> Result<SandboxExecutionPolicy, String> {
        let session = request.session.as_deref();
        let mut mode_override = None;
        let mut paths = std::collections::BTreeSet::new();
        let store = self
            .ctx
            .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false);
        if let Some(session) = session {
            session.visit_events(0, None, |event| {
                if event.type_ == "sandbox/mode" {
                    mode_override = effective_sandbox_mode(std::slice::from_ref(event));
                }
                if let Some(store) = &store {
                    for reference in
                        dsh_attachment::file_references_for_event(&event.type_, &event.data)
                    {
                        if let Some(path) = store.file_host_path(&reference) {
                            paths.insert(path.to_string_lossy().into_owned());
                        }
                    }
                }
                Ok(true)
            })?;
        }
        let mode = request.mode.or(mode_override).unwrap_or(self.default_mode);
        let workspace_root = resolve_workspace_root(
            session
                .and_then(|session| session.header().cwd.as_deref())
                .unwrap_or(&self.workspace_root),
        );
        Ok(SandboxExecutionPolicy {
            read_only_roots: paths.into_iter().collect(),
            mode,
            workspace_root,
            session_id: session.map(|session| session.header().id.clone()),
        })
    }

    /// Read the session override without applying the deployment default.
    pub fn override_of(&self, session: &Session) -> Option<SandboxMode> {
        self.try_override_of(session).unwrap_or_else(|error| {
            eprintln!("sandbox mode history unavailable; enforcing read-only access: {error}");
            Some(SandboxMode::ReadOnly)
        })
    }

    /// Read the override without granting the configured default on I/O failure.
    pub fn try_override_of(&self, session: &Session) -> Result<Option<SandboxMode>, String> {
        let mut mode = None;
        session.visit_events(0, None, |event| {
            if event.type_ == "sandbox/mode" {
                mode = effective_sandbox_mode(std::slice::from_ref(event));
            }
            Ok(true)
        })?;
        Ok(mode)
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

#[cfg(all(test, windows))]
mod archive_tests {
    use super::*;

    #[test]
    fn archived_policy_preserves_overrides_and_backing_refuses_external_handles() {
        let ctx = Context::root();
        let service = SandboxPolicyService::install(
            &ctx,
            Config {
                mode: Some(SandboxMode::DangerFullAccess),
                ..Default::default()
            },
        );
        let session =
            Session::create(dsh_session::session_id("sandbox-archive"), None, None, None).unwrap();
        crate::set_sandbox_mode(&session, SandboxMode::WorkspaceWrite).unwrap();
        crate::set_sandbox_mode(&session, SandboxMode::ReadOnly).unwrap();
        let directory = std::env::temp_dir().join(format!(
            "sandbox-policy-archive-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut builder = dsh_session::event_archive::EventArchiveBuilder::new(&directory).unwrap();
        session
            .visit_events(0, None, |event| {
                builder.push(event)?;
                Ok(true)
            })
            .unwrap();
        let archived = Arc::new(
            Session::from_event_archive(
                session.id().clone(),
                builder.finish().unwrap(),
                session.header(),
                session.inherited_event_count(),
                vec![],
            )
            .unwrap(),
        );
        let request = SandboxPolicyRequest {
            session: Some(archived.clone()),
            mode: None,
        };
        assert_eq!(
            service.try_resolve(&request).unwrap().mode,
            SandboxMode::ReadOnly
        );
        let approved = SandboxPolicyRequest {
            mode: Some(SandboxMode::WorkspaceWrite),
            ..request.clone()
        };
        assert_eq!(
            service.try_resolve(&approved).unwrap().mode,
            SandboxMode::WorkspaceWrite
        );
        let path = std::fs::read_dir(&directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(std::fs::OpenOptions::new().read(true).open(&path).is_err());
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert_eq!(
            service.try_resolve(&approved).unwrap().mode,
            SandboxMode::WorkspaceWrite
        );
        assert_eq!(service.resolve(&request).mode, SandboxMode::ReadOnly);
        assert_eq!(service.override_of(&archived), Some(SandboxMode::ReadOnly));
        drop(approved);
        drop(request);
        drop(archived);
        std::fs::remove_dir(directory).unwrap();
    }
}
