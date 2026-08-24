//! SQLite backend specifics. Rust port of the core
//! `packages/storage/storage-sqlite/tests/sqlite-backend.spec.ts` behaviors
//! (the shared backend contract suite is covered by the memory backend +
//! domain tests; the schema-level cases live in `src/schema.rs`).
//!
//! # Inexpressible cases
//!
//! - The hostile `toJSON` throw: values are `JsonValue`s, whose
//!   serialization cannot throw.
//! - The POSIX chmod/mode-preservation cases run under `#[cfg(unix)]`.

use std::sync::Arc;

use cordis::{Context, arc};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_storage::{Storage, StorageBackend, StorageErrorCode, storage_backend_service_key};
use dsh_storage_sqlite::{
    Config, JournalMode, STORAGE_SQLITE_SCHEMA_VERSION, SqliteStorageBackend, SqliteStoragePlugin,
    invariant,
};
use serde_json::json;

fn descriptor() -> dsh_storage::KvUnitDescriptor {
    dsh_storage::KvUnitDescriptor {
        name: "specimen".to_string(),
        version: 1,
        tables: vec!["records".to_string()],
        has_global: true,
    }
}

fn fresh_db_path(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dsh-storage-sqlite-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir.join("storage.db")
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

fn backend_at(path: &str) -> Arc<SqliteStorageBackend> {
    SqliteStorageBackend::new(Config {
        path: path.to_string(),
        journal_mode: JournalMode::default(),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn opens_an_in_memory_database() {
    let backend = backend_at(":memory:");
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    unit.put_record("records", "k", json!({"n": 1}))
        .await
        .expect("put");
    let snapshot = unit.load_all().await.expect("load");
    assert_eq!(snapshot.tables["records"]["k"], json!({"n": 1}));
    let _ = backend.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn materializes_strict_record_tables_and_stamps_the_schema_version() {
    let path = fresh_db_path("stamp");
    let backend = backend_at(path.to_string_lossy().as_ref());
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    unit.put_record("records", "k", json!({"n": 1}))
        .await
        .expect("put");
    let _ = backend.close().await;

    let db = rusqlite::Connection::open(&path).expect("open");
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, STORAGE_SQLITE_SCHEMA_VERSION);
    let sql: String = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'u_specimen_records'",
            [],
            |row| row.get(0),
        )
        .expect("table sql");
    assert!(sql.contains("STRICT"), "{sql}");
    let unit_row: i64 = db
        .query_row(
            "SELECT version FROM units WHERE name = ?1",
            ["specimen"],
            |row| row.get(0),
        )
        .expect("unit row");
    assert_eq!(unit_row, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_mismatched_database_schema_version() {
    let path = fresh_db_path("mismatch");
    {
        let db = rusqlite::Connection::open(&path).expect("open");
        db.execute_batch("PRAGMA user_version = 999")
            .expect("stamp");
    }
    let backend = backend_at(path.to_string_lossy().as_ref());
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert_eq!(error.code, StorageErrorCode::VersionMismatch);
    let _ = backend.close().await;
    cleanup(&path);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_invalid_unit_and_table_names_before_touching_the_medium() {
    let backend = backend_at(":memory:");
    let bad_name = dsh_storage::KvUnitDescriptor {
        name: "Bad-Name".to_string(),
        version: 1,
        tables: vec!["records".to_string()],
        has_global: true,
    };
    let error = backend
        .kv()
        .expect("kv")
        .open(&bad_name)
        .await
        .err()
        .expect("reject");
    assert!(error.message.contains("violates"), "{}", error.message);
    let bad_table = dsh_storage::KvUnitDescriptor {
        name: "specimen".to_string(),
        version: 1,
        tables: vec!["ok".to_string(), "1bad".to_string()],
        has_global: true,
    };
    let error = backend
        .kv()
        .expect("kv")
        .open(&bad_table)
        .await
        .err()
        .expect("reject");
    assert!(error.message.contains("violates"), "{}", error.message);
    let _ = backend.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_second_open_of_the_same_unit_name() {
    let backend = backend_at(":memory:");
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
}

#[tokio::test(flavor = "multi_thread")]
async fn allows_reopen_after_unit_close_and_rejects_open_on_a_closed_backend() {
    let backend = backend_at(":memory:");
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    let _ = unit.close().await;
    let again = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("reopen");
    again
        .put_record("records", "k", json!(1))
        .await
        .expect("put");
    let _ = backend.close().await;
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("closed");
    assert_eq!(error.code, StorageErrorCode::Closed);
}

#[tokio::test(flavor = "multi_thread")]
async fn round_trips_prototype_polluting_keys_as_own_properties() {
    let backend = backend_at(":memory:");
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    unit.put_record("records", "__proto__", json!({"evil": true}))
        .await
        .expect("put");
    unit.put_record("records", "constructor", json!({"n": 1}))
        .await
        .expect("put");
    let snapshot = unit.load_all().await.expect("load");
    let records = &snapshot.tables["records"];
    assert!(records.contains_key("__proto__"));
    assert_eq!(records["__proto__"], json!({"evil": true}));
    assert_eq!(records["constructor"], json!({"n": 1}));
    let _ = backend.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn leaves_a_failed_materialization_unstamped_so_a_repaired_medium_reopens() {
    let path = fresh_db_path("obstructed");
    {
        // Obstruct table creation: an index squatting on the unit_globals
        // name makes CREATE TABLE IF NOT EXISTS throw.
        let db = rusqlite::Connection::open(&path).expect("open");
        db.execute_batch("CREATE TABLE squatter (x TEXT)")
            .expect("squatter");
        db.execute_batch("CREATE INDEX unit_globals ON squatter(x)")
            .expect("index");
    }
    let broken = backend_at(path.to_string_lossy().as_ref());
    let error = broken
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert!(
        error.message.contains("unit_globals") || error.message.contains("index"),
        "{}",
        error.message
    );
    let _ = broken.close().await;

    {
        // The medium must still be version 0: no half-materialized stamp.
        let repair = rusqlite::Connection::open(&path).expect("open");
        let version: i64 = repair
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, 0);
        repair
            .execute_batch("DROP INDEX unit_globals")
            .expect("drop");
    }

    let backend = backend_at(path.to_string_lossy().as_ref());
    let unit = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    unit.put_record("records", "k", json!({"n": 1}))
        .await
        .expect("put");
    let _ = backend.close().await;
    cleanup(&path);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_unparsable_stored_json_with_malformed_medium() {
    let path = fresh_db_path("bad-json");
    {
        let backend = backend_at(path.to_string_lossy().as_ref());
        let unit = backend
            .kv()
            .expect("kv")
            .open(&descriptor())
            .await
            .expect("open");
        unit.put_record("records", "good", json!({"n": 1}))
            .await
            .expect("put");
        unit.set_global(json!({"g": 1})).await.expect("set");
        let _ = backend.close().await;
    }
    {
        let db = rusqlite::Connection::open(&path).expect("open");
        db.execute(
            "UPDATE u_specimen_records SET value = ?1 WHERE key = ?2",
            rusqlite::params!["{not json", "good"],
        )
        .expect("corrupt");
    }
    let reopened = backend_at(path.to_string_lossy().as_ref());
    let damaged = reopened
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    let error = damaged.load_all().await.expect_err("malformed");
    assert_eq!(error.code, StorageErrorCode::MalformedMedium);
    let _ = reopened.close().await;
    cleanup(&path);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_set_global_without_a_slot_and_writes_to_undeclared_tables() {
    let backend = backend_at(":memory:");
    let no_global = dsh_storage::KvUnitDescriptor {
        name: "specimen".to_string(),
        version: 1,
        tables: vec!["records".to_string()],
        has_global: false,
    };
    let unit = backend
        .kv()
        .expect("kv")
        .open(&no_global)
        .await
        .expect("open");
    let error = unit.set_global(json!({"g": 1})).await.expect_err("reject");
    assert!(
        error.message.contains("declared no global slot"),
        "{}",
        error.message
    );
    let error = unit
        .put_record("undeclared", "k", json!(1))
        .await
        .expect_err("reject");
    assert!(
        error.message.contains("declared no table"),
        "{}",
        error.message
    );
    let snapshot = unit.load_all().await.expect("load");
    assert_eq!(snapshot.global, json!(null));
    let _ = backend.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn drains_a_still_pending_failed_open_during_close() {
    let path = fresh_db_path("pending");
    {
        let first = backend_at(path.to_string_lossy().as_ref());
        let unit = first
            .kv()
            .expect("kv")
            .open(&descriptor())
            .await
            .expect("open");
        let _ = unit.close().await;
        let _ = first.close().await;
    }
    let backend = backend_at(path.to_string_lossy().as_ref());
    let facet = backend.kv().expect("kv").clone();
    let mismatched = dsh_storage::KvUnitDescriptor {
        name: "specimen".to_string(),
        version: 99,
        tables: vec!["records".to_string()],
        has_global: true,
    };
    // Do not await: close must tolerate an in-flight open that will reject.
    // One manual poll runs the open's synchronous prefix (reservation + the
    // spawn of the materialization), mirroring the TS synchronous start.
    let open_future = facet.open(&mismatched);
    futures::pin_mut!(open_future);
    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    assert!(
        open_future.as_mut().poll(&mut cx).is_pending(),
        "open must be in flight"
    );
    let close_task = tokio::spawn(async move { backend.close().await });
    let error = open_future.await.err().expect("version-mismatch");
    assert_eq!(error.code, StorageErrorCode::VersionMismatch);
    close_task
        .await
        .expect("close task")
        .expect("close drains the failed open");
    cleanup(&path);
}

#[tokio::test(flavor = "multi_thread")]
async fn propagates_an_invalid_database_filename_before_opening_sqlite() {
    let path = fresh_db_path("null");
    let backend = backend_at(&format!("{}\0invalid", path.to_string_lossy()));
    let error = backend
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("reject");
    assert!(error.message.contains("invalid path"), "{}", error.message);
    let _ = backend.close().await;
    cleanup(&path);
}

#[tokio::test(flavor = "current_thread")]
async fn registers_on_the_storage_hub_as_backend_sqlite_and_closes_on_dispose() {
    let ctx = Context::root();
    let _hub = Storage::install(&ctx);
    let fiber = ctx.plugin(
        Arc::new(SqliteStoragePlugin {
            config: Config {
                path: ":memory:".to_string(),
                journal_mode: JournalMode::default(),
            },
        }),
        arc(()),
    );
    fiber.settle().await.expect("settle");
    let lifecycle = ctx
        .get_typed::<Arc<SqliteStorageBackend>>(&storage_backend_service_key("sqlite"), false)
        .expect("lifecycle service")
        .as_ref()
        .clone();
    let unit = lifecycle
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    unit.put_record("records", "k", json!({"n": 1}))
        .await
        .expect("put");
    fiber.dispose().await;
    assert!(
        ctx.get_typed::<Arc<Storage>>("storage", false)
            .expect("hub")
            .backend
            .names()
            .is_empty()
    );
    let error = lifecycle
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .err()
        .expect("closed");
    assert_eq!(error.code, StorageErrorCode::Closed);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_an_unparsable_global_slot_with_malformed_medium() {
    let path = fresh_db_path("bad-global");
    {
        let backend = backend_at(path.to_string_lossy().as_ref());
        let unit = backend
            .kv()
            .expect("kv")
            .open(&descriptor())
            .await
            .expect("open");
        unit.set_global(json!({"g": 1})).await.expect("set");
        let _ = backend.close().await;
    }
    {
        let db = rusqlite::Connection::open(&path).expect("open");
        db.execute(
            "UPDATE unit_globals SET value = ?1 WHERE unit = ?2",
            rusqlite::params!["][", "specimen"],
        )
        .expect("corrupt");
    }
    let reopened = backend_at(path.to_string_lossy().as_ref());
    let damaged = reopened
        .kv()
        .expect("kv")
        .open(&descriptor())
        .await
        .expect("open");
    let error = damaged.load_all().await.expect_err("malformed");
    assert_eq!(error.code, StorageErrorCode::MalformedMedium);
    let _ = reopened.close().await;
    cleanup(&path);
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
    let fiber = ctx.plugin(Arc::new(invariant::SqliteStorageInvariantPlugin), arc(()));
    fiber.settle().await.expect("settle");
    fiber.dispose().await;
    let again = ctx.plugin(Arc::new(invariant::SqliteStorageInvariantPlugin), arc(()));
    again.settle().await.expect("settle");
    again.dispose().await;
}
