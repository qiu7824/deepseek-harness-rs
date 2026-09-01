//! Browser-safe subagent domain contract. Persisted transcript reads never
//! activate an Agent, while continuable prompts route through the exact
//! live direct parent into the child's Agent inbox. Rust port of
//! `packages/host/apiproxy/src/api/subagents.ts`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use dsh_llm::MessageId;
use dsh_session::SessionId;

use crate::api::rpc::{RpcRequest, RpcResponse};
use crate::api::sessions::{HistoryEntry, SessionProjectionsBlock};
use crate::fetch::handler::AbortSignal;

/// Child activity at the Host sampling boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentActivity {
    Running,
    Inactive,
}

/// Child continuation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentMode {
    OneShot,
    Continuable,
}

/// Diagnostic reason for a non-catalogable child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentDiagnosticReason {
    Corrupt,
    Unsupported,
    Unavailable,
}

/// Complete durable direct-child catalog row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SubagentListEntry {
    Child {
        id: SessionId,
        /// Whether the child Agent driver is running at the Host sampling
        /// boundary.
        activity: SubagentActivity,
        /// Whether a direct descendant has durable `origin: 'subagent'`.
        has_children: bool,
        mode: SubagentMode,
        /// Required for continuable children.
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Diagnostic {
        id: SessionId,
        reason: SubagentDiagnosticReason,
    },
}

/// Inbox identity returned once the continuation accepts one human message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentPromptReceipt {
    pub message_id: MessageId,
}

/// Uniform acknowledgement that one interrupt request was admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentInterruptReceipt {
    pub accepted: bool,
}

/// Durable parent/child address that selects subagent transport in the
/// client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentAddress {
    pub parent_session_id: SessionId,
    pub child_session_id: SessionId,
    pub mode: SubagentMode,
}

/// Complete direct-child catalog plus the delivery-time parent availability
/// hint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentCatalog {
    pub entries: Vec<SubagentListEntry>,
    pub parent_available: bool,
}

/// `subagent.list` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentListRequest {
    pub parent_session_id: SessionId,
}

/// `subagent.history` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentHistoryRequest {
    pub parent_session_id: SessionId,
    pub child_session_id: SessionId,
    pub mode: SubagentMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_messages: Option<u64>,
}

/// `subagent.history` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentHistoryResult {
    pub events: Vec<HistoryEntry>,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projections: Option<SessionProjectionsBlock>,
}

/// `subagent.prompt` request payload (continuable address only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentPromptRequest {
    pub parent_session_id: SessionId,
    pub child_session_id: SessionId,
    pub mode: SubagentMode,
    pub content: Vec<crate::api::sessions::PromptContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_time_zone: Option<String>,
}

/// `subagent.interrupt` request payload (continuable address only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentInterruptRequest {
    pub parent_session_id: SessionId,
    pub child_session_id: SessionId,
    pub mode: SubagentMode,
}

/// Subagent-domain unary methods.
#[async_trait]
pub trait SubagentsApi: Send + Sync {
    /// Lists direct session-backed children without loading either side.
    async fn list(
        &self,
        request: RpcRequest<SubagentListRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<SubagentCatalog>;

    /// Reads one healthy catalog child's transcript without Agent
    /// activation.
    async fn history(
        &self,
        request: RpcRequest<SubagentHistoryRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<SubagentHistoryResult>;

    /// Delivers human content to a continuable child through the exact live
    /// parent's continuation owner.
    async fn prompt(
        &self,
        request: RpcRequest<SubagentPromptRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<SubagentPromptReceipt>;

    /// Interrupts a live continuable child's current turn under the
    /// address's durable direct-parent authority. Fire-and-return.
    async fn interrupt(
        &self,
        request: RpcRequest<SubagentInterruptRequest>,
    ) -> RpcResponse<SubagentInterruptReceipt>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_catalog_entry_uses_client_wire_field_names() {
        let entry = SubagentListEntry::Child {
            id: dsh_session::session_id("child"),
            activity: SubagentActivity::Inactive,
            has_children: true,
            mode: SubagentMode::OneShot,
            label: None,
        };

        let value = serde_json::to_value(entry).expect("serialize child catalog entry");

        assert_eq!(value.get("kind"), Some(&serde_json::json!("child")));
        assert_eq!(value.get("hasChildren"), Some(&serde_json::json!(true)));
        assert!(value.get("has_children").is_none());
    }
}
