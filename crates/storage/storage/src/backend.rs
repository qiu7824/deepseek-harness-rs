//! Backend-facing vocabulary of the storage hub. Rust port of
//! `packages/storage/storage/src/backend.ts`.
//!
//! A backend owns one medium (a file-tree root, a database file) and
//! exposes operation groups over it; facets are optional members — a
//! backend that cannot serve a data kind simply omits it, and resolution
//! fails loud instead. Values are opaque JSON to this layer: no schema, no
//! events, no domain meaning. A unit does NOT serialize concurrent writes —
//! write ordering is the caller's responsibility (the domain layer runs one
//! write chain per unit); the unit only guarantees that each single call is
//! atomic on the medium and durable once resolved.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value as JsonValue;

use crate::error::{StorageError, StorageErrorCode};

/// Allowed format for unit and table names: safe as a file name and as a
/// SQL identifier segment without escaping (TS `UNIT_NAME_RE`).
pub fn unit_name_matches(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// Static identity and shape of one KV unit, projected from its owner's
/// spec (TS `KvUnitDescriptor`).
#[derive(Debug, Clone, PartialEq)]
pub struct KvUnitDescriptor {
    /// Unit name; must match [`unit_name_matches`]. Also the file-name /
    /// SQL-identifier segment.
    pub name: String,
    /// Unit format version; a non-negative integer stamped on the medium at
    /// first materialization.
    pub version: u64,
    /// Table names; each must match [`unit_name_matches`].
    pub tables: Vec<String>,
    /// Whether this unit carries the global singleton slot.
    pub has_global: bool,
}

/// The full current snapshot of one opened unit (TS `loadAll` result).
#[derive(Debug, Clone, PartialEq)]
pub struct KvUnitSnapshot {
    /// Every table's records keyed by table name.
    pub tables: HashMap<String, HashMap<String, JsonValue>>,
    /// The global singleton (`Null` when never written or not declared).
    pub global: JsonValue,
}

/// One opened unit (TS `KvUnit`). Any call after `close` rejects with
/// `closed`.
#[async_trait::async_trait]
pub trait KvUnit: Send + Sync {
    /// Read the full current snapshot.
    async fn load_all(&self) -> Result<KvUnitSnapshot, StorageError>;

    /// Upsert one record durably. Overwrite semantics.
    async fn put_record(
        &self,
        table: &str,
        key: &str,
        value: JsonValue,
    ) -> Result<(), StorageError>;

    /// Delete one record durably. Idempotent.
    async fn delete_record(&self, table: &str, key: &str) -> Result<(), StorageError>;

    /// Write the global singleton durably. Only valid when the descriptor
    /// declared `hasGlobal`.
    async fn set_global(&self, value: JsonValue) -> Result<(), StorageError>;

    /// Drain this unit's in-flight writes and release it. Idempotent.
    async fn close(&self) -> Result<(), StorageError>;
}

/// The key-value data shape: whole-unit snapshots plus per-record durable
/// writes (TS `KvFacet`).
#[async_trait::async_trait]
pub trait KvFacet: Send + Sync {
    /// Open one unit, creating it when the medium holds no trace of it yet.
    /// A version already stamped on the medium that differs from
    /// `descriptor.version` rejects with `version-mismatch`; a medium that
    /// cannot be parsed as this unit rejects with `malformed-medium`.
    /// Opening the same unit name twice without closing is a caller bug and
    /// rejects.
    async fn open(&self, descriptor: &KvUnitDescriptor) -> Result<Arc<dyn KvUnit>, StorageError>;
}

/// One registered backend (TS `StorageBackend`).
#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync {
    /// Key-value operations; `None` when this backend cannot serve them.
    fn kv(&self) -> Option<Arc<dyn KvFacet>>;

    /// Drain in-flight writes across all open units and release the medium.
    /// Idempotent; concurrent and repeated calls resolve once teardown
    /// finishes.
    async fn close(&self) -> Result<(), StorageError>;
}

/// Helper for backend implementers: the `closed` rejection.
pub fn closed_error(subject: &str) -> StorageError {
    StorageError::new(StorageErrorCode::Closed, format!("{subject} is closed"))
}

/// Helper for backend implementers: the `version-mismatch` rejection.
pub fn version_mismatch_error(unit: &str, stamped: u64, wanted: u64) -> StorageError {
    StorageError::new(
        StorageErrorCode::VersionMismatch,
        format!("storage unit '{unit}' is stamped v{stamped}, descriptor wants v{wanted}"),
    )
}
