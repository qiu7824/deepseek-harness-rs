//! `sessions` domain contract: unary method signatures are the source of
//! truth — methods take the `RpcRequest<P>` narrow form and the impl echoes
//! rpcId. Rust port of `packages/host/apiproxy/src/api/sessions.ts`
//! (interface + payload entities; the zod schemas map onto these serde
//! types).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use dsh_attachment::{AttachmentId, ImageAttachmentRef, ImageMediaType};
use dsh_llm::MessageId;
use dsh_session::{SessionEvent, SessionId};

use crate::api::events::ToolEventView;
use crate::api::rpc::{RpcRequest, RpcResponse};
use crate::api::workspace::WorkspaceId;
use crate::fetch::handler::AbortSignal;

/// Persisted hints used to summarize a cold Session without reading a large
/// log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListMetadata {
    /// Whether the checkpoint prefix contains no turn/start event.
    pub blank: bool,
    /// Latest source.kind=user message time in the checkpoint prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_prompt_at: Option<i64>,
}

/// One history page entry: the raw event plus the optional host-computed
/// render intent (a pagination-time derivation, never persisted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub event: SessionEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<ToolEventView>,
}

/// The projection baseline riding the history tail page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProjectionsBlock {
    /// Seq of the last event the values reflect; -1 for an empty log.
    pub as_of_seq: i64,
    /// Whole current value per registered projection key (wide: each value
    /// already passed its unit's own schema on the host).
    pub values: serde_json::Value,
}

/// Browser-submitted prompt content; the host promotes image bytes to
/// durable references.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum PromptContentPart {
    Text {
        text: String,
    },
    Image {
        media_type: ImageMediaType,
        /// Base64-encoded image bytes.
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

/// Complete model selection for one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    /// Registered provider route.
    pub provider: String,
    /// Provider-owned model id.
    pub model: String,
    /// Adapter-owned reasoning effort; absence preserves adapter/provider
    /// default behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// One adapter-owned reasoning effort displayed for an exact model route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReasoningEffort {
    /// Opaque value submitted back to the owning adapter.
    pub id: String,
    /// Adapter-supplied display name.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Selectable reasoning metadata for one exact model route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReasoning {
    /// Efforts in adapter-preferred display order.
    pub efforts: Vec<ModelReasoningEffort>,
    /// Adapter-configured default; absence preserves the provider default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

/// One model displayed inside its provider group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogModel {
    /// Provider-owned model id.
    pub id: String,
    /// Provider-supplied display name.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Exact-route reasoning metadata when the adapter exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ModelReasoning>,
}

/// One provider and the models it advertised successfully.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProviderGroup {
    /// Provider route id used for requests.
    pub id: String,
    /// Provider display name.
    pub name: String,
    /// Models in provider-preferred order.
    pub models: Vec<ModelCatalogModel>,
}

/// A provider whose asynchronous catalog lookup failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogFailure {
    /// Provider route id.
    pub id: String,
    /// Provider display name.
    pub name: String,
    /// Lookup failure diagnostic.
    pub message: String,
}

/// Detached model-directory snapshot for one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModels {
    /// Model selection for the session's next assembled step.
    pub current: ModelSelection,
    /// Whether an adapter currently serves `current.provider`, and therefore
    /// whether this session can start a turn at all.
    pub routable: bool,
    /// Successfully loaded provider groups.
    pub groups: Vec<ModelProviderGroup>,
    /// Provider-local failures; successful groups remain usable.
    pub failures: Vec<ModelCatalogFailure>,
}

/// A client-requested mutation of one still-pending queue item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum QueueAction {
    Edit { content: Vec<dsh_llm::ContentBlock> },
    Remove,
    Steer,
}

/// One Session list entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: SessionId,
    /// The later of creation and the latest human-authored prompt.
    pub updated_at: i64,
    /// Status of the attached agent; always false for cold sessions.
    pub running: bool,
    /// Derived conversation-not-started bit: true while no turn has run.
    pub blank: bool,
    /// fork/spawn lineage; absent for root sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    /// Coarse durable origin; never proves resumability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<SessionOrigin>,
    /// Session working directory; absent when unrecorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Agent preset this session's agent was composed from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_preset: Option<String>,
    /// Projection baseline for this row, with zero log loads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projections: Option<SessionProjectionsBlock>,
}

/// `SessionSummary.origin` literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionOrigin {
    Subagent,
}

/// One session-content search result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchItem {
    pub session_id: SessionId,
    /// Plain-text excerpt around the strongest matching visible message.
    pub snippet: String,
}

/// `session.list` request payload (cursor is a reserved seat, v1 ignores
/// it).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `session.list` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListResult {
    pub items: Vec<SessionSummary>,
}

/// `session.search` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSearchRequest {
    pub query: String,
}

/// `session.search` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResult {
    pub items: Vec<SessionSearchItem>,
    pub has_more: bool,
}

/// `session.create` request payload.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreateRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_preset: Option<String>,
}

/// `session.create` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreateResult {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_preset: Option<String>,
}

/// `session.history` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHistoryRequest {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_messages: Option<u64>,
}

/// `session.history` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHistoryResult {
    pub events: Vec<HistoryEntry>,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projections: Option<SessionProjectionsBlock>,
}

/// `session.models` / `session.selectModel` shared session ref.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRefRequest {
    pub session_id: SessionId,
}

/// `session.selectModel` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSelectModelRequest {
    pub session_id: SessionId,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// `session.selectModel` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSelectModelResult {
    pub selected: ModelSelection,
}

/// `session.rename` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRenameRequest {
    pub session_id: SessionId,
    pub title: String,
}

/// `session.rename` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRenameResult {
    pub title: String,
    pub seq: i64,
}

/// `session.fork` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkRequest {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_seq: Option<i64>,
}

/// `session.fork` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkResult {
    pub session_id: SessionId,
}

/// The prompt admission mode (queue → send, steer → steer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    Queue,
    Steer,
}

/// `session.prompt` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptRequest {
    pub session_id: SessionId,
    pub mode: PromptMode,
    pub content: Vec<PromptContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_time_zone: Option<String>,
}

/// The command slot of a successful slash-command prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptCommandSlot {
    pub kind: PromptCommandKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptCommandKind {
    Success,
}

/// `session.prompt` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptResult {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<PromptCommandSlot>,
}

/// `session.attachment` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAttachmentRequest {
    pub session_id: SessionId,
    pub attachment_id: AttachmentId,
}

/// `session.attachment` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAttachmentResult {
    pub attachment: ImageAttachmentRef,
    /// Base64-encoded image bytes.
    pub data: String,
}

/// `session.updateQueue` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdateQueueRequest {
    pub session_id: SessionId,
    pub item_id: MessageId,
    pub action: QueueAction,
}

/// Generic `{ accepted: true }` response value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedResult {
    pub accepted: bool,
}

/// Session-domain unary methods (the map keys `session.*` of
/// `RpcMethodMap`).
#[async_trait]
pub trait SessionsApi: Send + Sync {
    /// Lists persisted sessions (updatedAt descending). v1 returns
    /// everything; cursor is a reserved seat, unimplemented.
    async fn list(&self, request: RpcRequest<SessionListRequest>)
    -> RpcResponse<SessionListResult>;

    /// Searches the current message surface across sessions visible to
    /// `list`. Results contain at most 20 sessions and carry no
    /// continuation cursor.
    async fn search(
        &self,
        request: RpcRequest<SessionSearchRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<SessionSearchResult>;

    /// Creates a real session and its idle agent.
    async fn create(
        &self,
        request: RpcRequest<SessionCreateRequest>,
    ) -> RpcResponse<SessionCreateResult>;

    /// Reads a window of history events; page boundaries align to
    /// append-origin message boundaries.
    async fn history(
        &self,
        request: RpcRequest<SessionHistoryRequest>,
    ) -> RpcResponse<SessionHistoryResult>;

    /// Reads a fresh advisory model directory for an ordinary session.
    async fn models(&self, request: RpcRequest<SessionRefRequest>) -> RpcResponse<SessionModels>;

    /// Selects the complete model selection for this session.
    async fn select_model(
        &self,
        request: RpcRequest<SessionSelectModelRequest>,
    ) -> RpcResponse<SessionSelectModelResult>;

    /// Renames a session: appends a `session/title` event with the `user`
    /// source.
    async fn rename(
        &self,
        request: RpcRequest<SessionRenameRequest>,
    ) -> RpcResponse<SessionRenameResult>;

    /// Forks a new session from a completed-turn prefix of the source.
    async fn fork(&self, request: RpcRequest<SessionForkRequest>)
    -> RpcResponse<SessionForkResult>;

    /// Sends text and temporary image bytes to an ordinary session Agent
    /// after durable host admission.
    async fn prompt(
        &self,
        request: RpcRequest<SessionPromptRequest>,
    ) -> RpcResponse<SessionPromptResult>;

    /// Reads one durable image after proving that this session's log
    /// references its id.
    async fn attachment(
        &self,
        request: RpcRequest<SessionAttachmentRequest>,
    ) -> RpcResponse<SessionAttachmentResult>;

    /// Edits, removes, or strictly steers one pending queued occurrence.
    async fn update_queue(
        &self,
        request: RpcRequest<SessionUpdateQueueRequest>,
    ) -> RpcResponse<AcceptedResult>;

    /// Stops an ordinary session's active turn, preserving pending inbox
    /// work.
    async fn cancel(&self, request: RpcRequest<SessionRefRequest>) -> RpcResponse<AcceptedResult>;
}
