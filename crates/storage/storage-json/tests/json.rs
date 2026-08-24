//! JSON backend specifics. Rust port of the core
//! `packages/storage/storage-json/tests/json-backend.spec.ts` behaviors
//! (the shared backend contract suite is covered by the memory backend +
//! domain tests; the format-level cases live in `src/format.rs`).

use std::sync::Arc;

use cordis::{Context, arc};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_storage::{Storage, StorageBackend, StorageErrorCode, storage_backend_service_key};
use dsh_storage_json::{Config, JsonStorageBackend, JsonStoragePlugin, invariant};
use serde_json::json;

fn descriptor() -> dsh_storage::KvUnitDescriptor {
    dsh_storage::KvUnitDescriptor {
        name: "shape".to_string(),
        version: 1,
        tables: vec!["t".to_string()],
        has_global: true,
    }
}

fn fresh_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("dsh-storage-json-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn cleanup(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_a_human_readable_pretty_printed_file() {
    let root = fresh_root("pretty");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    unit.put_record("t", "k", json!({"hello": "world"}))
        .await
        .expect("put");
    let text = std::fs::read_to_string(root.join("shape.json")).expect("read");
    let expected = "{\n  \"unit\": {\n    \"name\": \"shape\",\n    \"version\": 1\n  },\n  \"global\": null,\n  \"tables\": {\n    \"t\": {\n      \"k\": {\n        \"hello\": \"world\"\n      }\n    }\n  }\n}\n";
    assert_eq!(text, expected);
    let _ = backend.close().await;
    cleanup(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn defers_materialization_until_the_first_write() {
    let root = fresh_root("lazy");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let _unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    assert!(!root.join("shape.json").exists());
    let _ = backend.close().await;
    cleanup(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_malformed_medium_and_a_foreign_unit_header() {
    let root = fresh_root("malformed");
    std::fs::create_dir_all(&root).expect("mkdir");
    std::fs::write(root.join("shape.json"), "not json at all").expect("write");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert_eq!(error.code, StorageErrorCode::MalformedMedium);

    std::fs::write(
        root.join("shape.json"),
        json!({"unit": {"name": "other", "version": 1}, "global": null, "tables": {}}).to_string(),
    )
    .expect("write");
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert_eq!(error.code, StorageErrorCode::MalformedMedium);
    let _ = backend.close().await;
    cleanup(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_double_open_of_one_unit_as_a_plain_caller_error() {
    let root = fresh_root("double");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let _unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert!(error.message.contains("already open"), "{}", error.message);
    let _ = backend.close().await;
    cleanup(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn rolls_back_memory_when_a_publish_fails() {
    let root = fresh_root("rollback");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    unit.put_record("t", "k", json!({"v": "committed"}))
        .await
        .expect("put");
    unit.set_global(json!({"g": "committed"}))
        .await
        .expect("set");
    let path = root.join("shape.json");
    let backup = root.join("shape.committed.json");
    // A directory at the publish target rejects atomic replacement on every
    // host.
    std::fs::rename(&path, &backup).expect("rename");
    std::fs::create_dir(&path).expect("mkdir");
    assert!(
        unit.put_record("t", "k", json!({"v": "rejected"}))
            .await
            .is_err()
    );
    assert!(
        unit.put_record("t", "k2", json!({"v": "also rejected"}))
            .await
            .is_err()
    );
    assert!(unit.delete_record("t", "k").await.is_err());
    assert!(unit.set_global(json!({"g": "rejected"})).await.is_err());
    std::fs::remove_dir_all(&path).expect("remove dir");
    std::fs::rename(&backup, &path).expect("restore");
    let snapshot = unit.load_all().await.expect("load");
    assert_eq!(snapshot.tables["t"]["k"], json!({"v": "committed"}));
    assert!(!snapshot.tables["t"].contains_key("k2"));
    assert_eq!(snapshot.global, json!({"g": "committed"}));
    // The next successful publish must not carry rejected writes to disk.
    unit.put_record("t", "k3", json!({"v": "later"}))
        .await
        .expect("put");
    let text = std::fs::read_to_string(&path).expect("read");
    assert!(!text.contains("rejected"), "{text}");
    let _ = backend.close().await;
    cleanup(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_undeclared_table_and_global_access_as_caller_errors() {
    let root = fresh_root("undeclared");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let descriptor = dsh_storage::KvUnitDescriptor {
        name: "shape".to_string(),
        version: 1,
        tables: vec!["t".to_string()],
        has_global: false,
    };
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor)
        .await
        .expect("open");
    let error = unit
        .put_record("undeclared", "k", json!({}))
        .await
        .expect_err("reject");
    assert!(
        error.message.contains("does not declare table"),
        "{}",
        error.message
    );
    let error = unit.set_global(json!({})).await.expect_err("reject");
    assert!(
        error.message.contains("does not declare a global slot"),
        "{}",
        error.message
    );
    let _ = backend.close().await;
    cleanup(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_invalid_unit_and_table_names_and_closed_backend() {
    let root = fresh_root("names");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let bad_name = dsh_storage::KvUnitDescriptor {
        name: "Bad-Name".to_string(),
        version: 1,
        tables: vec!["t".to_string()],
        has_global: true,
    };
    let error = backend
        .kv()
        .expect("kv")
        .open(&bad_name)
        .await
        .err()
        .expect("reject");
    assert_eq!(error.code, StorageErrorCode::MalformedMedium);
    let bad_table = dsh_storage::KvUnitDescriptor {
        name: "shape".to_string(),
        version: 1,
        tables: vec!["ok".to_string(), "not ok".to_string()],
        has_global: true,
    };
    let error = backend
        .kv()
        .expect("kv")
        .open(&bad_table)
        .await
        .err()
        .expect("reject");
    assert_eq!(error.code, StorageErrorCode::MalformedMedium);
    let _ = backend.close().await;
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert_eq!(error.code, StorageErrorCode::Closed);
    cleanup(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn opens_a_file_missing_a_declared_table_as_that_table_empty() {
    let root = fresh_root("missing-table");
    std::fs::create_dir_all(&root).expect("mkdir");
    std::fs::write(
        root.join("contract_unit.json"),
        json!({
            "unit": {"name": "contract_unit", "version": 3},
            "global": null,
            "tables": {"alpha": {"k": 1}},
        })
        .to_string(),
    )
    .expect("write");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let descriptor = dsh_storage::KvUnitDescriptor {
        name: "contract_unit".to_string(),
        version: 3,
        tables: vec!["alpha".to_string(), "beta".to_string()],
        has_global: true,
    };
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor)
        .await
        .expect("open");
    let snapshot = unit.load_all().await.expect("load");
    assert_eq!(snapshot.tables["alpha"]["k"], json!(1));
    assert!(snapshot.tables["beta"].is_empty());
    let _ = backend.close().await;
    cleanup(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn propagates_non_missing_read_failures() {
    let root = fresh_root("read-failure");
    std::fs::create_dir_all(&root).expect("mkdir");
    // A directory where the unit file should be: the read rejects.
    std::fs::create_dir(root.join("shape.json")).expect("mkdir dir");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert!(
        error.message.contains("failed to read"),
        "{}",
        error.message
    );
    let _ = backend.close().await;
    cleanup(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_malformed_table_shapes_and_foreign_versions_distinctly() {
    let root = fresh_root("shapes");
    std::fs::create_dir_all(&root).expect("mkdir");
    std::fs::write(
        root.join("shape.json"),
        json!({"unit": {"name": "shape", "version": 1}, "global": null, "tables": {"t": ["not", "an", "object"]}})
            .to_string(),
    )
    .expect("write");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert_eq!(error.code, StorageErrorCode::MalformedMedium);

    std::fs::write(
        root.join("shape.json"),
        json!({"unit": {"name": "shape", "version": 9}, "global": null, "tables": {}}).to_string(),
    )
    .expect("write");
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert_eq!(error.code, StorageErrorCode::VersionMismatch);

    std::fs::write(
        root.join("shape.json"),
        json!({"unit": {"name": "shape", "version": 1}, "global": null}).to_string(),
    )
    .expect("write");
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert_eq!(error.code, StorageErrorCode::MalformedMedium);
    let _ = backend.close().await;
    cleanup(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn registers_on_the_hub_via_apply_and_closes_on_dispose() {
    let root = fresh_root("apply");
    let ctx = Context::root();
    let _hub = Storage::install(&ctx);
    let fiber = ctx.plugin(
        Arc::new(JsonStoragePlugin {
            config: Config {
                root: root.to_string_lossy().to_string(),
            },
        }),
        arc(()),
    );
    fiber.settle().await.expect("settle");
    let lifecycle = ctx
        .get_typed::<Arc<JsonStorageBackend>>(&storage_backend_service_key("json"), false)
        .expect("lifecycle service")
        .as_ref()
        .clone();
    let unit = lifecycle
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    unit.put_record("t", "k", json!({"v": 1}))
        .await
        .expect("put");
    fiber.dispose().await;
    // After dispose: the hub registry name is gone and the unit is closed.
    let error = ctx
        .get_typed::<Arc<Storage>>("storage", false)
        .expect("hub")
        .backend
        .get("json")
        .err()
        .expect("unregistered");
    assert_eq!(error.code, StorageErrorCode::BackendNotFound);
    let error = unit
        .put_record("t", "x", json!({}))
        .await
        .expect_err("closed");
    assert_eq!(error.code, StorageErrorCode::Closed);
    cleanup(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn registers_the_invariant_companion_and_disposes_cleanly() {
    let ctx = Context::root();
    let _registry = InvariantRegistry::new(
        &ctx,
        InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let fiber = ctx.plugin(Arc::new(invariant::JsonStorageInvariantPlugin), arc(()));
    fiber.settle().await.expect("settle");
    fiber.dispose().await;
    // Disposal releases the reservation: a fresh mount succeeds.
    let again = ctx.plugin(Arc::new(invariant::JsonStorageInvariantPlugin), arc(()));
    again.settle().await.expect("settle");
    again.dispose().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn close_drains_in_flight_writes_and_blocks_in_flight_opens() {
    let root = fresh_root("drain");
    let backend = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    // Start the write INLINE (one manual poll runs the synchronous prefix —
    // the closed guard and the state mutation — and parks in the publish),
    // so it is genuinely in flight when close lands.
    let write_future = unit.put_record("t", "big", json!({"blob": "x".repeat(4 * 1024 * 1024)}));
    futures::pin_mut!(write_future);
    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    assert!(
        write_future.as_mut().poll(&mut cx).is_pending(),
        "write must be in flight"
    );
    // Drive close on a real task; completing the write future below runs
    // the publish's continuation (releasing the in-flight slot), which the
    // close's drain barrier is waiting for.
    let unit_for_close = unit.clone();
    let close_task = tokio::spawn(async move { unit_for_close.close().await });
    write_future.await.expect("in-flight write resolves");
    close_task.await.expect("close drains the in-flight write");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("shape.json")).expect("read"))
            .expect("json");
    assert!(on_disk["tables"]["t"]["big"].is_object());

    let backend2 = JsonStorageBackend::new(root.to_string_lossy().to_string());
    let facet2 = backend2.kv().expect("kv").clone();
    let opening = tokio::spawn(async move { facet2.open(&descriptor()).await });
    tokio::task::yield_now().await;
    let _ = backend2.close().await;
    // The open races the close; both orderings surface a `closed` failure.
    let opened = opening.await.expect("task");
    let error = match opened {
        Ok(unit2) => unit2
            .put_record("t", "x", json!({}))
            .await
            .expect_err("closed"),
        Err(error) => error,
    };
    assert_eq!(error.code, StorageErrorCode::Closed);
    cleanup(&root);
}
