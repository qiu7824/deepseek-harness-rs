//! Rust port of the core subset of the TS `fs-sandbox.spec.ts` suite: the
//! per-call policy fence on write/edit, reads passing through, the
//! containment matrix (`..` traversal, outside paths, symlink escapes), and
//! the TOCTOU-direction re-canonicalization.
//!
//! Deviations:
//!
//! - The workspace lives under `env::temp_dir()`'s PARENT-independent root
//!   is impossible (temp itself is a grant); the tests use a base under the
//!   user profile's temp on Windows and an explicit temp sibling elsewhere —
//!   the "outside" dir is a non-temp path so denials are real.

use std::sync::Arc;

use cordis::Context;
use dsh_fs::{FileSystem, FsErrorCode};
use dsh_fs_sandbox::SandboxedFileSystem;
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use dsh_sandbox_policy::{Config as PolicyConfig, SandboxPolicyService};

struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new() -> Self {
        // The base lives OUTSIDE the temp grant (under the temp dir's
        // parent) so a `..` traversal out of the workspace is a real
        // denial — the TS spec roots under HOME for the same reason.
        let parent = std::env::temp_dir()
            .parent()
            .map(|parent| parent.join("dsh-fssbx-rs"))
            .unwrap_or_else(|| std::env::temp_dir().join("dsh-fssbx-rs"));
        std::fs::create_dir_all(&parent).expect("parent");
        let base = parent.join(format!("{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).expect("root");
        Self(base)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A directory OUTSIDE every writable grant: the profile home's temp is a
/// grant, so the "outside" must live under a sibling of the workspace.
fn outside_base() -> std::path::PathBuf {
    let base = std::env::temp_dir()
        .parent()
        .map(|parent| parent.join("dsh-fssbx-out-rs"))
        .unwrap_or_else(|| std::env::temp_dir().join("dsh-fssbx-out-rs"));
    std::fs::create_dir_all(&base).expect("outside base");
    base
}

fn boot(ctx: &Context, mode: SandboxMode, workspace: &str) -> Arc<SandboxedFileSystem> {
    SandboxPolicyService::install(
        ctx,
        PolicyConfig {
            mode: Some(mode),
            workspace_root: Some(workspace.to_string()),
        },
    );
    SandboxedFileSystem::install(
        ctx,
        dsh_fs_local::Config {
            cwd: Some(workspace.to_string()),
            diff_basis_max_bytes: None,
        },
    )
    .expect("backend")
}

#[tokio::test(flavor = "current_thread")]
async fn reports_the_deployment_default_mode() {
    let temp = TempRoot::new();
    let ctx = Context::root();
    let backend = boot(&ctx, SandboxMode::WorkspaceWrite, &temp.path("ws"));
    assert_eq!(backend.sandbox_mode(), Some(SandboxMode::WorkspaceWrite));
}

#[tokio::test(flavor = "current_thread")]
async fn read_only_denies_write_and_edit_but_allows_reads() {
    let temp = TempRoot::new();
    let workspace = temp.path("ws");
    std::fs::create_dir_all(&workspace).expect("ws");
    let ctx = Context::root();
    let backend = boot(&ctx, SandboxMode::ReadOnly, &workspace);

    let denied = temp.path("ws/denied.txt");
    let error = backend
        .write_text(
            &backend.resolve("denied.txt", None).await.expect("resolve"),
            "x",
            None,
            None,
            None,
        )
        .await
        .err()
        .expect("denied");
    assert_eq!(error.code, FsErrorCode::FsSandboxDenied);
    assert!(!std::path::Path::new(&denied).exists());

    // Reads pass through in every mode.
    std::fs::write(temp.path("ws/readable.txt"), "hello").expect("write");
    let target = backend
        .resolve("readable.txt", None)
        .await
        .expect("resolve");
    assert_eq!(
        backend.read_text(&target, None).await.expect("read"),
        "hello"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_write_contains_and_denies_traversal_and_symlink_escapes() {
    let temp = TempRoot::new();
    let workspace = temp.path("ws");
    std::fs::create_dir_all(&workspace).expect("ws");
    let outside = temp.path("out");
    std::fs::create_dir_all(&outside).expect("out");
    let ctx = Context::root();
    let backend = boot(&ctx, SandboxMode::WorkspaceWrite, &workspace);

    // A write under the workspace lands.
    let ok = backend
        .write_text(
            &backend
                .resolve("nested/ok.txt", None)
                .await
                .expect("resolve"),
            "inside",
            None,
            None,
            None,
        )
        .await
        .expect("write");
    assert_eq!(ok.after, "inside");

    // An absolute path outside the workspace is denied.
    let escape = temp.path("out/escape.txt");
    let error = backend
        .write_text(
            &backend.resolve(&escape, None).await.expect("resolve"),
            "x",
            None,
            None,
            None,
        )
        .await
        .err()
        .expect("denied");
    assert_eq!(error.code, FsErrorCode::FsSandboxDenied);
    assert!(!std::path::Path::new(&escape).exists());

    // A `..` traversal out of the workspace is denied.
    let error = backend
        .write_text(
            &backend
                .resolve("../sibling-escape.txt", None)
                .await
                .expect("resolve"),
            "x",
            None,
            None,
            None,
        )
        .await
        .err()
        .expect("denied");
    assert_eq!(error.code, FsErrorCode::FsSandboxDenied);

    // A symlinked directory inside the workspace pointing OUT is denied
    // (canonicalized before containment).
    let link = temp.path("ws/link");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside, &link).expect("symlink");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    let error = backend
        .write_text(
            &backend.resolve("link/f.txt", None).await.expect("resolve"),
            "x",
            None,
            None,
            None,
        )
        .await
        .err()
        .expect("denied");
    assert_eq!(error.code, FsErrorCode::FsSandboxDenied);
    assert!(!std::path::Path::new(&outside).join("f.txt").exists());

    // An edit inside the workspace lands; outside is denied.
    std::fs::write(temp.path("ws/edit.txt"), "original").expect("write");
    let edit = backend
        .edit_text(
            &backend.resolve("edit.txt", None).await.expect("resolve"),
            &dsh_fs::FsEditRequest {
                old_string: "original".to_string(),
                new_string: "changed".to_string(),
                replace_all: false,
            },
            None,
            None,
            None,
        )
        .await
        .expect("edit");
    assert_eq!(edit.after, "changed");
}

#[tokio::test(flavor = "current_thread")]
async fn mutates_the_freshly_checked_identity_not_a_stale_outside_target_key() {
    let temp = TempRoot::new();
    let workspace = temp.path("ws");
    std::fs::create_dir_all(&workspace).expect("ws");
    let ctx = Context::root();
    let backend = boot(&ctx, SandboxMode::WorkspaceWrite, &workspace);

    let inside_path = temp.path("ws/landed.txt");
    let stale_target = dsh_fs::FsTarget {
        display_path: inside_path.clone(),
        target_key: dsh_fs::fs_target_key(temp.path("out/escaped.txt")),
    };
    backend
        .write_text(&stale_target, "inside", None, None, None)
        .await
        .expect("write");
    assert_eq!(
        std::fs::read_to_string(&inside_path).expect("read"),
        "inside"
    );
    assert!(!std::path::Path::new(&temp.path("out/escaped.txt")).exists());
}

#[tokio::test(flavor = "current_thread")]
async fn danger_full_access_writes_anywhere_unfenced() {
    let temp = TempRoot::new();
    let workspace = temp.path("ws");
    let outside = temp.path("out");
    std::fs::create_dir_all(&workspace).expect("ws");
    std::fs::create_dir_all(&outside).expect("out");
    let ctx = Context::root();
    let backend = boot(&ctx, SandboxMode::DangerFullAccess, &workspace);
    let path = temp.path("out/free.txt");
    backend
        .write_text(
            &backend.resolve(&path, None).await.expect("resolve"),
            "free",
            None,
            None,
            None,
        )
        .await
        .expect("write");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "free");
}

#[tokio::test(flavor = "current_thread")]
async fn the_per_call_policy_override_escalation() {
    let temp = TempRoot::new();
    let workspace = temp.path("ws");
    std::fs::create_dir_all(&workspace).expect("ws");
    let ctx = Context::root();
    let backend = boot(&ctx, SandboxMode::ReadOnly, &workspace);

    // The per-call workspace-write stamp lets a contained write land for
    // that call only.
    let escalated = SandboxExecutionPolicy {
        mode: SandboxMode::WorkspaceWrite,
        workspace_root: workspace.clone(),
        session_id: None,
    };
    backend
        .write_text(
            &backend
                .resolve("escalated.txt", None)
                .await
                .expect("resolve"),
            "granted",
            None,
            None,
            Some(&escalated),
        )
        .await
        .expect("granted");
    assert_eq!(
        std::fs::read_to_string(temp.path("ws/escalated.txt")).expect("read"),
        "granted"
    );
    // A neighboring plain call still runs under the read-only default.
    let error = backend
        .write_text(
            &backend.resolve("plain.txt", None).await.expect("resolve"),
            "x",
            None,
            None,
            None,
        )
        .await
        .err()
        .expect("denied");
    assert_eq!(error.code, FsErrorCode::FsSandboxDenied);
}
