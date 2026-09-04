use std::path::{Path, PathBuf};
use std::sync::Arc;

use dsh_storage::{KvLayout, KvUnitDescriptor, StorageBackend};
use dsh_storage_json::JsonStorageBackend;
use serde_json::json;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dsh-storage-json-per-record-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).expect("create temporary storage root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn descriptor() -> KvUnitDescriptor {
    KvUnitDescriptor {
        name: "cache".to_string(),
        version: 5,
        tables: vec!["sessions".to_string()],
        has_global: false,
        layout: KvLayout::PerRecord,
        compatible_versions: vec![3, 4],
    }
}

#[tokio::test]
async fn per_record_write_reopens_from_one_versioned_document() {
    let root = TempRoot::new();
    let backend = JsonStorageBackend::new(root.path().to_string_lossy());
    let unit = backend
        .kv()
        .expect("json backend has kv")
        .open(&descriptor())
        .await
        .expect("open per-record unit");

    unit.put_record("sessions", "session-one", json!({"title": "kept"}))
        .await
        .expect("write one record");

    let record_path = root
        .path()
        .join("cache")
        .join("sessions")
        .join("session-one.json");
    let document: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&record_path).expect("per-record document exists"),
    )
    .expect("record document is json");
    assert_eq!(document, json!({"version": 5, "record": {"title": "kept"}}));

    unit.close().await.expect("close first unit");
    let reopened = backend
        .kv()
        .expect("json backend has kv")
        .open(&descriptor())
        .await
        .expect("reopen per-record unit");
    let snapshot = reopened.load_all().await.expect("load per-record tree");
    assert_eq!(
        snapshot.tables["sessions"]["session-one"],
        json!({"title": "kept"})
    );
}

#[tokio::test]
async fn malformed_and_stale_record_documents_remain_explicit_and_untouched() {
    let root = TempRoot::new();
    let table_dir = root.path().join("cache").join("sessions");
    std::fs::create_dir_all(&table_dir).expect("create per-record table");
    std::fs::write(
        table_dir.join("valid.json"),
        r#"{"version":4,"record":{"ok":true}}"#,
    )
    .expect("write compatible record");
    std::fs::write(table_dir.join("malformed.json"), "not json").expect("write malformed record");
    std::fs::write(
        table_dir.join("stale.json"),
        r#"{"version":2,"record":{"old":true}}"#,
    )
    .expect("write stale record");

    let backend = JsonStorageBackend::new(root.path().to_string_lossy());
    let unit = backend
        .kv()
        .expect("json backend has kv")
        .open(&descriptor())
        .await
        .expect("open per-record unit");
    let snapshot = unit
        .load_all()
        .await
        .expect("foreign records must not brick the unit");

    assert_eq!(snapshot.tables["sessions"].len(), 1);
    assert_eq!(snapshot.tables["sessions"]["valid"], json!({"ok": true}));
    assert_eq!(snapshot.invalid.len(), 2);
    assert_eq!(
        std::fs::read_to_string(table_dir.join("malformed.json")).unwrap(),
        "not json"
    );
    assert!(table_dir.join("stale.json").exists());
}

#[tokio::test]
async fn compatible_legacy_unit_bootstraps_current_record_documents() {
    let root = TempRoot::new();
    let legacy_path = root.path().join("cache.json");
    let legacy = r#"{
  "unit": {"name": "cache", "version": 4},
  "global": null,
  "tables": {"sessions": {"legacy": {"title": "old"}}}
}
"#;
    std::fs::write(&legacy_path, legacy).expect("write legacy unit");

    let backend = JsonStorageBackend::new(root.path().to_string_lossy());
    let unit = backend
        .kv()
        .expect("json backend has kv")
        .open(&descriptor())
        .await
        .expect("open per-record unit");
    let snapshot = unit.load_all().await.expect("bootstrap legacy unit");

    assert_eq!(
        snapshot.tables["sessions"]["legacy"],
        json!({"title": "old"})
    );
    let migrated: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            root.path()
                .join("cache")
                .join("sessions")
                .join("legacy.json"),
        )
        .expect("current per-record document exists"),
    )
    .expect("migrated record document is json");
    assert_eq!(migrated, json!({"version": 5, "record": {"title": "old"}}));
    assert_eq!(
        std::fs::read_to_string(legacy_path).expect("legacy unit remains"),
        legacy
    );
}

#[tokio::test]
async fn per_record_writes_reject_unsafe_path_keys() {
    let root = TempRoot::new();
    let backend = JsonStorageBackend::new(root.path().to_string_lossy());
    let unit = backend
        .kv()
        .expect("json backend has kv")
        .open(&descriptor())
        .await
        .expect("open per-record unit");

    let error = unit
        .put_record("sessions", "../escape", json!({"bad": true}))
        .await
        .expect_err("unsafe path key must reject");

    assert!(error.message.contains("not path-safe"));
    assert!(!root.path().join("cache").join("escape.json").exists());
}

#[test]
fn per_record_close_waits_for_an_in_flight_write() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("test runtime");

    runtime.block_on(async {
        let root = TempRoot::new();
        let backend: Arc<dyn StorageBackend> =
            JsonStorageBackend::new(root.path().to_string_lossy().to_string());
        let unit = backend.kv().unwrap().open(&descriptor()).await.unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();

        let write_unit = Arc::clone(&unit);
        let write = tokio::spawn(async move {
            write_unit
                .put_record("sessions", "pending", json!({"value": "durable"}))
                .await
        });
        tokio::task::yield_now().await;

        let close_unit = Arc::clone(&unit);
        let mut close = tokio::spawn(async move { close_unit.close().await });
        let close_waited = tokio::time::timeout(std::time::Duration::from_millis(50), &mut close)
            .await
            .is_err();

        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        write.await.unwrap().unwrap();
        if close_waited {
            close.await.unwrap().unwrap();
        }
        backend.close().await.unwrap();

        assert!(
            close_waited,
            "close returned before the queued write drained"
        );
    });
}

#[tokio::test]
async fn backup_record_moves_document_out_of_readable_set() {
    let root = TempRoot::new();
    let path = root
        .path()
        .join("cache")
        .join("sessions")
        .join("broken.json");
    std::fs::create_dir_all(path.parent().expect("record parent"))
        .expect("create per-record table");
    std::fs::write(&path, "broken bytes").expect("write broken record");
    let backend = JsonStorageBackend::new(root.path().to_string_lossy());
    let unit = backend
        .kv()
        .expect("json backend has kv")
        .open(&descriptor())
        .await
        .expect("open per-record unit");

    let moved = unit
        .backup_record("sessions", "broken")
        .await
        .expect("backup malformed record")
        .expect("per-record backend supports backup");

    let moved = PathBuf::from(moved);
    assert!(!path.exists());
    assert_eq!(
        std::fs::read_to_string(&moved).expect("backup exists"),
        "broken bytes"
    );
    assert!(
        moved
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("broken.json.bak."))
    );
    std::fs::write(&path, "second broken document").unwrap();
    let second = unit
        .backup_record("sessions", "broken")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(PathBuf::from(&second), moved);
    assert_eq!(std::fs::read_to_string(moved).unwrap(), "broken bytes");
    assert_eq!(
        std::fs::read_to_string(second).unwrap(),
        "second broken document"
    );
}

#[tokio::test]
async fn deleted_migrated_records_do_not_return_from_legacy_on_restart() {
    let root = TempRoot::new();
    std::fs::write(
        root.path().join("cache.json"),
        r#"{"unit":{"name":"cache","version":3},"tables":{"sessions":{"old":{"title":"legacy"}}}}"#,
    )
    .unwrap();
    let backend = JsonStorageBackend::new(root.path().to_string_lossy());
    let unit = backend.kv().unwrap().open(&descriptor()).await.unwrap();
    assert_eq!(unit.load_all().await.unwrap().tables["sessions"].len(), 1);
    unit.delete_record("sessions", "old").await.unwrap();
    unit.close().await.unwrap();
    let reopened = backend.kv().unwrap().open(&descriptor()).await.unwrap();
    assert!(reopened.load_all().await.unwrap().tables["sessions"].is_empty());
}

#[tokio::test]
async fn malformed_global_cannot_be_read_as_default() {
    let root = TempRoot::new();
    std::fs::create_dir_all(root.path().join("cache")).unwrap();
    std::fs::write(
        root.path().join("cache/global.json"),
        r#"{"version":6,"record":{"protected":true}}"#,
    )
    .unwrap();
    let backend = JsonStorageBackend::new(root.path().to_string_lossy());
    let mut spec = descriptor();
    spec.has_global = true;
    let unit = backend.kv().unwrap().open(&spec).await.unwrap();
    assert_eq!(
        unit.load_all().await.unwrap_err().code,
        dsh_storage::StorageErrorCode::VersionMismatch
    );
}
