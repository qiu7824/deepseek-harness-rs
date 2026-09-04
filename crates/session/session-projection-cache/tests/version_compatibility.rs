use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use cordis::Context;
use dsh_session_projection_cache::spec::{CheckpointRecord, projection_cache_domain_spec};
use dsh_storage::{Storage, StorageBackend};
use dsh_storage_domain::{DomainFacility, DomainFacilityConfig};
use dsh_storage_json::JsonStorageBackend;
use serde_json::json;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(version: u64) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dsh-projection-v{version}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn legacy_v3_v4_and_current_v5_preserve_title_and_reopen_without_bootstrapping() {
    for version in [3, 4, 5] {
        let root = TempRoot::new(version);
        let record = json!({"identity":{"createdAt":7,"cwd":"workspace"},"rows":{"title":{"ver":1,"seq":4,"val":"preserved title"}}});
        if version < 5 {
            std::fs::write(root.0.join("session_projcache.json"), serde_json::to_vec(&json!({
                "unit":{"name":"session_projcache","version":version},"global":null,"tables":{"sessions":{"session":record}}
            })).unwrap()).unwrap();
        } else {
            std::fs::create_dir_all(root.0.join("session_projcache/sessions")).unwrap();
            std::fs::write(
                root.0.join("session_projcache/sessions/session.json"),
                serde_json::to_vec(&json!({"version":version,"record":record})).unwrap(),
            )
            .unwrap();
        }
        let ctx = Context::root();
        let storage = Storage::install(&ctx);
        let backend = JsonStorageBackend::new(root.0.to_string_lossy());
        let _registration = storage.backend.register("json", backend.clone()).unwrap();
        let facility = DomainFacility::install(
            &ctx,
            DomainFacilityConfig {
                backend: "json".to_string(),
                routes: Default::default(),
            },
        )
        .unwrap();
        let spec = projection_cache_domain_spec();
        let domain = facility.open(&spec).await.unwrap();
        let loaded: CheckpointRecord =
            serde_json::from_value(domain.table("sessions").get("session").unwrap()).unwrap();
        assert_eq!(loaded.rows["title"].val, "preserved title");
        assert!(!loaded.identity.is_seeded);
        assert_eq!(loaded.identity.inherited_event_count, 0);
        let document: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.0.join("session_projcache/sessions/session.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(document["version"], 5);
        domain.close().await;
        std::fs::remove_file(root.0.join("session_projcache/sessions/session.json")).unwrap();
        let reopened = facility.open(&spec).await.unwrap();
        assert!(reopened.table("sessions").is_empty());
        reopened.close().await;
        backend.close().await.unwrap();
        ctx.fiber.dispose().await;
    }
}
