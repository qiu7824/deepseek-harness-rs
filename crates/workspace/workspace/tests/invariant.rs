//! Rust port of the TS
//! `packages/workspace/workspace/tests/invariant.spec.ts` suite: the
//! package-owned cache/table invariant companion.
//!
//! Deviation: the TS `ctx.emit` dispatch is synchronous there, so a
//! violating emit throws inline; the Rust `ctx.emit` is fire-and-forget
//! (spawned), so the port tests the pure [`check_change`] logic directly
//! and exercises the live listener wiring through a collector closure.

mod common;

use std::sync::Arc;

use common::{MemoryMediaPool, TempRoot, deleted_change, harness, put_change};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_workspace::invariant::{self, check_change};

#[test]
fn accepts_a_put_whose_record_has_a_cached_entity_and_ignores_foreign_events() {
    let failures: Arc<parking_lot::Mutex<Vec<String>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let fail = |message: &str| {
        failures.lock().push(message.to_string());
    };
    check_change(&put_change("w1"), &|key| key == "w1", &fail);
    check_change(
        &dsh_storage_domain::DomainChanged::Put {
            domain: "other".to_string(),
            table: "workspaces".to_string(),
            key: "missing".to_string(),
            value: serde_json::json!({}),
        },
        &|key| key == "w1",
        &fail,
    );
    check_change(
        &dsh_storage_domain::DomainChanged::Put {
            domain: "workspace".to_string(),
            table: "other".to_string(),
            key: "missing".to_string(),
            value: serde_json::json!({}),
        },
        &|key| key == "w1",
        &fail,
    );
    assert!(failures.lock().is_empty());
}

#[test]
fn fails_deletion_while_the_registry_still_publishes_the_entity() {
    let failures: Arc<parking_lot::Mutex<Vec<String>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    check_change(&deleted_change("w1"), &|key| key == "w1", &|message| {
        failures.lock().push(message.to_string());
    });
    let failures = failures.lock();
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("cache still publishes"), "{}", failures[0]);
}

#[test]
fn allows_deletion_after_the_registry_removed_the_cache_entry() {
    let failures: Arc<parking_lot::Mutex<Vec<String>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    check_change(&deleted_change("w1"), &|_key| false, &|message| {
        failures.lock().push(message.to_string());
    });
    assert!(failures.lock().is_empty());
}

#[test]
fn fails_a_put_whose_record_the_registry_cache_does_not_hold() {
    let failures: Arc<parking_lot::Mutex<Vec<String>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    check_change(&put_change("w1"), &|_key| false, &|message| {
        failures.lock().push(message.to_string());
    });
    let failures = failures.lock();
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("diverged"), "{}", failures[0]);
}

#[tokio::test(flavor = "current_thread")]
async fn installer_listens_to_domain_changes_against_the_live_registry_cache() {
    let temp = TempRoot::new();
    let dir =
        futures::executor::block_on(dsh_workspace::realpath_normalize(&temp.dir("wired")))
            .expect("canonical");
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let workspace = result.registry.create(&dir, None).await.expect("create");
    let key = workspace.id().to_string();

    let failures: Arc<parking_lot::Mutex<Vec<String>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let fail: Arc<dyn Fn(&str) + Send + Sync> = {
        let failures = failures.clone();
        Arc::new(move |message: &str| {
            failures.lock().push(message.to_string());
        })
    };
    (invariant::installer().install)(&result.ctx, fail).await;

    // A put whose record the cache holds passes.
    result.ctx.emit("domain/changed", vec![cordis::arc(put_change(&key))]);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(failures.lock().is_empty());

    // A deletion while the registry still publishes the entity fails.
    result.ctx.emit("domain/changed", vec![cordis::arc(deleted_change(&key))]);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    {
        let failures = failures.lock();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("cache still publishes"), "{}", failures[0]);
    }

    // A put whose record the cache does not hold fails.
    result.ctx.emit("domain/changed", vec![cordis::arc(put_change("missing"))]);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let failures = failures.lock();
    assert_eq!(failures.len(), 2);
    assert!(failures[1].contains("diverged"), "{}", failures[1]);
}

#[tokio::test(flavor = "current_thread")]
async fn companion_plugin_registers_with_the_invariants_service() {
    let temp = TempRoot::new();
    let dir =
        futures::executor::block_on(dsh_workspace::realpath_normalize(&temp.dir("smoke")))
            .expect("canonical");
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    InvariantRegistry::new(&result.ctx, InvariantConfig::default());
    let disposer = invariant::apply(&result.ctx);

    // The installer child settles asynchronously; give it time to subscribe.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let workspace = result.registry.create(&dir, None).await.expect("create");
    // A valid put must pass the companion without crashing the dispatch.
    result
        .ctx
        .emit("domain/changed", vec![cordis::arc(put_change(&workspace.id().to_string()))]);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (disposer)().await;
}
