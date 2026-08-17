//! `goals` domain contract: mutations only — the read side is the `goal`
//! session projection, so there is no goal.get and no wire goal view.
//! Rust port of `packages/host/apiproxy/src/api/goals.ts`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use dsh_session::SessionId;

use crate::api::rpc::{RpcRequest, RpcResponse};

/// Brand marker for durable goal identities.
#[doc(hidden)]
pub enum GoalIdTag {}

/// Identifies one goal across its durable revisions.
pub type GoalId = dsh_brand::Branded<GoalIdTag>;

/// Compare-and-set identity for one exact goal revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalRef {
    pub id: GoalId,
    pub revision: i64,
}

/// `goal.create` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalCreateRequest {
    pub session_id: SessionId,
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_goal_rounds: Option<u64>,
}

/// `goal.edit` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalEditRequest {
    pub session_id: SessionId,
    #[serde(rename = "ref")]
    pub goal_ref: GoalRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_goal_rounds: Option<u64>,
}

/// `goal.pause` / `goal.resume` / `goal.complete` shared shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalVerbRequest {
    pub session_id: SessionId,
    #[serde(rename = "ref")]
    pub goal_ref: GoalRef,
}

/// `goal.clear` request payload (same shape as the other verbs).
pub type GoalClearRequest = GoalVerbRequest;

/// `goal.create` / `goal.edit` / `goal.pause` / `goal.resume` /
/// `goal.complete` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalRefResult {
    #[serde(rename = "ref")]
    pub goal_ref: GoalRef,
}

/// `goal.clear` response value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalClearResult {
    pub cleared: bool,
}

/// Goal-domain unary methods. Every mutation resolves an ordinary session's
/// Agent and applies one CAS-guarded verb; session-backed subagents reject
/// with `agent-busy`.
#[async_trait]
pub trait GoalsApi: Send + Sync {
    /// Create and arm a goal.
    async fn create(&self, request: RpcRequest<GoalCreateRequest>) -> RpcResponse<GoalRefResult>;

    /// Edit objective and/or round cap without changing phase.
    async fn edit(&self, request: RpcRequest<GoalEditRequest>) -> RpcResponse<GoalRefResult>;

    /// Pause an active goal and disarm automatic continuation.
    async fn pause(&self, request: RpcRequest<GoalVerbRequest>) -> RpcResponse<GoalRefResult>;

    /// Resume and arm a stopped goal.
    async fn resume(&self, request: RpcRequest<GoalVerbRequest>) -> RpcResponse<GoalRefResult>;

    /// Mark a current non-complete goal complete and disarm it.
    async fn complete(&self, request: RpcRequest<GoalVerbRequest>) -> RpcResponse<GoalRefResult>;

    /// Clear the current goal while retaining a durable tombstone and
    /// history.
    async fn clear(&self, request: RpcRequest<GoalClearRequest>) -> RpcResponse<GoalClearResult>;
}
