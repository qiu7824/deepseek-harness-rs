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
use dsh_storage_domain::{
    DomainSpec, InvalidRecordPolicy, KvLayout, define_domain_with_options, domain_table,
};

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
    #[serde(rename = "isSeeded", default)]
    pub is_seeded: bool,
    #[serde(rename = "inheritedEventCount", default)]
    pub inherited_event_count: u64,
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
            .ok_or_else(|| {
                "checkpoint identity createdAt must be a non-negative integer".to_string()
            })?;
        let _ = created_at;
        if let Some(cwd) = identity.get("cwd")
            && !cwd.is_string()
        {
            return Err("checkpoint identity cwd must be a string".to_string());
        }
        if let Some(is_seeded) = identity.get("isSeeded")
            && !is_seeded.is_boolean()
        {
            return Err("checkpoint identity isSeeded must be a boolean".to_string());
        }
        if let Some(count) = identity.get("inheritedEventCount")
            && count.as_u64().is_none()
        {
            return Err(
                "checkpoint identity inheritedEventCount must be a non-negative integer"
                    .to_string(),
            );
        }
        if !identity
            .get("isSeeded")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false)
            && identity
                .get("inheritedEventCount")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0)
                != 0
        {
            return Err("unseeded checkpoint identity must not inherit events".to_string());
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
            if row
                .get("seq")
                .and_then(|seq| seq.as_i64())
                .is_none_or(|seq| seq < -1)
            {
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
    define_domain_with_options(
        "session_projcache",
        5,
        KvLayout::PerRecord,
        vec![3, 4],
        InvalidRecordPolicy::BackupAndSkip,
        None,
        IndexMap::from([(
            "sessions".to_string(),
            domain_table(checkpoint_record_schema()),
        )]),
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

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_storage_domain::{InvalidRecordPolicy, KvLayout};

    #[test]
    fn projection_cache_uses_disposable_v5_per_record_domain() {
        let spec = projection_cache_domain_spec();

        assert_eq!(spec.version, 5);
        assert_eq!(spec.layout, KvLayout::PerRecord);
        assert_eq!(spec.compatible_versions, vec![3, 4]);
        assert_eq!(spec.invalid_records, InvalidRecordPolicy::BackupAndSkip);
    }

    #[test]
    fn checkpoint_schema_rejects_sequence_below_empty_log_sentinel() {
        let schema = checkpoint_record_schema();
        let record = serde_json::json!({
            "identity": {"createdAt": 1},
            "rows": {"title": {"ver": 1, "seq": -2, "val": null}}
        });

        let error = schema(&record).expect_err("sequence below -1 must reject");
        assert!(error.contains("integer >= -1"));
    }

    #[test]
    fn legacy_identity_normalizes_without_reusing_seeded_lineage() {
        let value = serde_json::json!({"identity":{"createdAt":7,"cwd":"workspace"},"rows":{}});
        checkpoint_record_schema()(&value).unwrap();
        let legacy: CheckpointRecord = serde_json::from_value(value).unwrap();
        assert!(!legacy.identity.is_seeded);
        assert_eq!(legacy.identity.inherited_event_count, 0);
        let seeded = CheckpointIdentity {
            is_seeded: true,
            inherited_event_count: 12,
            ..legacy.identity.clone()
        };
        assert!(!crate::index::identity_matches(&legacy.identity, &seeded));
        let encoded = serde_json::to_value(legacy).unwrap();
        assert_eq!(encoded["identity"]["isSeeded"], false);
        assert_eq!(encoded["identity"]["inheritedEventCount"], 0);
    }

    #[test]
    fn impossible_unseeded_lineage_is_invalid() {
        let value = serde_json::json!({"identity":{"createdAt":7,"isSeeded":false,"inheritedEventCount":12},"rows":{}});
        assert!(checkpoint_record_schema()(&value).is_err());
    }
}
