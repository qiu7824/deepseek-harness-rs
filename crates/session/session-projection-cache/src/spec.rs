//! The session-projcache domain declaration. Rust port of
//! `packages/session/session-projection-cache/src/spec.ts`: one `sessions`
//! table keyed by session id, each record the full projection checkpoint
//! for one session.

use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use dsh_session::SessionId;
use dsh_session_projection::ProjectionCheckpoint;
use dsh_storage_domain::{DomainSpec, define_domain, domain_table};

/// One persisted checkpoint row (TS `checkpointRow`): `ver` selects the
/// unit's contract, `seq` is the fold watermark (-1 = empty log), `val` is
/// the unit's plain-JSON state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointRow {
    pub ver: u64,
    pub seq: i64,
    pub val: JsonValue,
}

/// The stored-log identity a record is bound to (TS
/// `checkpointIdentity`): the immutable header fields that distinguish one
/// session lifecycle from another under the same id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointIdentity {
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// One session's stored record (TS `checkpointRecord`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointRecord {
    pub identity: CheckpointIdentity,
    #[serde(default)]
    pub rows: IndexMap<String, CheckpointRow>,
}

/// The durable-boundary validator for the `sessions` table (the TS zod
/// `checkpointRecord.parse` translated).
pub fn checkpoint_record_schema() -> dsh_storage_domain::RecordSchema {
    Arc::new(|value: &JsonValue| {
        let object = value
            .as_object()
            .ok_or_else(|| "expected a checkpoint record object".to_string())?;
        let identity = object
            .get("identity")
            .and_then(|identity| identity.as_object())
            .ok_or_else(|| "checkpoint record lacks an identity object".to_string())?;
        let created_at = identity
            .get("createdAt")
            .and_then(|created_at| created_at.as_u64())
            .ok_or_else(|| "checkpoint identity createdAt must be a non-negative integer".to_string())?;
        let _ = created_at;
        if let Some(cwd) = identity.get("cwd") {
            if !cwd.is_string() {
                return Err("checkpoint identity cwd must be a string".to_string());
            }
        }
        let rows = object
            .get("rows")
            .and_then(|rows| rows.as_object())
            .ok_or_else(|| "checkpoint record lacks a rows object".to_string())?;
        for (key, row) in rows {
            let row = row
                .as_object()
                .ok_or_else(|| format!("checkpoint row '{key}' must be an object"))?;
            if row.get("ver").and_then(|ver| ver.as_u64()).is_none() {
                return Err(format!(
                    "checkpoint row '{key}' ver must be a non-negative integer"
                ));
            }
            if row.get("seq").and_then(|seq| seq.as_i64()).is_none() {
                return Err(format!(
                    "checkpoint row '{key}' seq must be an integer >= -1"
                ));
            }
            if !row.contains_key("val") {
                return Err(format!("checkpoint row '{key}' lacks val"));
            }
        }
        Ok(())
    })
}

/// The session-projcache domain spec (TS `projectionCacheDomainSpec`).
pub fn projection_cache_domain_spec() -> DomainSpec {
    define_domain(
        "session_projcache",
        3,
        None,
        IndexMap::from([("sessions".to_string(), domain_table(checkpoint_record_schema()))]),
    )
    .expect("session_projcache spec is valid")
}

/// Keep the TS phantom key type visible at the call sites.
pub type CheckpointTableKey = SessionId;

/// The rows a cold read starts from (the TS inline `record?.rows ?? {}`).
pub fn rows_of(record: Option<&CheckpointRecord>) -> ProjectionCheckpoint {
    let mut rows = ProjectionCheckpoint::new();
    if let Some(record) = record {
        for (key, row) in &record.rows {
            rows.insert(
                key.clone(),
                dsh_session_projection::ProjectionCheckpointRow {
                    ver: row.ver,
                    seq: row.seq,
                    val: row.val.clone(),
                },
            );
        }
    }
    rows
}
