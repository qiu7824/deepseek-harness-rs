//! One opened JSON unit. Rust port of
//! `packages/storage/storage-json/src/unit.ts`. The in-memory state is
//! authoritative; every write primitive mutates it and republishes the
//! whole file atomically. Writes are NOT queued here — per the backend
//! contract, write ordering belongs to the caller (the domain layer's write
//! chain); this unit only guarantees that each single call publishes a
//! complete, durable file.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use parking_lot::Mutex;
use serde_json::Value as JsonValue;
use tokio::sync::Notify;

use dsh_storage::{KvUnit, KvUnitDescriptor, KvUnitSnapshot, StorageError, StorageErrorCode};

use crate::atomic::write_atomic;
use crate::format::{UnitState, parse, serialize};

/// Open (load or lazily create) one unit backed by `path` (TS
/// `openJsonUnit`).
pub async fn open_json_unit(
    descriptor: KvUnitDescriptor,
    path: PathBuf,
    on_close: Arc<dyn Fn() + Send + Sync>,
) -> Result<JsonKvUnit, StorageError> {
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        // Non-ENOENT read failures propagate (the TS raw error; the code
        // collapses into the message here).
        Err(error) => {
            return Err(StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!("unit '{}': failed to read: {error}", descriptor.name),
            ))
        }
    };
    let state = match text {
        None => UnitState {
            version: descriptor.version,
            global: JsonValue::Null,
            tables: descriptor
                .tables
                .iter()
                .map(|table| (table.clone(), indexmap::IndexMap::new()))
                .collect(),
        },
        Some(text) => parse(&text, &descriptor)?,
    };
    Ok(JsonKvUnit {
        descriptor,
        path,
        state: Mutex::new(state),
        on_close: Mutex::new(Some(on_close)),
        closed: AtomicBool::new(false),
        in_flight: Arc::new(InFlight::new()),
    })
}

struct InFlight {
    count: AtomicUsize,
    notify: Notify,
}

impl InFlight {
    fn new() -> Self {
        Self { count: AtomicUsize::new(0), notify: Notify::new() }
    }

    fn begin(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn end(&self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    async fn drain(&self) {
        loop {
            if self.count.load(Ordering::SeqCst) == 0 {
                return;
            }
            self.notify.notified().await;
        }
    }
}

/// One opened JSON unit (TS `JsonKvUnit`).
pub struct JsonKvUnit {
    descriptor: KvUnitDescriptor,
    path: PathBuf,
    state: Mutex<UnitState>,
    on_close: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    closed: AtomicBool,
    in_flight: Arc<InFlight>,
}

impl JsonKvUnit {
    fn assert_open(&self) -> Result<(), StorageError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!("unit '{}' is closed", self.descriptor.name),
            ));
        }
        Ok(())
    }

    /// Whether one declared table exists (TS `records`' missing-table
    /// check); an undeclared name is a caller bug.
    fn check_table(&self, table: &str) -> Result<(), StorageError> {
        if !self.state.lock().tables.contains_key(table) {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!(
                    "unit '{}' does not declare table '{table}'",
                    self.descriptor.name
                ),
            ));
        }
        Ok(())
    }

    /// Publish the current state durably, tracking the write for `close`.
    async fn publish(&self) -> Result<(), StorageError> {
        let data = serialize(&self.descriptor.name, &self.state.lock());
        self.in_flight.begin();
        let path = self.path.clone();
        let outcome = tokio::task::spawn_blocking(move || write_atomic(&path, &data))
            .await
            .map_err(|join| {
                StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!(
                        "unit '{}': publish task failed: {join}",
                        self.descriptor.name
                    ),
                )
            })
            .and_then(|result| {
                result.map_err(|error| {
                    StorageError::new(
                        StorageErrorCode::MalformedMedium,
                        format!(
                            "unit '{}': publish failed: {error}",
                            self.descriptor.name
                        ),
                    )
                })
            });
        self.in_flight.end();
        outcome
    }
}

#[async_trait::async_trait]
impl KvUnit for JsonKvUnit {
    async fn load_all(&self) -> Result<KvUnitSnapshot, StorageError> {
        self.assert_open()?;
        let state = self.state.lock();
        Ok(KvUnitSnapshot {
            tables: state
                .tables
                .iter()
                .map(|(table, records)| {
                    (
                        table.clone(),
                        records
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    )
                })
                .collect(),
            global: state.global.clone(),
        })
    }

    async fn put_record(
        &self,
        table: &str,
        key: &str,
        value: JsonValue,
    ) -> Result<(), StorageError> {
        self.assert_open()?;
        self.check_table(table)?;
        let (had_key, previous) = {
            let mut state = self.state.lock();
            let records = state.tables.get_mut(table).expect("declared table checked above");
            let had_key = records.contains_key(key);
            let previous = records.get(key).cloned();
            records.insert(key.to_string(), value);
            (had_key, previous)
        };
        if let Err(error) = self.publish().await {
            // Roll back on a failed publish: memory is authoritative, so a
            // rejected write must not survive in memory.
            let mut state = self.state.lock();
            let records = state.tables.get_mut(table).expect("declared table checked above");
            match (had_key, previous) {
                (true, Some(previous)) => {
                    records.insert(key.to_string(), previous);
                }
                (false, _) | (true, None) => {
                    records.shift_remove(key);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    async fn delete_record(&self, table: &str, key: &str) -> Result<(), StorageError> {
        self.assert_open()?;
        self.check_table(table)?;
        let previous = {
            let mut state = self.state.lock();
            let records = state.tables.get_mut(table).expect("declared table checked above");
            match records.get(key).cloned() {
                Some(previous) => {
                    records.shift_remove(key);
                    Some(previous)
                }
                None => None,
            }
        };
        let Some(previous) = previous else {
            return Ok(());
        };
        if let Err(error) = self.publish().await {
            let mut state = self.state.lock();
            state
                .tables
                .get_mut(table)
                .expect("declared table checked above")
                .insert(key.to_string(), previous);
            return Err(error);
        }
        Ok(())
    }

    async fn set_global(&self, value: JsonValue) -> Result<(), StorageError> {
        self.assert_open()?;
        if !self.descriptor.has_global {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!(
                    "unit '{}' does not declare a global slot",
                    self.descriptor.name
                ),
            ));
        }
        let previous = {
            let mut state = self.state.lock();
            let previous = state.global.clone();
            state.global = value;
            previous
        };
        if let Err(error) = self.publish().await {
            self.state.lock().global = previous;
            return Err(error);
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), StorageError> {
        if self.closed.swap(true, Ordering::SeqCst) {
            self.in_flight.drain().await;
            return Ok(());
        }
        self.in_flight.drain().await;
        let on_close = self.on_close.lock().take();
        if let Some(on_close) = on_close {
            on_close();
        }
        Ok(())
    }
}
