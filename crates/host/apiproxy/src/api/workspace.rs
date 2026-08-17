//! `workspace` domain contract: the workspace view vocabulary and unary
//! methods. Rust port of `packages/host/apiproxy/src/api/workspace.ts` +
//! `workspace.schema.ts`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::api::rpc::{RpcRequest, RpcResponse};

/// Stable workspace identity on the wire.
pub type WorkspaceId = dsh_brand::Branded<WorkspaceIdTag>;

#[doc(hidden)]
pub enum WorkspaceIdTag {}

/// Row of every `workspace.*` response (the TS `WorkspaceView`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    pub workspace_id: WorkspaceId,
    pub path: String,
    pub title: String,
    pub session_ids: Vec<String>,
    /// ISO-shaped timestamps (the domain stores epoch millis; the wire
    /// contract carries strings — the composition layer owns the render).
    pub created_at: String,
    pub updated_at: String,
}

/// `workspace.list` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListResult {
    pub items: Vec<WorkspaceView>,
    pub archived_session_ids: Vec<String>,
}

/// `workspace.create` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceCreateRequest {
    pub path: String,
}

/// `workspace.create` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCreateResult {
    pub workspace: WorkspaceView,
    pub created: bool,
}

/// `workspace.rename` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRenameRequest {
    pub workspace_id: WorkspaceId,
    pub title: String,
}

/// `workspace.rename` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRenameResult {
    pub workspace: WorkspaceView,
}

/// `workspace.delete` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDeleteRequest {
    pub workspace_id: WorkspaceId,
}

/// `workspace.delete` response value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDeleteResult {
    pub deleted: bool,
}

/// `workspace.insertBefore` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInsertBeforeRequest {
    pub workspace_id: WorkspaceId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_workspace_id: Option<WorkspaceId>,
}

/// `workspace.insertBefore` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInsertBeforeResult {
    pub workspace_ids: Vec<String>,
}

/// `workspace.insertSessionBefore` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInsertSessionBeforeRequest {
    pub workspace_id: WorkspaceId,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_session_id: Option<String>,
}

/// `workspace.insertSessionBefore` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInsertSessionBeforeResult {
    pub workspace: WorkspaceView,
}

/// `workspace.archiveSession` / `unarchiveSession` /
/// `deleteArchivedSession` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceArchiveSessionRequest {
    pub session_id: String,
}

/// `workspace.archiveSession` / `unarchiveSession` /
/// `deleteArchivedSession` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceArchiveSessionResult {
    pub archived_session_ids: Vec<String>,
}

/// Workspace-domain unary methods (the map keys `workspace.*`).
#[async_trait]
pub trait WorkspaceApi: Send + Sync {
    async fn list(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<WorkspaceListResult>;

    async fn create(
        &self,
        request: RpcRequest<WorkspaceCreateRequest>,
    ) -> RpcResponse<WorkspaceCreateResult>;

    async fn rename(
        &self,
        request: RpcRequest<WorkspaceRenameRequest>,
    ) -> RpcResponse<WorkspaceRenameResult>;

    async fn delete(
        &self,
        request: RpcRequest<WorkspaceDeleteRequest>,
    ) -> RpcResponse<WorkspaceDeleteResult>;

    async fn insert_before(
        &self,
        request: RpcRequest<WorkspaceInsertBeforeRequest>,
    ) -> RpcResponse<WorkspaceInsertBeforeResult>;

    async fn insert_session_before(
        &self,
        request: RpcRequest<WorkspaceInsertSessionBeforeRequest>,
    ) -> RpcResponse<WorkspaceInsertSessionBeforeResult>;

    async fn archive_session(
        &self,
        request: RpcRequest<WorkspaceArchiveSessionRequest>,
    ) -> RpcResponse<WorkspaceArchiveSessionResult>;

    async fn unarchive_session(
        &self,
        request: RpcRequest<WorkspaceArchiveSessionRequest>,
    ) -> RpcResponse<WorkspaceArchiveSessionResult>;

    async fn delete_archived_session(
        &self,
        request: RpcRequest<WorkspaceArchiveSessionRequest>,
    ) -> RpcResponse<WorkspaceArchiveSessionResult>;
}
