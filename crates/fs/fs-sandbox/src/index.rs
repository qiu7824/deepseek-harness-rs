//! `SandboxedFileSystem`: the sandbox-enforcing implementation of the
//! `dsh-fs` Service Definition. Rust port of
//! `packages/fs/fs-sandbox/src/index.ts`. It delegates every storage
//! mechanic to `dsh-fs-local` verbatim; this package adds only the per-call
//! POLICY fence on the two mutations. Reads pass through untouched: every
//! mode permits reading.
//!
//! The fence is a policy check in TRUSTED code over a MODEL-CONTROLLED path,
//! NOT a kernel boundary — kernel-grade isolation of untrusted CODE stays
//! the shell's job.
//!
//! Per-call policy: `read-only` denies every mutation; `workspace-write`
//! allows a mutation only when the target canonicalizes under the policy's
//! workspace root or a platform temp area (the SAME writable-root set
//! Seatbelt grants, derived from the one `writableRoots` function so bash
//! and fs cannot drift); `danger-full-access` delegates unfenced. A denial
//! throws the structured `FS_SANDBOX_DENIED`.

use std::sync::Arc;

use cordis::Context;

use dsh_fs::{
    AbortPredicate, FsEditOutcome, FsEditRequest, FsEditGuard, FsError, FsErrorCode, FsTarget,
    FsWriteIntent, FsWriteOutcome, FileSystem, ResolveOptions,
};
use dsh_fs_local::{Config as LocalConfig, LocalFileSystem};
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode, writable_roots};
use dsh_sandbox_policy::SandboxPolicyService;

use crate::containment::is_path_under;

/// Plugin config: the local backend's knobs verbatim (TS `Config`).
pub type Config = LocalConfig;

/// Sandbox-enforcing filesystem backend. Registers as `ctx.fs` (loading it
/// INSTEAD OF `dsh-fs-local`, together with a `ctx.sandboxPolicy`, is the
/// whole swap — the model-facing tools are untouched).
pub struct SandboxedFileSystem {
    local: Arc<LocalFileSystem>,
    /// The deployment default mode — the capability fact the tool layer
    /// reads to advertise escalation.
    default_mode: SandboxMode,
    policy: Arc<SandboxPolicyService>,
}

impl SandboxedFileSystem {
    /// Construct the backend over the local implementation WITHOUT
    /// registering a service (the TS `super(ctx, config)` half).
    pub fn build(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let policy = ctx
            .get_typed::<Arc<SandboxPolicyService>>("sandboxPolicy", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "dsh-fs-sandbox requires the sandboxPolicy service".to_string())?;
        let local = LocalFileSystem::build(config)?;
        Ok(Arc::new(Self {
            local,
            default_mode: policy.default_mode,
            policy,
        }))
    }

    /// Construct the backend over the local implementation and register as
    /// `ctx.fs` (the TS constructor + `super(ctx, config)` collapse). The
    /// `sandboxPolicy` service must be installed first (the TS
    /// `static inject = ['sandboxPolicy']`).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let backend = Self::build(ctx, config)?;
        let erased: Arc<dyn FileSystem> = backend.clone();
        ctx.register_service(erased);
        Ok(backend)
    }

    /// Enforce the per-call policy against `target` and return the EXACT
    /// target the mutation must use, so the checked identity is the mutated
    /// one (no check-here-write-there TOCTOU). `read-only` denies;
    /// `workspace-write` re-canonicalizes NOW, requires containment under a
    /// writable root, and returns THAT fresh target; `danger-full-access`
    /// returns the caller's target unfenced. Throws the structured
    /// `FS_SANDBOX_DENIED` on refusal.
    async fn checked_target(
        &self,
        target: &FsTarget,
        sandbox_policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsTarget, FsError> {
        let policy = match sandbox_policy {
            Some(policy) => policy.clone(),
            None => self.policy.resolve(&dsh_sandbox_policy::SandboxPolicyRequest::default()),
        };
        match policy.mode {
            SandboxMode::DangerFullAccess => return Ok(target.clone()),
            SandboxMode::ReadOnly => {
                return Err(FsError::new(
                    format!(
                        "cannot write \"{}\": file access denied under read-only mode",
                        target.display_path
                    ),
                    FsErrorCode::FsSandboxDenied,
                ))
            }
            SandboxMode::WorkspaceWrite => {}
        }
        // workspace-write: containment on the FRESH canonical path (catches
        // a symlink ancestor swapped since the tool resolved this target),
        // and the mutation delegates with THIS fresh target — never the
        // stale one.
        let fresh = self.local.resolve(&target.display_path, None).await?;
        let mut contained = false;
        for root in writable_roots(&policy) {
            if is_path_under(fresh.target_key.as_str(), &root, None)
                .await
                .map_err(|error| FsError::new(error, FsErrorCode::FsIoError))?
            {
                contained = true;
                break;
            }
        }
        if !contained {
            return Err(FsError::new(
                format!(
                    "cannot write \"{}\": file access denied under workspace-write mode",
                    target.display_path
                ),
                FsErrorCode::FsSandboxDenied,
            ));
        }
        Ok(fresh)
    }
}

#[async_trait::async_trait]
impl FileSystem for SandboxedFileSystem {
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        Some(self.default_mode)
    }

    async fn resolve(&self, path: &str, opts: Option<&ResolveOptions>) -> Result<FsTarget, FsError> {
        self.local.resolve(path, opts).await
    }

    fn process_path(&self, target: &FsTarget) -> String {
        self.local.process_path(target)
    }

    fn file_url(&self, target: &FsTarget) -> String {
        self.local.file_url(target)
    }

    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        self.local.contains(parent, child)
    }

    async fn stat(&self, target: &FsTarget, signal: Option<AbortPredicate>) -> Result<Option<dsh_fs::FsInfo>, FsError> {
        self.local.stat(target, signal).await
    }

    async fn lstat(
        &self,
        path: &str,
        opts: Option<&dsh_fs::LstatOptions>,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<dsh_fs::FsPathInfo>, FsError> {
        self.local.lstat(path, opts, signal).await
    }

    async fn read_text(&self, target: &FsTarget, signal: Option<AbortPredicate>) -> Result<String, FsError> {
        self.local.read_text(target, signal).await
    }

    async fn stream_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<futures::stream::BoxStream<'static, Result<String, FsError>>, FsError> {
        self.local.stream_text(target, signal).await
    }

    async fn read_bytes(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        self.local.read_bytes(target, signal, max_bytes).await
    }

    async fn list_dir(&self, target: &FsTarget, signal: Option<AbortPredicate>) -> Result<Vec<dsh_fs::FsDirEntry>, FsError> {
        self.local.list_dir(target, signal).await
    }

    async fn write_text(
        &self,
        target: &FsTarget,
        content: &str,
        expected: Option<&FsWriteIntent>,
        signal: Option<AbortPredicate>,
        sandbox_policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsWriteOutcome, FsError> {
        let checked = self.checked_target(target, sandbox_policy).await?;
        self.local.write_text(&checked, content, expected, signal, None).await
    }

    async fn edit_text(
        &self,
        target: &FsTarget,
        edit: &FsEditRequest,
        expected: Option<&FsEditGuard>,
        signal: Option<AbortPredicate>,
        sandbox_policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsEditOutcome, FsError> {
        let checked = self.checked_target(target, sandbox_policy).await?;
        self.local.edit_text(&checked, edit, expected, signal, None).await
    }
}
