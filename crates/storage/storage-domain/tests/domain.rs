//! Domain-runtime behavior over the in-memory backend: spec validation,
//! write-chain ordering, change events, close semantics, and the
//! version-mismatch / invalid-record boundaries. Rust port of the TS
//! contract text (`packages/storage/storage/src/backend.ts` + the domain
//! layer's runtime semantics; the TS package ships no standalone spec).

use std::sync::Arc;

use cordis::Context;
use dsh_storage::Storage;
use dsh_storage_domain::{
    DomainChanged, DomainFacility, DomainFacilityConfig, define_domain, domain_global, domain_table,
};
use dsh_storage_test_support::{MemoryMediaPool, MemoryStorageBackend};
use serde_json::{Value as JsonValue, json};

fn accept_all() -> dsh_storage_domain::RecordSchema {
    Arc::new(|_value| Ok(()))
}

fn marks_schema() -> dsh_storage_domain::RecordSchema {
    Arc::new(|value: &JsonValue| match value {
        JsonValue::Object(object) if object.contains_key("marks") => Ok(()),
        other => Err(format!("expected an object with marks, got {other}")),
    })
}

fn install_hub(ctx: &Context, pool: Arc<MemoryMediaPool>) -> (Arc<Storage>, Arc<DomainFacility>) {
    let hub = Storage::install(ctx);
    let backend = MemoryStorageBackend::with_shared_pool(pool);
    hub.backend
        .register("memory", backend)
        .expect("register backend");
    let facility = DomainFacility::install(
        ctx,
        DomainFacilityConfig {
            backend: "memory".to_string(),
            routes: Default::default(),
        },
    )
    .expect("facility");
    (hub, facility)
}

#[test]
fn define_domain_validates_names_versions_and_global_null_rejection() {
    let table = indexmap::IndexMap::new();
    assert!(define_domain("BadName", 1, None, table.clone()).is_err());
    assert!(define_domain("good_name", 1, None, table.clone()).is_ok());
    let mut tables = indexmap::IndexMap::new();
    tables.insert("BadTable".to_string(), domain_table(accept_all()));
    assert!(define_domain("good_name", 1, None, tables).is_err());
    // A global schema that accepts null cannot round-trip the sentinel.
    let nullable = domain_global(accept_all(), json!({"x": 1}));
    assert!(define_domain("g", 1, Some(nullable), indexmap::IndexMap::new()).is_err());
    let strict = domain_global(marks_schema(), json!({"marks": []}));
    assert!(define_domain("g", 1, Some(strict), indexmap::IndexMap::new()).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn open_puts_and_reads_round_trip_with_change_events() {
    let ctx = Context::root();
    let pool = Arc::new(MemoryMediaPool::new());
    let facility = install_hub(&ctx, pool.clone()).1;
    let spec = define_domain(
        "sessions",
        1,
        None,
        indexmap::IndexMap::from([("rows".to_string(), domain_table(marks_schema()))]),
    )
    .expect("spec");

    let changes: Arc<parking_lot::Mutex<Vec<DomainChanged>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let changes_for_listener = changes.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let change = cordis::downcast::<DomainChanged>(&args[0]).cloned();
        let changes = changes_for_listener.clone();
        Box::pin(async move {
            if let Some(change) = change {
                changes.lock().push(change);
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "domain/changed",
        listener,
        cordis::EventOptions::default(),
    ));

    let domain = facility.open(&spec).await.expect("open");
    let table = domain.table("rows");
    assert!(table.get("a").is_none());
    table.put("a", json!({"marks": ["x"]})).await.expect("put");
    assert_eq!(table.get("a"), Some(json!({"marks": ["x"]})));
    assert_eq!(table.len(), 1);
    assert_eq!(table.keys(), vec!["a".to_string()]);
    assert_eq!(
        table.entries(),
        vec![("a".to_string(), json!({"marks": ["x"]}))]
    );

    let updated = table
        .update(
            "a",
            Arc::new(|mut current| {
                current["marks"] = json!(["x", "y"]);
                current
            }),
        )
        .await
        .expect("update");
    assert_eq!(updated["marks"], json!(["x", "y"]));
    let missing = table
        .update("absent", Arc::new(|value| value))
        .await
        .err()
        .expect("missing-key");
    assert!(
        missing.contains("no record 'absent' to update"),
        "{missing}"
    );

    assert!(table.delete("a").await.expect("delete"));
    assert!(!table.delete("a").await.expect("delete idempotent"));

    // The memory backend holds the committed shapes (events too, in order;
    // the fire-and-forget dispatch needs a yield before the assert).
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    let stored = pool.media.lock();
    let medium = stored.get("sessions").expect("medium");
    assert!(medium.tables.get("rows").expect("table").is_empty());
    drop(stored);
    let changes = changes.lock();
    // put(a), update(a), delete(a); the idempotent second delete emits
    // nothing (TS: no write and no event).
    assert_eq!(changes.len(), 3);
    assert!(matches!(&changes[0], DomainChanged::Put { .. }));
    assert!(matches!(&changes[1], DomainChanged::Put { .. }));
    assert!(matches!(&changes[2], DomainChanged::Deleted { .. }));

    domain.close().await;
    let _ = facility;
}

#[tokio::test(flavor = "current_thread")]
async fn global_slot_round_trips_initial_then_set() {
    let ctx = Context::root();
    let pool = Arc::new(MemoryMediaPool::new());
    let facility = install_hub(&ctx, pool.clone()).1;
    let spec = define_domain(
        "globals",
        1,
        Some(domain_global(marks_schema(), json!({"marks": []}))),
        indexmap::IndexMap::new(),
    )
    .expect("spec");
    let domain = facility.open(&spec).await.expect("open");
    assert_eq!(domain.global().get(), json!({"marks": []}));
    domain
        .global()
        .set(json!({"marks": ["g"]}))
        .await
        .expect("set");
    assert_eq!(domain.global().get(), json!({"marks": ["g"]}));
    domain.close().await;

    // Reopen over the same pool: the stored global survives.
    let reopened = facility.open(&spec).await.expect("reopen");
    assert_eq!(reopened.global().get(), json!({"marks": ["g"]}));
    reopened.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn double_open_rejects_and_close_frees_the_name() {
    let ctx = Context::root();
    let pool = Arc::new(MemoryMediaPool::new());
    let facility = install_hub(&ctx, pool.clone()).1;
    let spec = define_domain(
        "reopenable",
        1,
        None,
        indexmap::IndexMap::from([("rows".to_string(), domain_table(accept_all()))]),
    )
    .expect("spec");
    let domain = facility.open(&spec).await.expect("open");
    let error = facility.open(&spec).await.err().expect("double-open");
    assert!(error.contains("already open"), "{error}");
    domain.close().await;
    let again = facility.open(&spec).await.expect("reopen after close");
    again.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn version_mismatch_and_invalid_records_reject_at_open() {
    let ctx = Context::root();
    let pool = Arc::new(MemoryMediaPool::new());
    pool.versions.lock().insert("versioned".to_string(), 7);
    let facility = install_hub(&ctx, pool.clone()).1;
    let spec = define_domain(
        "versioned",
        1,
        None,
        indexmap::IndexMap::from([("rows".to_string(), domain_table(accept_all()))]),
    )
    .expect("spec");
    let error = facility.open(&spec).await.err().expect("version-mismatch");
    assert!(error.contains("stamped v7"), "{error}");

    // A stored record violating the table schema rejects as invalid-record.
    let invalid_ctx = Context::root();
    let pool = Arc::new(MemoryMediaPool::new());
    {
        let mut media = pool.media.lock();
        let mut tables = std::collections::HashMap::new();
        let mut rows = std::collections::HashMap::new();
        rows.insert("bad".to_string(), json!({"not": "marks"}));
        tables.insert("rows".to_string(), rows);
        media.insert(
            "validated".to_string(),
            dsh_storage_test_support::MemoryMedium {
                tables,
                global: JsonValue::Null,
            },
        );
    }
    let facility2 = install_hub(&invalid_ctx, pool.clone()).1;
    let validated = define_domain(
        "validated",
        1,
        None,
        indexmap::IndexMap::from([("rows".to_string(), domain_table(marks_schema()))]),
    )
    .expect("spec");
    let error = facility2
        .open(&validated)
        .await
        .err()
        .expect("invalid-record");
    assert!(
        error.contains("stored record 'bad' in table 'rows' does not match its schema"),
        "{error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn writes_reject_after_close_and_queued_writes_drain() {
    let ctx = Context::root();
    let pool = Arc::new(MemoryMediaPool::new());
    let facility = install_hub(&ctx, pool.clone()).1;
    let spec = define_domain(
        "closing",
        1,
        None,
        indexmap::IndexMap::from([("rows".to_string(), domain_table(accept_all()))]),
    )
    .expect("spec");
    let domain = facility.open(&spec).await.expect("open");
    let table = domain.table("rows");
    table.put("a", json!(1)).await.expect("put");
    let _ = table.put("b", json!(2)).await.expect("put");
    domain.close().await;
    let error = table.put("c", json!(3)).await.err().expect("closed");
    assert!(error.contains("is closed"), "{error}");
}
