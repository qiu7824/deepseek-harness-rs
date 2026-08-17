//! `skills` domain contract: read-only skill catalog lookup addressed by
//! session. Rust port of `packages/host/apiproxy/src/api/skills.ts`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use dsh_session::SessionId;

use crate::api::rpc::{RpcRequest, RpcResponse};

/// Skill catalog row (wire projection of the host SkillSummary;
/// provider/source vocabulary stays host-side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    /// Kebab-case identifier the user references as `/name` in the
    /// composer.
    pub name: String,
    /// Short routing description.
    pub description: String,
    /// Optional extra routing guidance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    /// False marks a user-only skill: invocable here, absent from the model
    /// catalog.
    pub model_invocable: bool,
}

/// `skill.list` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillListRequest {
    pub session_id: SessionId,
}

/// `skill.list` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillListResult {
    pub skills: Vec<SkillEntry>,
}

/// Skill-domain unary methods (the map key `skill.*`). Listing is the
/// domain's only RPC: invocation itself is a plain `session.prompt` whose
/// leading `/name` token the host recognizes at the pre-step boundary.
#[async_trait]
pub trait SkillsApi: Send + Sync {
    /// Lists the user-invocable skill catalog for the session's project.
    async fn list(&self, request: RpcRequest<SkillListRequest>) -> RpcResponse<SkillListResult>;
}
