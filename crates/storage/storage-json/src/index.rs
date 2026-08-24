//! JSON storage backend: one human-readable file per unit under a
//! configured root, published by atomic whole-file rewrite. Rust port of
//! `packages/storage/storage-json/src/index.ts`. Registers as backend
//! `json` on the storage hub.
//!
//! # Deviations
//!
//! - The TS caller-bug throws (double-open, undeclared table/global) fold
//!   into [`StorageError`] with the exact prose (the code discriminants of
//!   plain TS `Error`s do not exist).
//! - Non-ENOENT read failures wrap the io error into the message (the TS
//!   raw errno code is host-specific).
//! - The backend itself doubles as the `storage.backend.json` lifecycle
//!   service (the TS `ctx.provide(storageBackendServiceKey('json'),
//!   backend)`): it implements `cordis::Service` under that name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;
use parking_lot::Mutex;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, Service, arc, downcast};
use dsh_storage::{
    KvFacet, KvUnit, KvUnitDescriptor, Storage, StorageBackend, StorageError, StorageErrorCode,
    closed_error, unit_name_matches,
};

use crate::unit::open_json_unit;

/// Cordis plugin name (TS `name`).
pub const NAME: &str = "storage-json";

/// The hub must exist before the backend can register (TS `inject`).
pub const INJECT: [&str; 1] = ["storage"];

/// Plugin configuration (TS `Config`). `root` has NO default on purpose: a
/// `process.cwd()` fallback would scatter unit files wherever the process
/// happens to start.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    /// Directory holding one `<unit>.json` file per unit.
    pub root: String,
}

/// JSON backend: owns the file-tree root and serves the `kv` facet.
pub struct JsonStorageBackend {
    root: PathBuf,
    open: Arc<Mutex<HashMap<String, Arc<dyn KvUnit>>>>,
    closed: AtomicBool,
    kv_facet: Arc<JsonKvFacet>,
}

struct JsonKvFacet {
    backend: std::sync::Weak<JsonStorageBackend>,
}

/// The backend doubles as its own lifecycle service (TS `apply`'s
/// `ctx.provide(storageBackendServiceKey('json'), backend)`).
impl Service for JsonStorageBackend {
    fn service_name(&self) -> &'static str {
        // The lifecycle key must match storageBackendServiceKey("json").
        "storage.backend.json"
    }
}

impl JsonStorageBackend {
    pub fn new(root: impl Into<String>) -> Arc<Self> {
        let root = PathBuf::from(root.into());
        Arc::new_cyclic(|weak| Self {
            root,
            open: Arc::new(Mutex::new(HashMap::new())),
            closed: AtomicBool::new(false),
            kv_facet: Arc::new(JsonKvFacet {
                backend: weak.clone(),
            }),
        })
    }

    fn assert_open(&self) -> Result<(), StorageError> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(closed_error("json backend"));
        }
        Ok(())
    }
}

fn validate_descriptor(descriptor: &KvUnitDescriptor) -> Result<(), StorageError> {
    if !unit_name_matches(&descriptor.name) {
        return Err(StorageError::new(
            StorageErrorCode::MalformedMedium,
            format!("invalid unit name '{}'", descriptor.name),
        ));
    }
    for table in &descriptor.tables {
        if !unit_name_matches(table) {
            return Err(StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!("invalid table name '{table}' in unit '{}'", descriptor.name),
            ));
        }
    }
    Ok(())
}

#[async_trait]
impl KvFacet for JsonKvFacet {
    async fn open(&self, descriptor: &KvUnitDescriptor) -> Result<Arc<dyn KvUnit>, StorageError> {
        let Some(backend) = self.backend.upgrade() else {
            return Err(closed_error("json backend"));
        };
        backend.assert_open()?;
        validate_descriptor(descriptor)?;
        {
            let open = backend.open.lock();
            if open.contains_key(&descriptor.name) {
                return Err(StorageError::new(
                    StorageErrorCode::Closed,
                    format!(
                        "unit '{}' is already open; a unit has exactly one live handle",
                        descriptor.name
                    ),
                ));
            }
        }
        // Ensure the root exists (mode 700 best-effort on unix).
        if let Err(error) = tokio::fs::create_dir_all(&backend.root).await {
            return Err(StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!("json backend: failed to create root: {error}"),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                tokio::fs::set_permissions(&backend.root, std::fs::Permissions::from_mode(0o700))
                    .await;
        }
        let path = backend.root.join(format!("{}.json", descriptor.name));
        let open_slots = Arc::clone(&backend.open);
        let descriptor_name = descriptor.name.clone();
        let unit = open_json_unit(
            descriptor.clone(),
            path,
            Arc::new(move || {
                open_slots.lock().remove(&descriptor_name);
            }),
        )
        .await?;
        // The backend closed while this open was in flight: do not hand out
        // a live unit past close().
        if backend.closed.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = unit.close().await;
            return Err(closed_error("json backend"));
        }
        let unit: Arc<dyn KvUnit> = Arc::new(unit);
        backend
            .open
            .lock()
            .insert(descriptor.name.clone(), unit.clone());
        Ok(unit)
    }
}

#[async_trait]
impl StorageBackend for JsonStorageBackend {
    fn kv(&self) -> Option<Arc<dyn KvFacet>> {
        Some(self.kv_facet.clone())
    }

    async fn close(&self) -> Result<(), StorageError> {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        let units: Vec<Arc<dyn KvUnit>> = self.open.lock().values().cloned().collect();
        for unit in units {
            let _ = unit.close().await;
        }
        Ok(())
    }
}

/// The root directory (diagnostic surface).
pub fn root_of(backend: &JsonStorageBackend) -> &Path {
    &backend.root
}

/// Register the `json` backend on the storage hub (TS `apply`).
pub fn apply(ctx: &Context, config: Config) -> Result<cordis::Disposer, String> {
    let hub = ctx
        .get_typed::<Arc<Storage>>("storage", false)
        .ok_or_else(|| "the storage hub is not configured".to_string())?
        .as_ref()
        .clone();
    let backend = JsonStorageBackend::new(config.root);
    // The lifecycle service the domain form providers inject.
    ctx.register_service(backend.clone());
    let unregister = hub
        .backend
        .register("json", backend.clone())
        .map_err(|error| error.message)?;
    let dispose_backend = backend.clone();
    let disposer = ctx.effect(
        "storage-json.register()",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                let unregister = unregister.clone();
                let backend = dispose_backend.clone();
                Box::pin(async move {
                    let _ = unregister().await;
                    let _ = backend.close().await;
                })
            }))
        }),
    );
    Ok(disposer)
}

/// The Cordis plugin form.
pub struct JsonStoragePlugin {
    pub config: Config,
}

#[async_trait]
impl Plugin for JsonStoragePlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = downcast::<Config>(&config)
            .cloned()
            .or_else(|| {
                serde_json::from_value(downcast::<serde_json::Value>(&config)?.clone()).ok()
            })
            .unwrap_or_else(|| self.config.clone());
        apply(ctx, config)
            .map(|_| ())
            .map_err(|error| PluginError::new(arc(error)))
    }
}
