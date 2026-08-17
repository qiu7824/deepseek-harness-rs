//! Mount audit integration tests: Rust port of the core subset of
//! `tests/mount.spec.ts`.
//!
//! Covers: successful standing mount + records, the unusable-row audit, the
//! root-realm leak audit (with the isolate escape hatch), the
//! missing-file / broken-composition diagnostics, and the
//! `standingMountFor` lookup.

mod common;

use common::{boot, scoped, seed_preset, temp_dir};
use std::sync::Mutex;

/// The mount records live in a process-global table (as in TS); serialize
/// the tests that observe it so parallel runs cannot cross-pollute.
static MOUNT_TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    MOUNT_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const VALID: &str = "- id: alpha\n  name: contribute\n  config:\n    tool: alpha\n";
const PENDING: &str = "- id: waits\n  name: needs-missing\n";
const LEAKY: &str = "- id: leak-z\n  name: global-service\n  config:\n    service: zzzFixtureLeakedSvc\n    label: LEAKED-Z\n- id: leak-a\n  name: global-service\n  config:\n    service: aaaFixtureLeakedSvc\n    label: LEAKED-A\n";
const ISOLATED: &str = "- id: svc\n  name: global-service\n  isolate:\n    fixtureIsolatedSvc: true\n  config:\n    service: fixtureIsolatedSvc\n    label: ISOLATED\n";
const SELF_DISPOSE: &str = "- id: gone\n  name: self-dispose\n";

#[tokio::test]
async fn mounts_a_usable_preset_and_records_it() {
    let guard = serial();
    let ctx = boot().await;
    let root = temp_dir("mount-ok");
    let preset = seed_preset(&root, "standard", VALID).await;
    let (scope, _key) = scoped(&ctx);

    dsh_agent_presets::mount_preset(&scope.ctx, &preset)
        .await
        .expect("mount succeeds");

    let mounts = dsh_agent_presets::live_preset_mounts();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].preset_id, "standard");
    assert!(mounts[0].key.is_some());

    // The agent's scope finds the standing composition through its key.
    let joined = dsh_agent_presets::standing_mount_for(&scope.ctx);
    assert!(joined.is_none(), "no parent link until bind_scope_parent");

    (scope.dispose)().await;
    let mounts = dsh_agent_presets::live_preset_mounts();
    assert!(
        mounts.is_empty(),
        "disposed subtree prunes the mount record"
    );
    drop(guard);
}

#[tokio::test]
async fn refuses_a_preset_whose_rows_wait_for_a_missing_service() {
    let guard = serial();
    let ctx = boot().await;
    let root = temp_dir("mount-pending");
    let preset = seed_preset(&root, "pending", PENDING).await;
    let (scope, _key) = scoped(&ctx);

    let error = dsh_agent_presets::mount_preset(&scope.ctx, &preset)
        .await
        .expect_err("pending rows reject the mount");
    assert!(
        error.reason.contains("row(s) did not activate"),
        "unexpected diagnostic: {error}"
    );
    assert!(
        error
            .reason
            .contains("waits (needs-missing): waiting for serviceThatDoesNotExist"),
        "names the pending row: {error}"
    );
    drop(guard);
}

#[tokio::test]
async fn refuses_rows_that_publish_into_the_root_realm() {
    let guard = serial();
    let ctx = boot().await;
    let root = temp_dir("mount-leaky");
    let preset = seed_preset(&root, "leaky", LEAKY).await;
    let (scope, _key) = scoped(&ctx);

    let error = dsh_agent_presets::mount_preset(&scope.ctx, &preset)
        .await
        .expect_err("root-realm services reject the mount");
    assert!(
        error.reason.contains("process-global service(s)"),
        "unexpected diagnostic: {error}"
    );
    // Lexical order, matching the TS assertion.
    assert!(
        error
            .reason
            .contains("[aaaFixtureLeakedSvc, zzzFixtureLeakedSvc]"),
        "leak names are ordered: {error}"
    );
    drop(guard);
}

#[tokio::test]
async fn accepts_the_same_provider_behind_an_isolate_realm() {
    let guard = serial();
    let ctx = boot().await;
    let root = temp_dir("mount-isolated");
    let preset = seed_preset(&root, "isolated", ISOLATED).await;
    let (scope, _key) = scoped(&ctx);

    dsh_agent_presets::mount_preset(&scope.ctx, &preset)
        .await
        .expect("isolate-realm services are per-session");
    (scope.dispose)().await;
    drop(guard);
}

#[tokio::test]
async fn refuses_a_preset_with_a_missing_composition_file() {
    let guard = serial();
    let ctx = boot().await;
    let root = temp_dir("mount-missing");
    let preset = dsh_agent_presets::AgentPreset {
        id: "hole".to_string(),
        trust: dsh_agent_presets::PresetTrust::User,
        path: root
            .join("hole")
            .join(dsh_agent_presets::COMPOSITION_FILE)
            .to_string_lossy()
            .to_string(),
        name: None,
        description: None,
        order: None,
        broken: None,
    };
    let (scope, _key) = scoped(&ctx);

    let error = dsh_agent_presets::mount_preset(&scope.ctx, &preset)
        .await
        .expect_err("a missing composition fails the mount");
    assert!(
        error.reason.contains("config file not found"),
        "unexpected diagnostic: {error}"
    );
    drop(guard);
}

#[tokio::test]
async fn reports_every_broken_row_in_one_flat_diagnostic() {
    let guard = serial();
    let ctx = boot().await;
    let root = temp_dir("mount-two-broken");
    let preset = seed_preset(
        &root,
        "two-broken",
        "- id: first-missing\n  name: does-not-exist\n- id: second-missing\n  name: also-missing\n",
    )
    .await;
    let (scope, _key) = scoped(&ctx);

    let error = dsh_agent_presets::mount_preset(&scope.ctx, &preset)
        .await
        .expect_err("unresolvable rows fail the mount");
    // Both rows appear in the flattened diagnostic.
    assert!(
        error.reason.contains("first-missing"),
        "first row named: {error}"
    );
    assert!(
        error.reason.contains("second-missing"),
        "second row named: {error}"
    );
    drop(guard);
}

#[tokio::test]
async fn a_self_disposing_entry_never_writes_the_preset_back() {
    let guard = serial();
    let ctx = boot().await;
    let root = temp_dir("mount-self-dispose");
    let preset = seed_preset(&root, "volatile", SELF_DISPOSE).await;

    let original = tokio::fs::read_to_string(&preset.path)
        .await
        .expect("read composition");
    let (scope, _key) = scoped(&ctx);
    dsh_agent_presets::mount_preset(&scope.ctx, &preset)
        .await
        .expect("mount succeeds");
    // Give the self-dispose write-back path a beat to misbehave.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let after = tokio::fs::read_to_string(&preset.path)
        .await
        .expect("read composition after");
    assert_eq!(
        after, original,
        "a self-disposing entry must never truncate the preset file"
    );
    (scope.dispose)().await;
    drop(guard);
}

#[tokio::test]
async fn refuses_an_unscoped_context() {
    let guard = serial();
    let ctx = boot().await;
    let root = temp_dir("mount-unscoped");
    let preset = seed_preset(&root, "standard", VALID).await;

    let error = dsh_agent_presets::mount_preset(&ctx, &preset)
        .await
        .expect_err("an unscoped context must not mount");
    assert!(
        error.reason.contains("unscoped context"),
        "unexpected diagnostic: {error}"
    );
    drop(guard);
}
