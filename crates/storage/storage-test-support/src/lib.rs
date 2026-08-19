//! In-memory storage backend test double implementing the full [`KvUnit`]
//! primitive set. Rust port of the TS
//! `packages/storage/storage-domain/tests/helpers/memory-backend.ts`.
//!
//! Fidelity to the backend contract: version stamping and
//! `version-mismatch` on reopen, `malformed` never (memory cannot corrupt),
//! per-call atomicity trivially, `closed` after close, delete idempotence.
//! Media survive across backends through the shared pool, which simulates
//! process restarts.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value as JsonValue;

use dsh_storage::{
    KvFacet, KvUnit, KvUnitDescriptor, KvUnitSnapshot, StorageBackend, StorageError,
    StorageErrorCode, closed_error, version_mismatch_error,
};

/// One unit's medium: tables of records plus the global slot (`Null` =
/// never written).
#[derive(Debug, Clone, Default)]
pub struct MemoryMedium {
    pub tables: HashMap<String, HashMap<String, JsonValue>>,
    pub global: JsonValue,
}

impl MemoryMedium {
    fn new() -> Self {
        Self {
            tables: HashMap::new(),
            global: JsonValue::Null,
        }
    }
}

/// Shared media pool (TS `MemoryMediaPool`).
pub struct MemoryMediaPool {
    /// Unit name → its records; a missing entry is a never-materialized
    /// unit.
    pub media: Mutex<HashMap<String, MemoryMedium>>,
    /// Unit name → stamped version; tests may pre-stamp to force
    /// `version-mismatch`.
    pub versions: Mutex<HashMap<String, u64>>,
    /// When positive, that many subsequent write primitives reject without
    /// touching the medium, decrementing per rejection.
    pub fail_next_writes: std::sync::atomic::AtomicUsize,
}

impl Default for MemoryMediaPool {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryMediaPool {
    pub fn new() -> Self {
        Self {
            media: Mutex::new(HashMap::new()),
            versions: Mutex::new(HashMap::new()),
            fail_next_writes: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Consume one injected failure, throwing in a rejected write's place.
    fn consume_injected_failure(&self) -> Result<(), StorageError> {
        use std::sync::atomic::Ordering;
        let mut remaining = self.fail_next_writes.load(Ordering::SeqCst);
        loop {
            if remaining == 0 {
                return Ok(());
            }
            match self.fail_next_writes.compare_exchange(
                remaining,
                remaining - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Err(StorageError::new(
                        StorageErrorCode::Closed,
                        "injected write failure",
                    ));
                }
                Err(current) => remaining = current,
            }
        }
    }
}

/// In-memory KV unit over one pooled medium.
struct MemoryKvUnit {
    pool: Arc<MemoryMediaPool>,
    descriptor: KvUnitDescriptor,
    closed: std::sync::atomic::AtomicBool,
    on_close: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl MemoryKvUnit {
    fn assert_open(&self) -> Result<(), StorageError> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(closed_error(&format!(
                "memory unit '{}'",
                self.descriptor.name
            )));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl KvUnit for MemoryKvUnit {
    async fn load_all(&self) -> Result<KvUnitSnapshot, StorageError> {
        self.assert_open()?;
        let media = self.pool.media.lock();
        let medium = media.get(&self.descriptor.name);
        let mut tables = HashMap::new();
        for table in &self.descriptor.tables {
            tables.insert(
                table.clone(),
                medium
                    .and_then(|m| m.tables.get(table))
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        Ok(KvUnitSnapshot {
            tables,
            global: medium.map(|m| m.global.clone()).unwrap_or(JsonValue::Null),
        })
    }

    async fn put_record(
        &self,
        table: &str,
        key: &str,
        value: JsonValue,
    ) -> Result<(), StorageError> {
        self.assert_open()?;
        self.pool.consume_injected_failure()?;
        let mut media = self.pool.media.lock();
        let medium = media
            .entry(self.descriptor.name.clone())
            .or_insert_with(MemoryMedium::new);
        medium
            .tables
            .entry(table.to_string())
            .or_default()
            .insert(key.to_string(), value);
        Ok(())
    }

    async fn delete_record(&self, table: &str, key: &str) -> Result<(), StorageError> {
        self.assert_open()?;
        self.pool.consume_injected_failure()?;
        let mut media = self.pool.media.lock();
        if let Some(medium) = media.get_mut(&self.descriptor.name) {
            if let Some(records) = medium.tables.get_mut(table) {
                records.remove(key);
            }
        }
        Ok(())
    }

    async fn set_global(&self, value: JsonValue) -> Result<(), StorageError> {
        self.assert_open()?;
        self.pool.consume_injected_failure()?;
        let mut media = self.pool.media.lock();
        let medium = media
            .entry(self.descriptor.name.clone())
            .or_insert_with(MemoryMedium::new);
        medium.global = value;
        Ok(())
    }

    async fn close(&self) -> Result<(), StorageError> {
        if !self.closed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let on_close = self.on_close.lock().take();
            if let Some(on_close) = on_close {
                on_close();
            }
        }
        Ok(())
    }
}

/// The in-memory KV facet: opens units over one shared pool.
struct MemoryKvFacet {
    pool: Arc<MemoryMediaPool>,
    open_units: Arc<Mutex<HashSet<String>>>,
    closed: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl KvFacet for MemoryKvFacet {
    async fn open(&self, descriptor: &KvUnitDescriptor) -> Result<Arc<dyn KvUnit>, StorageError> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(closed_error("memory backend"));
        }
        {
            let open_units = self.open_units.lock();
            if open_units.contains(&descriptor.name) {
                return Err(StorageError::new(
                    StorageErrorCode::Closed,
                    format!(
                        "memory unit '{}' is already open (double-open is a caller bug)",
                        descriptor.name
                    ),
                ));
            }
        }
        {
            let mut versions = self.pool.versions.lock();
            match versions.get(&descriptor.name) {
                None => {
                    versions.insert(descriptor.name.clone(), descriptor.version);
                }
                Some(&stamped) if stamped != descriptor.version => {
                    return Err(version_mismatch_error(
                        &descriptor.name,
                        stamped,
                        descriptor.version,
                    ));
                }
                Some(_) => {}
            }
        }
        {
            let mut media = self.pool.media.lock();
            media
                .entry(descriptor.name.clone())
                .or_insert_with(MemoryMedium::new);
        }
        self.open_units.lock().insert(descriptor.name.clone());
        let pool = self.pool.clone();
        let descriptor_name = descriptor.name.clone();
        let open_units = Arc::clone(&self.open_units);
        Ok(Arc::new(MemoryKvUnit {
            pool,
            descriptor: descriptor.clone(),
            closed: std::sync::atomic::AtomicBool::new(false),
            on_close: Mutex::new(Some(Arc::new(move || {
                open_units.lock().remove(&descriptor_name);
            }))),
        }))
    }
}

/// In-memory storage backend (TS `MemoryStorageBackend`).
pub struct MemoryStorageBackend {
    pub pool: Arc<MemoryMediaPool>,
    kv_facet: Arc<MemoryKvFacet>,
}

impl MemoryStorageBackend {
    pub fn new(pool: Arc<MemoryMediaPool>) -> Self {
        let kv_facet = Arc::new(MemoryKvFacet {
            pool: pool.clone(),
            open_units: Arc::new(Mutex::new(HashSet::new())),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        Self { pool, kv_facet }
    }

    pub fn with_shared_pool(pool: Arc<MemoryMediaPool>) -> Arc<Self> {
        Arc::new(Self::new(pool))
    }

    /// The stored medium record for one session id (test helper mirroring
    /// the TS `storedRecord`).
    pub fn stored_record(&self, unit: &str, table: &str, key: &str) -> Option<JsonValue> {
        self.pool
            .media
            .lock()
            .get(unit)
            .and_then(|medium| medium.tables.get(table))
            .and_then(|records| records.get(key))
            .cloned()
    }
}

#[async_trait::async_trait]
impl StorageBackend for MemoryStorageBackend {
    fn kv(&self) -> Option<Arc<dyn KvFacet>> {
        Some(self.kv_facet.clone())
    }

    async fn close(&self) -> Result<(), StorageError> {
        self.kv_facet
            .closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.kv_facet.open_units.lock().clear();
        Ok(())
    }
}
