//! `agent-presets` domain contract: the roster a browser offers when
//! starting a session, plus the authoring calls behind it. The authoring
//! calls are privileged and loopback-pinned. Rust port of
//! `packages/host/apiproxy/src/api/agent-presets.ts`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use dsh_session::SessionId;

use crate::api::rpc::{RpcRequest, RpcResponse};
use crate::fetch::handler::AbortSignal;

/// Preset provenance/trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentPresetTrust {
    System,
    User,
}

/// One preset the deployment can compose a session's agent from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetEntry {
    /// Stable identifier, also the display name until presets carry
    /// metadata.
    pub id: String,
    /// Whether the preset ships with the deployment or was authored
    /// locally.
    pub trust: AgentPresetTrust,
    /// Whether a session that names no preset gets this one.
    pub is_default: bool,
    /// Display name the preset published, absent when it published none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// One sentence on what the preset is for, when it published one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Why this preset cannot compose a session, absent when it can.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broken: Option<String>,
}

/// `agentPreset.list` response value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetListResult {
    pub presets: Vec<AgentPresetEntry>,
    pub authorable: bool,
    pub has_document: bool,
}

/// `agentPreset.select` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetSelectRequest {
    pub session_id: SessionId,
    pub agent_preset: String,
}

/// `agentPreset.select` / `agentPreset.copy` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetSelectResult {
    pub agent_preset: String,
}

/// `agentPreset.read` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetReadRequest {
    pub agent_preset: String,
}

/// `agentPreset.read` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetReadResult {
    pub agent_preset: String,
    pub trust: AgentPresetTrust,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `agentPreset.copy` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetCopyRequest {
    pub from: String,
    pub agent_preset: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `agentPreset.openDocument` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetOpenDocumentRequest {
    pub agent_preset: String,
}

/// `agentPreset.openDocument` response value: opened, or the resolved
/// directory for the surface to show as text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetOpenDocumentResult {
    pub opened: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `agentPreset.remove` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetRemoveRequest {
    pub agent_preset: String,
}

/// Agent-preset-domain unary methods (the map key `agentPreset.*`).
#[async_trait]
pub trait AgentPresetsApi: Send + Sync {
    /// Lists every preset the deployment currently supplies, in
    /// root-precedence order.
    async fn list(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<AgentPresetListResult>;

    /// Recompose one session's agent from a different preset. Allowed only
    /// while the session is blank.
    async fn select(
        &self,
        request: RpcRequest<AgentPresetSelectRequest>,
    ) -> RpcResponse<AgentPresetSelectResult>;

    /// Read one preset's composition text, for the read-only viewer.
    async fn read(
        &self,
        request: RpcRequest<AgentPresetReadRequest>,
    ) -> RpcResponse<AgentPresetReadResult>;

    /// Create a locally authored preset by copying an existing one whole
    /// (the only authoring write).
    async fn copy(
        &self,
        request: RpcRequest<AgentPresetCopyRequest>,
    ) -> RpcResponse<AgentPresetSelectResult>;

    /// Hand one locally authored preset's DIRECTORY to the platform opener.
    /// Shipped presets are refused.
    async fn open_document(
        &self,
        request: RpcRequest<AgentPresetOpenDocumentRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<AgentPresetOpenDocumentResult>;

    /// Delete a locally authored preset. Shipped presets are refused.
    async fn remove(
        &self,
        request: RpcRequest<AgentPresetRemoveRequest>,
    ) -> RpcResponse<serde_json::Value>;
}
