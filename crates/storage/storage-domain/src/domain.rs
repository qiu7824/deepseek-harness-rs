//! Runtime of one open domain: authoritative in-memory state, the single
//! per-domain write chain, and change-event emission. Rust port of
//! `packages/storage/storage-domain/src/domain.ts`.
//!
//! # Deviations
//!
//! - The TS promise-chain write queue becomes a `tokio::sync::Mutex<()>`
//!   acquired per job (fair, FIFO-order); the close drain barrier is one
//!   final acquisition.
//! - `domain/changed` emission goes through cordis `emit` (fire-and-forget,
//!   per-listener containment is inherent) — the TS inline synchronous
//!   throw is inexpressible; the write is committed before emission in both.
//! - Domain-level rejections carry the TS prose; the `DomainError` code
//!   discriminants collapse into the error strings (no consumer in this
//!   repository switches on the code).
//! - `KvTable` values are JSON clones on read (the TS hands out stored
//!   references; Rust's ownership makes defensive copies the only sound
//!   choice).
//! - Reads after a domain fully closes panic (the TS synchronous
//!   `DomainError('closed')` throw; still catchable via `catch_unwind`).

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde_json::Value as JsonValue;
use tokio::sync::OnceCell;

use cordis::{Context, arc};

use dsh_storage::KvUnit;

use crate::events::DomainChanged;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DomainState {
    Open,
    Disposing,
    Closed,
}

/// Shared machinery handed to table handles and the global handle.
struct DomainHost {
    ctx: Context,
    domain_name: String,
    unit: Arc<dyn KvUnit>,
    chain: Arc<tokio::sync::Mutex<()>>,
    state: Mutex<DomainState>,
    on_closed: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    disposal: OnceCell<()>,
}

impl DomainHost {
    fn closed_error(&self) -> String {
        format!("domain '{}' is closed", self.domain_name)
    }

    /// Queue one job on the domain's single write chain (TS `enqueue`):
    /// reject once disposal begins, then serialize on the chain lock.
    async fn enqueue(&self) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
        if *self.state.lock() != DomainState::Open {
            return Err(self.closed_error());
        }
        let guard = Arc::clone(&self.chain).lock_owned().await;
        // Re-check after queuing: a close that began before this job got the
        // lock must win (TS order: `disposing` checked at enqueue time, but
        // the drain barrier waits for jobs already queued — keep both).
        if *self.state.lock() == DomainState::Closed {
            return Err(self.closed_error());
        }
        Ok(guard)
    }

    fn assert_readable(&self) {
        if *self.state.lock() == DomainState::Closed {
            panic!("{}", self.closed_error());
        }
    }

    /// Emit `domain/changed` for one durably landed write (TS
    /// `emitChanged`; containment is inherent in the fire-and-forget
    /// dispatch).
    fn emit_changed(&self, change: DomainChanged) {
        self.ctx.emit("domain/changed", vec![arc(change)]);
    }
}

/// A record-level table handle (TS `KvTable`).
#[async_trait::async_trait]
pub trait KvTable: Send + Sync {
    /// Read one record, synchronously from memory (a detached clone).
    fn get(&self, key: &str) -> Option<JsonValue>;

    /// Snapshot over `[key, record]` pairs.
    fn entries(&self) -> Vec<(String, JsonValue)>;

    /// Snapshot over keys.
    fn keys(&self) -> Vec<String>;

    /// Current record count.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Insert or overwrite one record durably (TS `put`).
    async fn put(&self, key: &str, value: JsonValue) -> Result<(), String>;

    /// Delete one record durably (TS `delete`).
    async fn delete(&self, key: &str) -> Result<bool, String>;

    /// Atomic read-modify-write on the domain's write chain (TS `update`);
    /// a missing key rejects with the `missing-key` prose.
    async fn update(
        &self,
        key: &str,
        f: Arc<dyn Fn(JsonValue) -> JsonValue + Send + Sync>,
    ) -> Result<JsonValue, String>;
}

/// Table handle bound to one in-memory record map and its domain's write
/// chain (TS `KvTableImpl`).
struct KvTableImpl {
    host: Arc<DomainHost>,
    table_name: String,
    records: Mutex<HashMap<String, JsonValue>>,
}

impl KvTableImpl {
    fn emit_put(&self, key: &str, value: &JsonValue) {
        self.host.emit_changed(DomainChanged::Put {
            domain: self.host.domain_name.clone(),
            table: self.table_name.clone(),
            key: key.to_string(),
            value: value.clone(),
        });
    }
}

#[async_trait::async_trait]
impl KvTable for KvTableImpl {
    fn get(&self, key: &str) -> Option<JsonValue> {
        self.host.assert_readable();
        self.records.lock().get(key).cloned()
    }

    fn entries(&self) -> Vec<(String, JsonValue)> {
        self.host.assert_readable();
        self.records
            .lock()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    fn keys(&self) -> Vec<String> {
        self.host.assert_readable();
        self.records.lock().keys().cloned().collect()
    }

    fn len(&self) -> usize {
        self.host.assert_readable();
        self.records.lock().len()
    }

    async fn put(&self, key: &str, value: JsonValue) -> Result<(), String> {
        let _guard = self.host.enqueue().await?;
        self.host
            .unit
            .put_record(&self.table_name, key, value.clone())
            .await
            .map_err(|error| error.message)?;
        self.records.lock().insert(key.to_string(), value.clone());
        self.emit_put(key, &value);
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<bool, String> {
        let _guard = self.host.enqueue().await?;
        // Existence is decided at this job's chain slot, not at call time.
        if !self.records.lock().contains_key(key) {
            return Ok(false);
        }
        self.host
            .unit
            .delete_record(&self.table_name, key)
            .await
            .map_err(|error| error.message)?;
        self.records.lock().remove(key);
        self.host.emit_changed(DomainChanged::Deleted {
            domain: self.host.domain_name.clone(),
            table: self.table_name.clone(),
            key: key.to_string(),
        });
        Ok(true)
    }

    async fn update(
        &self,
        key: &str,
        f: Arc<dyn Fn(JsonValue) -> JsonValue + Send + Sync>,
    ) -> Result<JsonValue, String> {
        let _guard = self.host.enqueue().await?;
        if !self.records.lock().contains_key(key) {
            return Err(format!(
                "domain '{}' table '{}' has no record '{key}' to update",
                self.host.domain_name, self.table_name
            ));
        }
        let next = f(self.records.lock().get(key).cloned().expect("checked"));
        self.host
            .unit
            .put_record(&self.table_name, key, next.clone())
            .await
            .map_err(|error| error.message)?;
        self.records.lock().insert(key.to_string(), next.clone());
        self.emit_put(key, &next);
        Ok(next)
    }
}

/// Handle on a domain's global singleton (TS `DomainGlobal`).
pub struct DomainGlobal {
    host: Arc<DomainHost>,
    value: RwLock<JsonValue>,
}

impl DomainGlobal {
    /// Current value, synchronously from the authoritative in-memory state.
    pub fn get(&self) -> JsonValue {
        self.host.assert_readable();
        self.value.read().clone()
    }

    /// Replace the value durably (TS `set`).
    pub async fn set(&self, value: JsonValue) -> Result<(), String> {
        let _guard = self.host.enqueue().await?;
        self.host
            .unit
            .set_global(value.clone())
            .await
            .map_err(|error| error.message)?;
        *self.value.write() = value.clone();
        self.host.emit_changed(DomainChanged::Put {
            domain: self.host.domain_name.clone(),
            table: String::new(),
            key: String::new(),
            value,
        });
        Ok(())
    }
}

/// One open domain (TS `Domain` + `DomainImpl` collapsed: the facility
/// erases the spec typing in both).
pub struct Domain {
    name: String,
    host: Arc<DomainHost>,
    tables: HashMap<String, Arc<dyn KvTable>>,
    global: Option<DomainGlobal>,
}

impl Domain {
    /// Construct one domain from a validated `loadAll` snapshot (the
    /// facility is the only constructor).
    pub(crate) fn new(
        ctx: Context,
        name: String,
        unit: Arc<dyn KvUnit>,
        table_records: HashMap<String, HashMap<String, JsonValue>>,
        global_value: Option<JsonValue>,
        on_closed: Arc<dyn Fn() + Send + Sync>,
    ) -> Arc<Self> {
        let host = Arc::new(DomainHost {
            ctx,
            domain_name: name.clone(),
            unit,
            chain: Arc::new(tokio::sync::Mutex::new(())),
            state: Mutex::new(DomainState::Open),
            on_closed: Mutex::new(Some(on_closed)),
            disposal: OnceCell::new(),
        });
        let mut tables = HashMap::new();
        for (table_name, records) in table_records {
            tables.insert(
                table_name.clone(),
                Arc::new(KvTableImpl {
                    host: host.clone(),
                    table_name,
                    records: Mutex::new(records),
                }) as Arc<dyn KvTable>,
            );
        }
        let global = global_value.map(|value| DomainGlobal {
            host: host.clone(),
            value: RwLock::new(value),
        });
        Arc::new(Self { name, host, tables, global })
    }

    /// Domain name from the spec.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Resolve one declared table handle; an undeclared name is a caller
    /// bug and panics (TS throw).
    pub fn table(&self, name: &str) -> Arc<dyn KvTable> {
        self.tables
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("domain '{}' declares no table '{name}'", self.name))
    }

    /// The global singleton handle; accessing it on a spec that declares no
    /// global is a caller bug and panics (TS throw).
    pub fn global(&self) -> &DomainGlobal {
        self.global
            .as_ref()
            .unwrap_or_else(|| panic!("domain '{}' declares no global", self.name))
    }

    /// Close this domain: reject new writes immediately, drain
    /// already-queued writes (their events still emit), release the backend
    /// unit, then free the domain name. Idempotent.
    pub async fn close(&self) {
        let _ = self
            .host
            .disposal
            .get_or_init(|| {
                let host = self.host.clone();
                async move {
                    *host.state.lock() = DomainState::Disposing;
                    // Drain barrier: one final chain acquisition waits for
                    // every already-queued job.
                    let _guard = host.chain.lock().await;
                    let _ = host.unit.close().await;
                    *host.state.lock() = DomainState::Closed;
                    let on_closed = host.on_closed.lock().take();
                    if let Some(on_closed) = on_closed {
                        on_closed();
                    }
                }
            })
            .await;
    }
}
