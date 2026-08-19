//! One opened SQLite KV unit. Rust port of
//! `packages/storage/storage-sqlite/src/unit.ts`: document-per-row
//! (`key TEXT` / `value TEXT` JSON) over the `u_<unit>_<table>` record
//! tables plus this unit's row in the shared `unit_globals` table. Each
//! primitive is a single statement, so atomicity comes from SQLite itself —
//! no explicit transactions, and no write queue (write ordering is the
//! caller's responsibility per the KV contract).
//!
//! # Deviations
//!
//! - The TS constructor prepares statements once; the Rust port uses
//!   `prepare_cached` per call (equivalent reuse semantics, no long-lived
//!   statement borrows).
//! - The `toJSON` hostile-value test is inexpressible: values are
//!   [`JsonValue`]s, whose serialization cannot throw.
//! - `rusqlite::Connection` is `!Sync`; the shared handle is
//!   `Arc<parking_lot::Mutex<Connection>>` with all primitives running
//!   synchronously under the lock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value as JsonValue;

use dsh_storage::{KvUnit, KvUnitDescriptor, KvUnitSnapshot, StorageError, StorageErrorCode};

use crate::schema::record_table_name;

/// One opened SQLite unit (TS `SqliteKvUnit`).
pub struct SqliteKvUnit {
    db: Arc<Mutex<Connection>>,
    descriptor: KvUnitDescriptor,
    on_close: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    closed: AtomicBool,
}

impl SqliteKvUnit {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        descriptor: KvUnitDescriptor,
        on_close: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            db,
            descriptor,
            on_close: Mutex::new(Some(on_close)),
            closed: AtomicBool::new(false),
        }
    }

    fn ensure_open(&self) -> Result<(), StorageError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!("kv unit '{}' is closed", self.descriptor.name),
            ));
        }
        Ok(())
    }

    /// Parse one stored value column, mapping bad JSON to
    /// `malformed-medium` (TS `parseValue`).
    fn parse_value(&self, text: &str, slot: &str) -> Result<JsonValue, StorageError> {
        serde_json::from_str(text).map_err(|error| {
            StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!(
                    "kv unit '{}' holds unparsable JSON at {slot}: {error}",
                    self.descriptor.name
                ),
            )
        })
    }

    fn check_table(&self, table: &str) -> Result<(), StorageError> {
        if !self
            .descriptor
            .tables
            .iter()
            .any(|declared| declared == table)
        {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!(
                    "kv unit '{}' declared no table '{table}'",
                    self.descriptor.name
                ),
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl KvUnit for SqliteKvUnit {
    async fn load_all(&self) -> Result<KvUnitSnapshot, StorageError> {
        self.ensure_open()?;
        let db = self.db.lock();
        let mut tables = std::collections::HashMap::new();
        for table in &self.descriptor.tables {
            let physical = record_table_name(&self.descriptor.name, table);
            let mut records = std::collections::HashMap::new();
            let mut statement = db
                .prepare_cached(&format!("SELECT key, value FROM \"{physical}\""))
                .map_err(|error| {
                    StorageError::new(
                        StorageErrorCode::MalformedMedium,
                        format!("kv unit '{}': {error}", self.descriptor.name),
                    )
                })?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| {
                    StorageError::new(
                        StorageErrorCode::MalformedMedium,
                        format!("kv unit '{}': {error}", self.descriptor.name),
                    )
                })?;
            for row in rows {
                let (key, value) = row.map_err(|error| {
                    StorageError::new(
                        StorageErrorCode::MalformedMedium,
                        format!("kv unit '{}': {error}", self.descriptor.name),
                    )
                })?;
                let parsed = self.parse_value(&value, &format!("table '{table}' key '{key}'"))?;
                records.insert(key, parsed);
            }
            tables.insert(table.clone(), records);
        }
        let mut global = JsonValue::Null;
        if self.descriptor.has_global {
            let value: Option<String> = db
                .query_row(
                    "SELECT value FROM unit_globals WHERE unit = ?1",
                    [self.descriptor.name.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| {
                    StorageError::new(
                        StorageErrorCode::MalformedMedium,
                        format!("kv unit '{}': {error}", self.descriptor.name),
                    )
                })?;
            if let Some(value) = value {
                global = self.parse_value(&value, "global slot")?;
            }
        }
        Ok(KvUnitSnapshot { tables, global })
    }

    async fn put_record(
        &self,
        table: &str,
        key: &str,
        value: JsonValue,
    ) -> Result<(), StorageError> {
        self.ensure_open()?;
        self.check_table(table)?;
        let physical = record_table_name(&self.descriptor.name, table);
        let text = serde_json::to_string(&value).map_err(|error| {
            StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!(
                    "kv unit '{}': value is not serializable: {error}",
                    self.descriptor.name
                ),
            )
        })?;
        self.db
            .lock()
            .execute(
                &format!(
                    "INSERT INTO \"{physical}\" (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value"
                ),
                rusqlite::params![key, text],
            )
            .map_err(|error| {
                StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!("kv unit '{}': {error}", self.descriptor.name),
                )
            })?;
        Ok(())
    }

    async fn delete_record(&self, table: &str, key: &str) -> Result<(), StorageError> {
        self.ensure_open()?;
        self.check_table(table)?;
        let physical = record_table_name(&self.descriptor.name, table);
        self.db
            .lock()
            .execute(
                &format!("DELETE FROM \"{physical}\" WHERE key = ?1"),
                rusqlite::params![key],
            )
            .map_err(|error| {
                StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!("kv unit '{}': {error}", self.descriptor.name),
                )
            })?;
        Ok(())
    }

    async fn set_global(&self, value: JsonValue) -> Result<(), StorageError> {
        self.ensure_open()?;
        if !self.descriptor.has_global {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!("kv unit '{}' declared no global slot", self.descriptor.name),
            ));
        }
        let text = serde_json::to_string(&value).map_err(|error| {
            StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!(
                    "kv unit '{}': value is not serializable: {error}",
                    self.descriptor.name
                ),
            )
        })?;
        self.db
            .lock()
            .execute(
                "INSERT INTO unit_globals (unit, value) VALUES (?1, ?2) ON CONFLICT(unit) DO UPDATE SET value = excluded.value",
                rusqlite::params![self.descriptor.name.as_str(), text],
            )
            .map_err(|error| {
                StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!("kv unit '{}': {error}", self.descriptor.name),
                )
            })?;
        Ok(())
    }

    async fn close(&self) -> Result<(), StorageError> {
        if !self.closed.swap(true, Ordering::SeqCst) {
            let on_close = self.on_close.lock().take();
            if let Some(on_close) = on_close {
                on_close();
            }
        }
        Ok(())
    }
}
