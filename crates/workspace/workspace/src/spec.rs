//! The workspace domain declaration. Rust port of
//! `packages/workspace/workspace/src/spec.ts`: record schema and the
//! `defineDomain` spec the registry opens. The zod schema collapses into a
//! serde record type plus a JSON validation closure at the durable
//! boundary.

use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use dsh_session::SessionId;
use dsh_storage_domain::{DomainSpec, define_domain, domain_global, domain_table};

use crate::types::WorkspaceId;

/// Durable shape of one workspace record (TS `workspaceRecord`). `path` is
/// the `fs.realpath` canon stamped at create; `sessionIds` is the ordered
/// ownership account; timestamps are ISO-8601 strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    pub path: String,
    pub title: String,
    #[serde(rename = "sessionIds")]
    pub session_ids: Vec<SessionId>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// Recoverable two-write mutation marker (TS `workspacePendingMutation`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "lowercase")]
pub enum WorkspacePendingMutation {
    Create {
        #[serde(rename = "workspaceId")]
        workspace_id: WorkspaceId,
    },
    Delete {
        #[serde(rename = "workspaceId")]
        workspace_id: WorkspaceId,
    },
}

/// Durable registry state (TS `workspaceDomainState`). `initialized`
/// distinguishes a valid empty registry from one that still needs the
/// header-only history bootstrap; `workspaceIds` is the authoritative
/// display order. `archivedSessionIds` defaults so records written before
/// the field parse unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceDomainState {
    pub initialized: bool,
    #[serde(rename = "workspaceIds")]
    pub workspace_ids: Vec<WorkspaceId>,
    #[serde(rename = "archivedSessionIds", default)]
    pub archived_session_ids: Vec<SessionId>,
    #[serde(
        rename = "pendingMutation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub pending_mutation: Option<WorkspacePendingMutation>,
}

impl WorkspaceDomainState {
    pub fn initial() -> Self {
        Self {
            initialized: false,
            workspace_ids: Vec::new(),
            archived_session_ids: Vec::new(),
            pending_mutation: None,
        }
    }
}

/// The durable-boundary validator for the `workspaces` table (the TS zod
/// `workspaceRecord.parse` translated).
pub fn workspace_record_schema() -> dsh_storage_domain::RecordSchema {
    Arc::new(|value: &JsonValue| {
        let object = value
            .as_object()
            .ok_or_else(|| "expected a workspace record object".to_string())?;
        for field in ["path", "title", "createdAt", "updatedAt"] {
            if !object.get(field).is_some_and(|value| value.is_string()) {
                return Err(format!("workspace record {field} must be a string"));
            }
        }
        let session_ids = object
            .get("sessionIds")
            .and_then(|value| value.as_array())
            .ok_or_else(|| "workspace record sessionIds must be an array".to_string())?;
        if !session_ids.iter().all(|id| id.is_string()) {
            return Err("workspace record sessionIds entries must be strings".to_string());
        }
        Ok(())
    })
}

/// The durable-boundary validator for the global slot (the TS zod
/// `workspaceDomainState.parse` translated).
pub fn workspace_domain_state_schema() -> dsh_storage_domain::RecordSchema {
    Arc::new(|value: &JsonValue| {
        let object = value
            .as_object()
            .ok_or_else(|| "expected a workspace domain state object".to_string())?;
        if !object
            .get("initialized")
            .is_some_and(|value| value.as_bool().is_some())
        {
            return Err("workspace domain state initialized must be a boolean".to_string());
        }
        let ids = object
            .get("workspaceIds")
            .and_then(|value| value.as_array())
            .ok_or_else(|| "workspace domain state workspaceIds must be an array".to_string())?;
        if !ids.iter().all(|id| id.is_string()) {
            return Err("workspace domain state workspaceIds entries must be strings".to_string());
        }
        let archived = object
            .get("archivedSessionIds")
            .cloned()
            .unwrap_or(JsonValue::Null);
        if !archived.is_null()
            && !archived
                .as_array()
                .is_some_and(|ids| ids.iter().all(|id| id.is_string()))
        {
            return Err(
                "workspace domain state archivedSessionIds must be an array of strings".to_string(),
            );
        }
        if let Some(pending) = object.get("pendingMutation") {
            let pending = pending
                .as_object()
                .ok_or_else(|| "workspace pendingMutation must be an object".to_string())?;
            let operation = pending.get("operation").and_then(|op| op.as_str());
            let workspace_id = pending.get("workspaceId").is_some_and(|id| id.is_string());
            if !matches!(operation, Some("create") | Some("delete")) || !workspace_id {
                return Err("workspace pendingMutation shape is invalid".to_string());
            }
        }
        Ok(())
    })
}

/// The workspace domain spec (TS `workspaceDomainSpec`): one `workspaces`
/// table plus the bootstrap/order singleton.
pub fn workspace_domain_spec() -> DomainSpec {
    define_domain(
        "workspace",
        2,
        Some(domain_global(
            workspace_domain_state_schema(),
            serde_json::to_value(WorkspaceDomainState::initial()).expect("initial state"),
        )),
        IndexMap::from([(
            "workspaces".to_string(),
            domain_table(workspace_record_schema()),
        )]),
    )
    .expect("workspace spec is valid")
}

/// Read one record from a raw stored JSON value.
pub fn record_from_value(value: &JsonValue) -> Result<WorkspaceRecord, String> {
    serde_json::from_value(value.clone())
        .map_err(|error| format!("stored workspace record is malformed: {error}"))
}

/// Read the domain state from a raw stored JSON value.
pub fn state_from_value(value: &JsonValue) -> Result<WorkspaceDomainState, String> {
    serde_json::from_value(value.clone())
        .map_err(|error| format!("stored workspace domain state is malformed: {error}"))
}
