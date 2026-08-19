//! SQLite storage backend for the storage hub: one database file hosts
//! every routed unit, document-per-row (`key TEXT` / `value TEXT` JSON).
//! Rust port of `packages/storage/storage-sqlite/src/index.ts`. Registers
//! as backend `sqlite`; the disposer unregisters first, then closes the
//! medium.
//!
//! # Deviations
//!
//! - The TS constructor's plain-`Error` throws (invalid names, double-open)
//!   fold into [`StorageError`] with the exact prose (documented collapse).
//! - The eager constructor open becomes a sticky lazy open on the first
//!   primitive (`OnceCell` caches the `Err` too, matching the TS sticky
//!   rejected `ready` promise).
//! - `rusqlite::Connection` is `!Sync`; the shared handle is
//!   `Arc<parking_lot::Mutex<Connection>>` and every primitive runs
//!   synchronously under the lock (the backend performs no async I/O
//!   beyond the open sequence, which runs on `spawn_blocking`).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use tokio::sync::{OnceCell, oneshot};

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, Service, arc, downcast};
use dsh_storage::{
    KvFacet, KvUnit, KvUnitDescriptor, Storage, StorageBackend, StorageError, StorageErrorCode,
    closed_error, storage_backend_service_key, unit_name_matches,
};

use crate::schema::{JournalMode, open_database, record_table_name};
use crate::unit::SqliteKvUnit;

/// Cordis plugin name (TS `name`).
pub const NAME: &str = "storage-sqlite";

/// The backend registers on the storage hub (TS `inject`).
pub const INJECT: [&str; 1] = ["storage"];

/// Plugin configuration (TS `Config`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    /// Filesystem path to the SQLite database file, or `:memory:` for an
    /// in-process database (tests).
    pub path: String,
    /// SQLite `journal_mode` pragma (default `wal`).
    #[serde(default)]
    pub journal_mode: JournalMode,
}

/// One unit-name slot: a live unit or a still-materializing open.
enum Slot {
    Open(Arc<dyn KvUnit>),
    Pending(oneshot::Receiver<Result<Arc<dyn KvUnit>, StorageError>>),
}

/// SQLite storage backend: owns one database connection and the open-unit
/// table.
pub struct SqliteStorageBackend {
    ready: OnceCell<Result<Arc<Mutex<Connection>>, StorageError>>,
    config: Config,
    units: Arc<Mutex<HashMap<String, Slot>>>,
    closing: AtomicBool,
    kv_facet: Arc<SqliteKvFacet>,
}

struct SqliteKvFacet {
    backend: std::sync::Weak<SqliteStorageBackend>,
}

/// The backend doubles as its own lifecycle service (TS `apply`'s
/// `ctx.provide(storageBackendServiceKey('sqlite'), backend)`).
impl Service for SqliteStorageBackend {
    fn service_name(&self) -> &'static str {
        "storage.backend.sqlite"
    }
}

impl SqliteStorageBackend {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            ready: OnceCell::new(),
            config,
            units: Arc::new(Mutex::new(HashMap::new())),
            closing: AtomicBool::new(false),
            kv_facet: Arc::new(SqliteKvFacet {
                backend: weak.clone(),
            }),
        })
    }

    /// The sticky database handle: an open failure is cached (the TS
    /// rejected `ready` promise).
    async fn database(&self) -> Result<Arc<Mutex<Connection>>, StorageError> {
        self.ready
            .get_or_init(|| {
                let path = self.config.path.clone();
                let journal_mode = self.config.journal_mode;
                async move {
                    tokio::task::spawn_blocking(move || open_database(&path, journal_mode))
                        .await
                        .map_err(|join| {
                            StorageError::new(
                                StorageErrorCode::MalformedMedium,
                                format!("sqlite backend: open task failed: {join}"),
                            )
                        })?
                        .map(|db| Arc::new(Mutex::new(db)))
                }
            })
            .await
            .clone()
    }

    fn assert_open(&self) -> Result<(), StorageError> {
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(closed_error("sqlite storage backend"));
        }
        Ok(())
    }

    async fn materialize(
        self: &Arc<Self>,
        descriptor: KvUnitDescriptor,
    ) -> Result<Arc<dyn KvUnit>, StorageError> {
        let db = self.database().await?;
        let unit_row = tokio::task::spawn_blocking({
            let db = db.clone();
            let name = descriptor.name.clone();
            move || -> Result<Option<i64>, StorageError> {
                let db = db.lock();
                let mut statement = db
                    .prepare_cached("SELECT version FROM units WHERE name = ?1")
                    .map_err(|error| {
                        StorageError::new(
                            StorageErrorCode::MalformedMedium,
                            format!("sqlite backend: {error}"),
                        )
                    })?;
                let version = statement
                    .query_row([name.as_str()], |row| row.get::<_, i64>(0))
                    .optional()
                    .map_err(|error| {
                        StorageError::new(
                            StorageErrorCode::MalformedMedium,
                            format!("sqlite backend: {error}"),
                        )
                    })?;
                Ok(version)
            }
        })
        .await
        .map_err(|join| {
            StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!("sqlite backend: query task failed: {join}"),
            )
        })??;
        match unit_row {
            None => {
                let name = descriptor.name.clone();
                let version = descriptor.version;
                let db_for_insert = db.clone();
                tokio::task::spawn_blocking(move || {
                    db_for_insert
                        .lock()
                        .execute(
                            "INSERT INTO units (name, version) VALUES (?1, ?2)",
                            rusqlite::params![name.as_str(), version as i64],
                        )
                        .map_err(|error| {
                            StorageError::new(
                                StorageErrorCode::MalformedMedium,
                                format!("sqlite backend: {error}"),
                            )
                        })
                })
                .await
                .map_err(|join| {
                    StorageError::new(
                        StorageErrorCode::MalformedMedium,
                        format!("sqlite backend: insert task failed: {join}"),
                    )
                })??;
            }
            Some(stamped) if stamped != descriptor.version as i64 => {
                return Err(StorageError::new(
                    StorageErrorCode::VersionMismatch,
                    format!(
                        "kv unit '{}' is stamped version {stamped} on the medium, incompatible with descriptor version {}",
                        descriptor.name, descriptor.version
                    ),
                ));
            }
            Some(_) => {}
        }
        // Ensure the unit's record tables (both segments passed
        // UNIT_NAME_RE, so the identifiers are safe in DDL).
        for table in &descriptor.tables {
            let physical = record_table_name(&descriptor.name, table);
            let db = db.clone();
            tokio::task::spawn_blocking(move || {
                db.lock()
                    .execute_batch(&format!(
                        "CREATE TABLE IF NOT EXISTS \"{physical}\" (
                            key   TEXT PRIMARY KEY,
                            value TEXT NOT NULL
                        ) STRICT"
                    ))
                    .map_err(|error| {
                        StorageError::new(
                            StorageErrorCode::MalformedMedium,
                            format!("sqlite backend: {error}"),
                        )
                    })
            })
            .await
            .map_err(|join| {
                StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!("sqlite backend: ddl task failed: {join}"),
                )
            })??;
        }
        let units = Arc::clone(&self.units);
        let descriptor_name = descriptor.name.clone();
        Ok(Arc::new(SqliteKvUnit::new(
            db,
            descriptor,
            Arc::new(move || {
                units.lock().remove(&descriptor_name);
            }),
        )))
    }
}

#[async_trait]
impl KvFacet for SqliteKvFacet {
    async fn open(&self, descriptor: &KvUnitDescriptor) -> Result<Arc<dyn KvUnit>, StorageError> {
        let Some(backend) = self.backend.upgrade() else {
            return Err(closed_error("sqlite storage backend"));
        };
        backend.assert_open()?;
        if !unit_name_matches(&descriptor.name) {
            return Err(StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!(
                    "kv unit name '{}' violates ^[a-z][a-z0-9_]*$",
                    descriptor.name
                ),
            ));
        }
        for table in &descriptor.tables {
            if !unit_name_matches(table) {
                return Err(StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!(
                        "kv table name '{table}' in unit '{}' violates ^[a-z][a-z0-9_]*$",
                        descriptor.name
                    ),
                ));
            }
        }
        let (done_tx, done_rx) = oneshot::channel();
        {
            let mut units = backend.units.lock();
            if units.contains_key(&descriptor.name) {
                return Err(StorageError::new(
                    StorageErrorCode::Closed,
                    format!(
                        "kv unit '{}' is already open (double-open is a caller bug)",
                        descriptor.name
                    ),
                ));
            }
            units.insert(descriptor.name.clone(), Slot::Pending(done_rx));
        }
        // Reserve the name synchronously; the materialization completes
        // asynchronously and replaces the pending slot (or removes it on
        // failure).
        let backend_for_task = backend.clone();
        let descriptor_for_task = descriptor.clone();
        let name_for_slot = descriptor.name.clone();
        let units_for_slot = Arc::clone(&backend.units);
        let handle = tokio::spawn(async move {
            let result = backend_for_task.materialize(descriptor_for_task).await;
            match &result {
                Ok(unit) => {
                    units_for_slot
                        .lock()
                        .insert(name_for_slot, Slot::Open(unit.clone()));
                }
                Err(_) => {
                    units_for_slot.lock().remove(&name_for_slot);
                }
            }
            let _ = done_tx.send(result.clone());
            result
        });
        handle.await.map_err(|join| {
            StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!("sqlite backend: open task failed: {join}"),
            )
        })?
    }
}

#[async_trait]
impl StorageBackend for SqliteStorageBackend {
    fn kv(&self) -> Option<Arc<dyn KvFacet>> {
        Some(self.kv_facet.clone())
    }

    async fn close(&self) -> Result<(), StorageError> {
        self.closing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let slots: Vec<Slot> = {
            let mut units = self.units.lock();
            units.drain().map(|(_, slot)| slot).collect()
        };
        for slot in slots {
            match slot {
                Slot::Open(unit) => {
                    let _ = unit.close().await;
                }
                Slot::Pending(done) => {
                    // Drain a still-pending open: it either materializes
                    // (then closes) or rejects (nothing to release).
                    if let Ok(Ok(unit)) = done.await {
                        let _ = unit.close().await;
                    }
                }
            }
        }
        // The medium releases when the last `Connection` handle drops
        // (caller-held unit handles keep the shared connection alive; their
        // closed guards reject every primitive).
        Ok(())
    }
}

/// Register the SQLite backend as `sqlite` on the storage hub (TS `apply`).
/// The disposer unregisters the name first, then closes the backend.
pub fn apply(ctx: &Context, config: Config) -> Result<cordis::Disposer, String> {
    let hub = ctx
        .get_typed::<Arc<Storage>>("storage", false)
        .ok_or_else(|| "the storage hub is not configured".to_string())?
        .as_ref()
        .clone();
    let backend = SqliteStorageBackend::new(config);
    ctx.register_service(backend.clone());
    let unregister = hub
        .backend
        .register("sqlite", backend.clone())
        .map_err(|error| error.message)?;
    let dispose_backend = backend.clone();
    let disposer = ctx.effect(
        "storage-sqlite.registerBackend",
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
pub struct SqliteStoragePlugin {
    pub config: Config,
}

#[async_trait]
impl Plugin for SqliteStoragePlugin {
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
