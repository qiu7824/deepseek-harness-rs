use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use cordis::Context;
use dsh_storage::{Storage, StorageBackend};
use dsh_storage_domain::{
    DomainFacility, DomainFacilityConfig, DomainSpec, define_domain, domain_table,
};
use dsh_storage_json::JsonStorageBackend;
use dsh_storage_test_support::{MemoryMediaPool, MemoryStorageBackend};
use indexmap::indexmap;
use serde_json::{Value, json};

struct TempRoot(PathBuf);
impl TempRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("dsh-domain-order-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn specification() -> DomainSpec {
    define_domain(
        "ordered",
        1,
        None,
        indexmap! {"items".into()=>domain_table(Arc::new(|_:&Value|Ok(())))},
    )
    .unwrap()
}

fn fixture(backend: Arc<dyn StorageBackend>) -> (Context, Arc<DomainFacility>) {
    let ctx = Context::root();
    let storage = Storage::install(&ctx);
    storage.backend.register("fixture", backend).unwrap();
    let facility = DomainFacility::install(
        &ctx,
        DomainFacilityConfig {
            backend: "fixture".into(),
            routes: Default::default(),
        },
    )
    .unwrap();
    (ctx, facility)
}

fn file_keys(path: &std::path::Path) -> Vec<String> {
    let document: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    document["tables"]["items"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

#[tokio::test]
async fn json_domain_preserves_creation_order_through_edits_deletion_and_reopen() {
    let root = TempRoot::new();
    let backend = JsonStorageBackend::new(root.0.to_string_lossy());
    let (ctx, facility) = fixture(backend.clone());
    let spec = specification();
    let domain = facility.open(&spec).await.unwrap();
    let table = domain.table("items");
    for key in ["zulu", "alpha", "middle", "中文"] {
        table.put(key, json!({"value":key})).await.unwrap();
    }
    assert_eq!(table.keys(), ["zulu", "alpha", "middle", "中文"]);
    table
        .put("alpha", json!({"value":"overwritten"}))
        .await
        .unwrap();
    table
        .update("middle", Arc::new(|_| json!({"value":"edited"})))
        .await
        .unwrap();
    assert_eq!(table.keys(), ["zulu", "alpha", "middle", "中文"]);
    assert!(table.delete("alpha").await.unwrap());
    assert!(!table.delete("absent").await.unwrap());
    assert_eq!(table.keys(), ["zulu", "middle", "中文"]);
    table
        .put("alpha", json!({"value":"recreated"}))
        .await
        .unwrap();
    let expected = ["zulu", "middle", "中文", "alpha"];
    assert_eq!(table.keys(), expected);
    assert_eq!(
        table
            .entries()
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(file_keys(&root.0.join("ordered.json")), expected);
    domain.close().await;
    backend.close().await.unwrap();
    ctx.fiber.dispose().await;

    // A fresh backend and context rule out retained domain state hiding a
    // loss of order when the JSON snapshot is reconstructed.
    let reopened_backend = JsonStorageBackend::new(root.0.to_string_lossy());
    let (ctx, facility) = fixture(reopened_backend.clone());
    let reopened = facility.open(&spec).await.unwrap();
    let table = reopened.table("items");
    assert_eq!(table.keys(), expected);
    assert_eq!(table.get("middle"), Some(json!({"value":"edited"})));
    table
        .put("zulu", json!({"value":"changed after reopen"}))
        .await
        .unwrap();
    table.put("last", json!({"value":"new"})).await.unwrap();
    assert_eq!(table.keys(), ["zulu", "middle", "中文", "alpha", "last"]);
    assert_eq!(file_keys(&root.0.join("ordered.json")), table.keys());
    reopened.close().await;
    reopened_backend.close().await.unwrap();
    ctx.fiber.dispose().await;
}

#[tokio::test]
async fn failed_json_delete_restores_the_original_backend_position() {
    let root = TempRoot::new();
    let backend = JsonStorageBackend::new(root.0.to_string_lossy());
    let (ctx, facility) = fixture(backend.clone());
    let spec = specification();
    let domain = facility.open(&spec).await.unwrap();
    let table = domain.table("items");
    for key in ["zulu", "alpha", "middle"] {
        table.put(key, json!(key)).await.unwrap();
    }
    let path = root.0.join("ordered.json");
    let original = std::fs::read(&path).unwrap();
    let saved = root.0.join("ordered.saved.json");
    std::fs::rename(&path, &saved).unwrap();
    std::fs::create_dir(&path).unwrap();
    let result = table.delete("alpha").await;
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(saved, &path).unwrap();
    assert!(
        result.is_err(),
        "directory target must prevent atomic file replacement"
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(table.keys(), ["zulu", "alpha", "middle"]);
    assert_eq!(table.get("alpha"), Some(json!("alpha")));
    // The next publish exposes the backend's rollback order, not merely
    // the domain's untouched memory snapshot.
    table.put("last", json!("last")).await.unwrap();
    assert_eq!(file_keys(&path), ["zulu", "alpha", "middle", "last"]);
    domain.close().await;
    let reopened = facility.open(&spec).await.unwrap();
    assert_eq!(
        reopened.table("items").keys(),
        ["zulu", "alpha", "middle", "last"]
    );
    reopened.close().await;
    backend.close().await.unwrap();
    ctx.fiber.dispose().await;
}

#[tokio::test]
async fn unordered_backends_reopen_deterministically_and_new_records_append() {
    let backend = MemoryStorageBackend::with_shared_pool(Arc::new(MemoryMediaPool::new()));
    let (ctx, facility) = fixture(backend.clone());
    let spec = specification();
    let domain = facility.open(&spec).await.unwrap();
    let table = domain.table("items");
    for key in ["zulu", "alpha", "middle"] {
        table.put(key, json!(key)).await.unwrap();
    }
    assert_eq!(table.keys(), ["zulu", "alpha", "middle"]);
    domain.close().await;
    let domain = facility.open(&spec).await.unwrap();
    let table = domain.table("items");
    assert_eq!(table.keys(), ["alpha", "middle", "zulu"]);
    table.put("first", json!("last inserted")).await.unwrap();
    table.put("middle", json!("overwritten")).await.unwrap();
    assert_eq!(table.keys(), ["alpha", "middle", "zulu", "first"]);
    domain.close().await;
    backend.close().await.unwrap();
    ctx.fiber.dispose().await;
}
