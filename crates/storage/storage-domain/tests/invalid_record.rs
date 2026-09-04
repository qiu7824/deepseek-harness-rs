use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use cordis::Context;
use dsh_storage::{Storage, StorageBackend};
use dsh_storage_domain::{
    DomainFacility, DomainFacilityConfig, InvalidRecordPolicy, KvLayout,
    define_domain_with_options, domain_table,
};
use dsh_storage_json::JsonStorageBackend;
use indexmap::IndexMap;
use serde_json::json;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dsh-storage-domain-invalid-record-{}-{nonce}",
            std::process::id()
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

#[tokio::test]
async fn backup_and_skip_keeps_domain_open_with_valid_records() {
    let root = TempRoot::new();
    let table_dir = root.path().join("derived").join("records");
    std::fs::create_dir_all(&table_dir).expect("create per-record table");
    std::fs::write(
        table_dir.join("good.json"),
        r#"{"version":5,"record":{"valid":true}}"#,
    )
    .expect("write valid record");
    std::fs::write(
        table_dir.join("bad.json"),
        r#"{"version":5,"record":{"valid":false}}"#,
    )
    .expect("write schema-invalid record");

    let ctx = Context::root();
    let storage = Storage::install(&ctx);
    let backend = JsonStorageBackend::new(root.path().to_string_lossy());
    let _unregister = storage
        .backend
        .register("json", backend.clone())
        .expect("register json backend");
    let facility = DomainFacility::install(
        &ctx,
        DomainFacilityConfig {
            backend: "json".to_string(),
            routes: Default::default(),
        },
    )
    .expect("install domain facility");
    let schema = Arc::new(|value: &serde_json::Value| {
        if value.get("valid").and_then(serde_json::Value::as_bool) == Some(true) {
            Ok(())
        } else {
            Err("valid must be true".to_string())
        }
    });
    let spec = define_domain_with_options(
        "derived",
        5,
        KvLayout::PerRecord,
        vec![3, 4],
        InvalidRecordPolicy::BackupAndSkip,
        None,
        IndexMap::from([("records".to_string(), domain_table(schema))]),
    )
    .expect("valid disposable domain spec");

    let domain = facility
        .open(&spec)
        .await
        .expect("invalid disposable record must not brick domain");

    let records = domain.table("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records.get("good"), Some(json!({"valid": true})));
    assert_eq!(records.get("bad"), None);
    assert!(!table_dir.join("bad.json").exists());
    let backups: Vec<_> = std::fs::read_dir(&table_dir)
        .expect("read record directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("bad.json.bak."))
        })
        .collect();
    assert_eq!(backups.len(), 1);

    domain.close().await;
    backend.close().await.expect("close backend");
    ctx.fiber.dispose().await;
}

async fn check_invalid_document(policy: InvalidRecordPolicy, legacy: bool, bytes: &[u8]) {
    let root = TempRoot::new();
    let path = if legacy {
        root.path().join("derived.json")
    } else {
        root.path().join("derived/records/bad.json")
    };
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let ctx = Context::root();
    let storage = Storage::install(&ctx);
    let backend = JsonStorageBackend::new(root.path().to_string_lossy());
    let _unregister = storage.backend.register("json", backend.clone()).unwrap();
    let facility = DomainFacility::install(
        &ctx,
        DomainFacilityConfig {
            backend: "json".to_string(),
            routes: Default::default(),
        },
    )
    .unwrap();
    let spec = define_domain_with_options(
        "derived",
        5,
        KvLayout::PerRecord,
        vec![3, 4],
        policy,
        None,
        IndexMap::from([("records".to_string(), domain_table(Arc::new(|_| Ok(()))))]),
    )
    .unwrap();
    let outcome = facility.open(&spec).await;
    if policy == InvalidRecordPolicy::FailLoud {
        assert!(outcome.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    } else {
        let domain = outcome.expect("disposable cache recovers after preserving bytes");
        assert!(domain.table("records").is_empty());
        assert!(!path.exists());
        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".bak."))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(backups[0].path()).unwrap(), bytes);
        domain.close().await;
        let reopened = facility
            .open(&spec)
            .await
            .expect("recovery survives reopen");
        assert!(reopened.table("records").is_empty());
        reopened.close().await;
    }
    backend.close().await.unwrap();
    ctx.fiber.dispose().await;
}

#[tokio::test]
async fn raw_corruption_and_future_records_obey_domain_policy() {
    for policy in [
        InvalidRecordPolicy::FailLoud,
        InvalidRecordPolicy::BackupAndSkip,
    ] {
        for bytes in [
            b"not json".as_slice(),
            b"\xff\xfe".as_slice(),
            br#"{"version":6,"record":{"protected":true}}"#.as_slice(),
        ] {
            check_invalid_document(policy, false, bytes).await;
        }
    }
}

#[tokio::test]
async fn incompatible_legacy_unit_is_preserved_before_cache_recovery() {
    for policy in [
        InvalidRecordPolicy::FailLoud,
        InvalidRecordPolicy::BackupAndSkip,
    ] {
        for bytes in [
            b"not json".as_slice(),
            br#"{"unit":{"name":"derived","version":6},"tables":{"records":{"protected":{}}}}"#
                .as_slice(),
        ] {
            check_invalid_document(policy, true, bytes).await;
        }
    }
}
