//! Storage hub and backend registry behaviors. Rust port of the core
//! `packages/storage/storage/tests/registry.spec.ts` behaviors.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::Context;
use dsh_storage::{
    BackendRegistry, KvFacet, KvUnit, KvUnitDescriptor, KvUnitSnapshot, Storage,
    StorageBackend, StorageError, StorageErrorCode, storage_backend_service_key,
};
use serde_json::Value as JsonValue;

struct FakeBackend;

#[async_trait]
impl StorageBackend for FakeBackend {
    fn kv(&self) -> Option<Arc<dyn KvFacet>> {
        None
    }

    async fn close(&self) -> Result<(), StorageError> {
        Ok(())
    }
}

fn fake_backend() -> Arc<dyn StorageBackend> {
    Arc::new(FakeBackend)
}

#[test]
fn registry_registers_resolves_and_disposes_names() {
    let registry = BackendRegistry::new();
    let backend = fake_backend();
    let dispose = registry.register("json", backend.clone()).expect("register");
    assert!(std::sync::Arc::ptr_eq(&registry.get("json").expect("get"), &backend));
    assert_eq!(registry.names(), vec!["json".to_string()]);
    let _ = futures::executor::block_on(dispose());
    assert_eq!(registry.names(), Vec::<String>::new());
    let error = registry.get("json").err().expect("backend-not-found");
    assert_eq!(error.code, StorageErrorCode::BackendNotFound);
}

#[test]
fn registry_rejects_duplicate_names() {
    let registry = BackendRegistry::new();
    registry.register("json", fake_backend()).expect("register");
    let error = registry.register("json", fake_backend()).err().expect("duplicate");
    assert_eq!(error.code, StorageErrorCode::DuplicateBackend);
}

#[test]
fn derives_stable_lifecycle_service_keys_for_named_backends() {
    assert_eq!(storage_backend_service_key("json"), "storage.backend.json");
    assert_eq!(storage_backend_service_key("tenant-a"), "storage.backend.tenant-a");
}

#[tokio::test(flavor = "current_thread")]
async fn hub_mounts_on_the_context_and_exposes_registry_plus_form_mounting() {
    let ctx = Context::root();
    let hub = Storage::install(&ctx);
    let resolved = ctx
        .get_typed::<Arc<Storage>>("storage", false)
        .expect("storage service")
        .as_ref()
        .clone();
    assert!(std::sync::Arc::ptr_eq(&hub, &resolved));

    let facility = cordis::arc(("marker", 42_i64));
    let dispose = hub.mount("domain", facility.clone()).expect("mount");
    let form = hub.form("domain").expect("form");
    assert!(std::sync::Arc::ptr_eq(&form, &facility));
    let duplicate = hub.mount("domain", facility.clone()).err().expect("duplicate-mount");
    assert_eq!(duplicate.code, StorageErrorCode::DuplicateMount);
    let _ = futures::executor::block_on(dispose());
    let missing = hub.form("domain").err().expect("form-not-mounted");
    assert_eq!(missing.code, StorageErrorCode::FormNotMounted);
}

#[tokio::test(flavor = "current_thread")]
async fn ignores_a_stale_disposer_after_dispose_and_remount_or_reregister() {
    let ctx = Context::root();
    let hub = Storage::install(&ctx);
    let first = cordis::arc(("first", true));
    let second = cordis::arc(("second", true));
    let stale_mount = hub.mount("domain", first).expect("mount");
    let _ = futures::executor::block_on(stale_mount());
    hub.mount("domain", second.clone()).expect("remount");
    let _ = futures::executor::block_on(stale_mount());
    let form = hub.form("domain").expect("form");
    assert!(std::sync::Arc::ptr_eq(&form, &second));

    let backend_a = fake_backend();
    let backend_b = fake_backend();
    let stale_register = hub.backend.register("json", backend_a).expect("register");
    let _ = futures::executor::block_on(stale_register());
    hub.backend.register("json", backend_b.clone()).expect("reregister");
    let _ = futures::executor::block_on(stale_register());
    let current = hub.backend.get("json").expect("get");
    assert!(std::sync::Arc::ptr_eq(&current, &backend_b));
}

/// A backend WITH a kv facet so the domain form resolution path can be
/// exercised end to end.
struct KvFakeBackend;

#[async_trait]
impl StorageBackend for KvFakeBackend {
    fn kv(&self) -> Option<Arc<dyn KvFacet>> {
        Some(Arc::new(KvFakeFacet))
    }

    async fn close(&self) -> Result<(), StorageError> {
        Ok(())
    }
}

struct KvFakeFacet;

#[async_trait]
impl KvFacet for KvFakeFacet {
    async fn open(&self, descriptor: &KvUnitDescriptor) -> Result<Arc<dyn KvUnit>, StorageError> {
        Ok(Arc::new(KvFakeUnit {
            descriptor: descriptor.clone(),
        }))
    }
}

struct KvFakeUnit {
    descriptor: KvUnitDescriptor,
}

#[async_trait]
impl KvUnit for KvFakeUnit {
    async fn load_all(&self) -> Result<KvUnitSnapshot, StorageError> {
        Ok(KvUnitSnapshot {
            tables: self
                .descriptor
                .tables
                .iter()
                .map(|table| (table.clone(), std::collections::HashMap::new()))
                .collect(),
            global: JsonValue::Null,
        })
    }

    async fn put_record(
        &self,
        _table: &str,
        _key: &str,
        _value: JsonValue,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn delete_record(&self, _table: &str, _key: &str) -> Result<(), StorageError> {
        Ok(())
    }

    async fn set_global(&self, _value: JsonValue) -> Result<(), StorageError> {
        Ok(())
    }

    async fn close(&self) -> Result<(), StorageError> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn backend_without_a_kv_facet_fails_domain_open_loud() {
    let ctx = Context::root();
    let hub = Storage::install(&ctx);
    hub.backend.register("bare", fake_backend()).expect("register");
    let facility = dsh_storage_domain::DomainFacility::install(
        &ctx,
        dsh_storage_domain::DomainFacilityConfig {
            backend: "bare".to_string(),
            routes: Default::default(),
        },
    )
    .expect("facility");
    let spec = dsh_storage_domain::define_domain(
        "needs_kv",
        1,
        None,
        indexmap::IndexMap::new(),
    )
    .expect("spec");
    let error = facility.open(&spec).await.err().expect("facet-unsupported");
    assert!(error.contains("has no kv facet"), "{error}");
}
