//! Rust port of the TS `policy.spec.ts` service subset: the deployment
//! defaults, per-session resolution, the mode-override kit, and the
//! session-event invariant.
//!
//! Deviations:
//!
//! - The `systemPrompt.context` contribution is deferred (recorded in the
//!   port notes); the request-context rendering cases are not ported.
//! - The `sessions`-inject late registration walks the store list directly.

use std::sync::Arc;

use cordis::Context;
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use dsh_sandbox_policy::{
    Config, SANDBOX_MODES, SandboxPolicyRequest, SandboxPolicyService, effective_sandbox_mode,
    invariant::validate_event, set_sandbox_mode,
};
use dsh_session::{Session, SessionStore, session_id};
use serde_json::json;

fn boot(config: Config) -> (Context, Arc<SandboxPolicyService>) {
    let ctx = Context::root();
    let service = SandboxPolicyService::install(&ctx, config);
    (ctx, service)
}

fn session_with_cwd(cwd: Option<&str>) -> Arc<Session> {
    let store_ctx = Context::root();
    let store = SessionStore::install(&store_ctx);
    let session = futures::executor::block_on(store.create(
        &store_ctx,
        Some(session_id("policy-session")),
        Some(dsh_session::CreateSessionOptions {
            seed: None,
            meta: Some(dsh_session::CreateSessionMeta {
                cwd: cwd.map(|cwd| cwd.to_string()),
                ..Default::default()
            }),
        }),
    ))
    .expect("create");
    Arc::new(session)
}

#[test]
fn defaults_to_read_only_under_the_process_cwd() {
    let (_ctx, service) = boot(Config::default());
    assert_eq!(service.default_mode, SandboxMode::ReadOnly);
    let policy = service.resolve(&SandboxPolicyRequest::default());
    assert_eq!(policy.mode, SandboxMode::ReadOnly);
    assert!(std::path::Path::new(&policy.workspace_root).is_absolute());
}

#[test]
fn carries_a_configured_mode_and_resolves_the_workspace_root_absolute() {
    let (_ctx, service) = boot(Config {
        mode: Some(SandboxMode::WorkspaceWrite),
        workspace_root: Some(".".to_string()),
    });
    assert_eq!(service.default_mode, SandboxMode::WorkspaceWrite);
    let policy = service.resolve(&SandboxPolicyRequest::default());
    assert!(std::path::Path::new(&policy.workspace_root).is_absolute());
}

// Session-backed cases need a tokio runtime (the SessionStore installs its
// fibers through tokio::spawn).
#[tokio::test(flavor = "current_thread")]
async fn resolves_each_session_mode_and_cwd_together_without_changing_the_fallback() {
    let (_ctx, service) = boot(Config {
        mode: Some(SandboxMode::ReadOnly),
        workspace_root: Some(std::env::temp_dir().to_string_lossy().into_owned()),
    });
    let cwd = std::env::temp_dir().join(format!("dsh-policy-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).expect("cwd");
    let session = session_with_cwd(Some(&cwd.to_string_lossy()));

    // A session with no override runs under the default, rooted at its cwd.
    let policy = service.resolve(&SandboxPolicyRequest { session: Some(session.clone()), mode: None });
    assert_eq!(policy.mode, SandboxMode::ReadOnly);
    assert!(policy.workspace_root.contains("dsh-policy-cwd"), "{}", policy.workspace_root);

    // A session override outranks the default.
    set_sandbox_mode(&session, SandboxMode::WorkspaceWrite).expect("switch");
    let policy = service.resolve(&SandboxPolicyRequest { session: Some(session.clone()), mode: None });
    assert_eq!(policy.mode, SandboxMode::WorkspaceWrite);

    // The fallback stays unchanged for agentless calls.
    let agentless = service.resolve(&SandboxPolicyRequest::default());
    assert_eq!(agentless.mode, SandboxMode::ReadOnly);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[tokio::test(flavor = "current_thread")]
async fn lets_an_approved_mode_outrank_the_session_mode_while_retaining_its_root() {
    let (_ctx, service) = boot(Config { mode: Some(SandboxMode::ReadOnly), workspace_root: None });
    let session = session_with_cwd(None);
    set_sandbox_mode(&session, SandboxMode::WorkspaceWrite).expect("switch");
    let policy = service.resolve(&SandboxPolicyRequest {
        session: Some(session.clone()),
        mode: Some(SandboxMode::DangerFullAccess),
    });
    assert_eq!(policy.mode, SandboxMode::DangerFullAccess);
    assert_eq!(policy.session_id.as_ref(), Some(&session.header().id));
}

#[tokio::test(flavor = "current_thread")]
async fn uses_the_configured_root_when_a_session_has_no_cwd() {
    let (_ctx, service) = boot(Config {
        mode: Some(SandboxMode::WorkspaceWrite),
        workspace_root: Some(std::env::temp_dir().to_string_lossy().into_owned()),
    });
    let session = session_with_cwd(None);
    let policy = service.resolve(&SandboxPolicyRequest { session: Some(session.clone()), mode: None });
    // The fallback root resolves canonical; compare against the canonical
    // temp spelling.
    let canonical_temp = dsh_sandbox::canonical_path(&std::env::temp_dir().to_string_lossy());
    assert!(policy.workspace_root.contains(&canonical_temp), "{}", policy.workspace_root);
    let _ = policy;
}

#[tokio::test(flavor = "current_thread")]
async fn the_session_mode_kit_folds_and_appends() {
    assert_eq!(
        SANDBOX_MODES,
        &[SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite, SandboxMode::DangerFullAccess]
    );
    let session = session_with_cwd(None);
    assert_eq!(effective_sandbox_mode(&session.events()), None);
    set_sandbox_mode(&session, SandboxMode::ReadOnly).expect("switch");
    set_sandbox_mode(&session, SandboxMode::DangerFullAccess).expect("switch");
    assert_eq!(
        effective_sandbox_mode(&session.events()),
        Some(SandboxMode::DangerFullAccess)
    );
}

#[test]
fn the_invariant_validates_the_package_owned_event_fields() {
    let failures: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let fail = {
        let failures = failures.clone();
        move |message: &str| {
            failures.lock().push(message.to_string());
        }
    };
    let event = |mode: serde_json::Value| dsh_session::SessionEvent {
        type_: "sandbox/mode".to_string(),
        seq: 0,
        time: 0,
        data: json!({ "mode": mode }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    };
    validate_event(&event(json!("read-only")), &fail);
    validate_event(&event(json!("workspace-write")), &fail);
    assert!(failures.lock().is_empty());
    validate_event(&event(json!("bogus-mode")), &fail);
    assert_eq!(failures.lock().len(), 1);
    assert!(failures.lock()[0].contains("unknown mode"), "{}", failures.lock()[0]);
}
