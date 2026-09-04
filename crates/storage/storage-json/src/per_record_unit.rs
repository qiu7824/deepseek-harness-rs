//! JSON KV unit using one versioned document per table record.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use parking_lot::Mutex;
use serde_json::Value as JsonValue;
use tokio::sync::Notify;

use dsh_storage::{
    KvInvalidEntry, KvUnit, KvUnitDescriptor, KvUnitSnapshot, StorageError, StorageErrorCode,
};

use crate::atomic::write_atomic;
use crate::format::{parse, parse_record, serialize_record};

fn is_safe_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

pub async fn open_per_record_unit(
    descriptor: KvUnitDescriptor,
    root: &Path,
    on_close: Arc<dyn Fn() + Send + Sync>,
) -> Result<PerRecordJsonUnit, StorageError> {
    Ok(PerRecordJsonUnit {
        dir: root.join(&descriptor.name),
        descriptor,
        on_close: Mutex::new(Some(on_close)),
        closed: AtomicBool::new(false),
        in_flight: InFlightWrites::new(),
    })
}

pub struct PerRecordJsonUnit {
    descriptor: KvUnitDescriptor,
    dir: PathBuf,
    on_close: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    closed: AtomicBool,
    in_flight: InFlightWrites,
}

struct InFlightWrites {
    count: AtomicUsize,
    drained: Notify,
}

impl InFlightWrites {
    fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            drained: Notify::new(),
        }
    }

    fn begin<'a>(&'a self, closed: &AtomicBool) -> Option<InFlightWrite<'a>> {
        if closed.load(Ordering::SeqCst) {
            return None;
        }
        self.count.fetch_add(1, Ordering::SeqCst);
        if closed.load(Ordering::SeqCst) {
            self.finish();
            return None;
        }
        Some(InFlightWrite { owner: self })
    }

    fn finish(&self) {
        if self.count.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.drained.notify_waiters();
        }
    }

    async fn drain(&self) {
        loop {
            let notified = self.drained.notified();
            if self.count.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }
}

struct InFlightWrite<'a> {
    owner: &'a InFlightWrites,
}

impl Drop for InFlightWrite<'_> {
    fn drop(&mut self) {
        self.owner.finish();
    }
}

impl PerRecordJsonUnit {
    fn assert_open(&self) -> Result<(), StorageError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!("unit '{}' is closed", self.descriptor.name),
            ));
        }
        Ok(())
    }

    fn begin_write(&self) -> Result<InFlightWrite<'_>, StorageError> {
        self.in_flight.begin(&self.closed).ok_or_else(|| {
            StorageError::new(
                StorageErrorCode::Closed,
                format!("unit '{}' is closed", self.descriptor.name),
            )
        })
    }

    fn table_dir(&self, table: &str) -> Result<PathBuf, StorageError> {
        if !self
            .descriptor
            .tables
            .iter()
            .any(|declared| declared == table)
        {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!(
                    "unit '{}' does not declare table '{table}'",
                    self.descriptor.name
                ),
            ));
        }
        Ok(self.dir.join(table))
    }

    fn assert_safe_key(&self, key: &str) -> Result<(), StorageError> {
        if !is_safe_key(key) {
            return Err(StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!(
                    "unit '{}': per-record key '{key}' is not path-safe (must match ^[a-zA-Z0-9_-]+$)",
                    self.descriptor.name
                ),
            ));
        }
        Ok(())
    }

    fn accepted_versions(&self) -> Vec<u64> {
        let mut versions = Vec::with_capacity(1 + self.descriptor.compatible_versions.len());
        versions.push(self.descriptor.version);
        versions.extend(self.descriptor.compatible_versions.iter().copied());
        versions
    }

    fn legacy_path(&self) -> PathBuf {
        self.dir
            .parent()
            .expect("unit parent")
            .join(format!("{}.json", self.descriptor.name))
    }

    fn empty_snapshot(&self) -> KvUnitSnapshot {
        KvUnitSnapshot {
            tables: self
                .descriptor
                .tables
                .iter()
                .map(|table| (table.clone(), HashMap::new()))
                .collect(),
            global: JsonValue::Null,
            invalid: Vec::new(),
        }
    }

    async fn bootstrap_legacy(&self) -> Result<KvUnitSnapshot, StorageError> {
        let _write = self.begin_write()?;
        let mut snapshot = self.empty_snapshot();
        let bytes = match tokio::fs::read(self.legacy_path()).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(snapshot),
            Err(error) => return Err(self.io_error("read legacy unit", error)),
        };
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                snapshot.invalid.push(KvInvalidEntry::LegacyUnit {
                    error: StorageError::new(
                        StorageErrorCode::MalformedMedium,
                        "legacy unit is not UTF-8",
                    ),
                });
                return Ok(snapshot);
            }
        };
        let mut legacy_descriptor = self.descriptor.clone();
        if let Some(version) = serde_json::from_str::<JsonValue>(&text)
            .ok()
            .and_then(|document| document.get("unit")?.get("version")?.as_u64())
            && self.accepted_versions().contains(&version)
        {
            legacy_descriptor.version = version;
        }
        let state = match parse(&text, &legacy_descriptor) {
            Ok(state) => state,
            Err(error) => {
                snapshot.invalid.push(KvInvalidEntry::LegacyUnit { error });
                return Ok(snapshot);
            }
        };
        if state
            .tables
            .values()
            .any(|records| records.keys().any(|key| !is_safe_key(key)))
        {
            snapshot.invalid.push(KvInvalidEntry::LegacyUnit {
                error: StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    "legacy unit contains a record key incompatible with per-record layout",
                ),
            });
            return Ok(snapshot);
        }
        // Publish the complete layout at once; a crash cannot leave a partial
        // migration that suppresses the remaining legacy records on restart.
        let staging = self.dir.parent().expect("unit parent").join(format!(
            ".{}-migration-{}",
            self.descriptor.name,
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir(&staging)
            .await
            .map_err(|error| self.io_error("create migration directory", error))?;
        let outcome = async {
            for (table, records) in &state.tables {
                for (key, value) in records {
                    self.write_document(
                        staging.join(table).join(format!("{key}.json")),
                        value.clone(),
                    )
                    .await?;
                    snapshot
                        .tables
                        .get_mut(table)
                        .expect("declared table")
                        .insert(key.clone(), value.clone());
                }
            }
            if self.descriptor.has_global && !state.global.is_null() {
                self.write_document(staging.join("global.json"), state.global.clone())
                    .await?;
                snapshot.global = state.global;
            }
            tokio::fs::rename(&staging, &self.dir)
                .await
                .map_err(|error| self.io_error("publish legacy migration", error))?;
            #[cfg(unix)]
            crate::atomic::fsync_directory(self.dir.parent().expect("unit parent"))
                .map_err(|error| self.io_error("sync legacy migration", error))?;
            Ok::<(), StorageError>(())
        }
        .await;
        if outcome.is_err() {
            let _ = tokio::fs::remove_dir_all(&staging).await;
        }
        outcome?;
        Ok(snapshot)
    }

    fn io_error(&self, action: &str, error: std::io::Error) -> StorageError {
        StorageError::new(
            StorageErrorCode::MalformedMedium,
            format!(
                "unit '{}': failed to {action}: {error}",
                self.descriptor.name
            ),
        )
    }

    async fn backup_path(&self, path: PathBuf) -> Result<Option<String>, StorageError> {
        let _write = self.begin_write()?;
        let mut moved_name = path.as_os_str().to_os_string();
        moved_name.push(format!(".bak.{}", uuid::Uuid::new_v4()));
        let moved = PathBuf::from(moved_name);
        tokio::fs::rename(&path, &moved)
            .await
            .map_err(|error| self.io_error("preserve unreadable document", error))?;
        #[cfg(unix)]
        crate::atomic::fsync_directory(moved.parent().expect("backup parent"))
            .map_err(|error| self.io_error("sync backup", error))?;
        Ok(Some(moved.to_string_lossy().into_owned()))
    }

    async fn write_document(&self, path: PathBuf, value: JsonValue) -> Result<(), StorageError> {
        let parent = path
            .parent()
            .expect("record path has a parent")
            .to_path_buf();
        tokio::fs::create_dir_all(&parent).await.map_err(|error| {
            StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!(
                    "unit '{}': failed to create record directory: {error}",
                    self.descriptor.name
                ),
            )
        })?;
        let data = serialize_record(self.descriptor.version, &value);
        let unit_name = self.descriptor.name.clone();
        tokio::task::spawn_blocking(move || write_atomic(&path, &data))
            .await
            .map_err(|join| {
                StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!("unit '{unit_name}': record write task failed: {join}"),
                )
            })?
            .map_err(|error| {
                StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!("unit '{unit_name}': record publish failed: {error}"),
                )
            })
    }
}

#[async_trait::async_trait]
impl KvUnit for PerRecordJsonUnit {
    async fn load_all(&self) -> Result<KvUnitSnapshot, StorageError> {
        self.assert_open()?;
        // An existing new-layout directory, including an empty one, is
        // authoritative. Deleted records must not reappear from legacy data.
        if !tokio::fs::try_exists(&self.dir)
            .await
            .map_err(|error| self.io_error("inspect unit directory", error))?
        {
            return self.bootstrap_legacy().await;
        }
        let mut snapshot = self.empty_snapshot();
        let accepted = self.accepted_versions();
        for table in &self.descriptor.tables {
            let dir = self.dir.join(table);
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(self.io_error("read table directory", error)),
            };
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| self.io_error("enumerate table", error))?
            {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(key) = name.strip_suffix(".json") else {
                    continue;
                };
                self.assert_safe_key(key)?;
                let kind = entry
                    .file_type()
                    .await
                    .map_err(|error| self.io_error("inspect record", error))?;
                if !kind.is_file() {
                    return Err(StorageError::new(
                        StorageErrorCode::MalformedMedium,
                        "record document must be a regular file",
                    ));
                }
                let bytes = tokio::fs::read(entry.path())
                    .await
                    .map_err(|error| self.io_error("read record document", error))?;
                let value = std::str::from_utf8(&bytes)
                    .map_err(|_| {
                        StorageError::new(
                            StorageErrorCode::MalformedMedium,
                            "record document is not UTF-8",
                        )
                    })
                    .and_then(|text| parse_record(text, &accepted));
                match value {
                    Ok(value) => {
                        snapshot
                            .tables
                            .get_mut(table)
                            .expect("declared table")
                            .insert(key.to_string(), value);
                    }
                    Err(error) => snapshot.invalid.push(KvInvalidEntry::Record {
                        table: table.clone(),
                        key: key.to_string(),
                        error,
                    }),
                }
            }
        }
        if self.descriptor.has_global {
            snapshot.global = match tokio::fs::read_to_string(self.dir.join("global.json")).await {
                Ok(text) => parse_record(&text, &accepted)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => JsonValue::Null,
                Err(error) => return Err(self.io_error("read global document", error)),
            };
        }
        Ok(snapshot)
    }

    async fn put_record(
        &self,
        table: &str,
        key: &str,
        value: JsonValue,
    ) -> Result<(), StorageError> {
        self.assert_open()?;
        self.assert_safe_key(key)?;
        let path = self.table_dir(table)?.join(format!("{key}.json"));
        let _write = self.begin_write()?;
        self.write_document(path, value).await
    }

    async fn delete_record(&self, table: &str, key: &str) -> Result<(), StorageError> {
        self.assert_open()?;
        self.assert_safe_key(key)?;
        let path = self.table_dir(table)?.join(format!("{key}.json"));
        let _write = self.begin_write()?;
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!(
                    "unit '{}': record delete failed: {error}",
                    self.descriptor.name
                ),
            )),
        }
    }

    async fn backup_record(&self, table: &str, key: &str) -> Result<Option<String>, StorageError> {
        self.assert_open()?;
        self.assert_safe_key(key)?;
        self.backup_path(self.table_dir(table)?.join(format!("{key}.json")))
            .await
    }

    async fn backup_legacy_unit(&self) -> Result<Option<String>, StorageError> {
        self.assert_open()?;
        self.backup_path(self.legacy_path()).await
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
        let _write = self.begin_write()?;
        self.write_document(self.dir.join("global.json"), value)
            .await
    }

    async fn close(&self) -> Result<(), StorageError> {
        self.closed.store(true, Ordering::SeqCst);
        self.in_flight.drain().await;
        if let Some(on_close) = self.on_close.lock().take() {
            on_close();
        }
        Ok(())
    }
}
