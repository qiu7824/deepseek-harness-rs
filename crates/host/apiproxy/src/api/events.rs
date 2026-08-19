//! `events` domain contract: signatures and frame unions for the two
//! logical streams. Four-quadrant: streams yield the narrow form
//! `RpcRequest<Frame>` (server-request view) — rpcId must be exposed to the
//! business layer, because responses to answerable frames
//! (approval/question requested) echo it; for pure pushes it identifies
//! that one push. The signal is a local stream-control parameter,
//! independent of the request (never on the wire).
//!
//! Rust port of `packages/host/apiproxy/src/api/events.ts` +
//! `events.schema.ts`.
//!
//! # Deviations
//!
//! - `session/event` frames carry the domain crate's `SessionEvent`
//!   serde form (the TS schema's strict-envelope + wide-data passthrough
//!   branch), so the second parse is the domain type's own deserializer.
//! - `question/requested` frames reference the domain crate's
//!   `AskUserQuestionItem`, whose intent `kind` is a plain string (the TS
//!   wire rejects unknown tags; the host validated the tagged union at
//!   construction).

use std::pin::Pin;

use dsh_llm::{CallId, Message, MessageId};
use dsh_session::{SessionEvent, SessionId};
use dsh_user_approval::{ApprovalOutcome, ApprovalRequestId};
use dsh_user_questions::AskUserQuestionItem;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::api::jobs::JobView;
use crate::api::rpc::{RpcError, RpcId, RpcRequest};
use crate::api::workspace::WorkspaceView;
use crate::fetch::handler::AbortSignal;

/// Host-computed render intent accompanying a `tool/call` or `tool/result`
/// event. A pure derivation of args/result through the presenter registered
/// at emission time — never persisted (the session log carries only the
/// event). `for` names which vocabulary applies without re-inspecting the
/// event type. An absent view means the client's documented default
/// (generic JSON card).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "for")]
pub enum ToolEventView {
    #[serde(rename = "call")]
    Call { view: dsh_tools::ToolCallView },
    #[serde(rename = "result")]
    Result { view: dsh_tools::ToolResultView },
}

/// One pending inbox occurrence in the authoritative `session/queue`
/// snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedInboxItem {
    /// Message identity used by inbox mutations.
    pub id: MessageId,
    /// Agent-resolved FIFO placement; queued and steering items render on
    /// different surfaces, context items stay invisible until claimed.
    pub placement: QueuedInboxPlacement,
    /// Complete pending message; it is not durable until the Agent claims
    /// it.
    pub message: Message,
}

/// The inbox placement vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueuedInboxPlacement {
    Queued,
    Steering,
    Context,
}

/// Streaming face of the contract: the two logical stream openers
/// (mux + host).
pub trait EventsApi: Send + Sync {
    /// All-session aggregated mux stream. On open, emits a subscribed
    /// control frame for every attached session, then replays each
    /// session's still-pending approval/question requested frames (rpcId
    /// reused verbatim — the refresh-recovery baseline).
    fn mux(
        &self,
        request: RpcRequest<serde_json::Value>,
        signal: AbortSignal,
    ) -> Pin<Box<dyn Stream<Item = RpcRequest<MuxFrame>> + Send>>;

    /// Host-level info stream: session create/destroy, running-status
    /// flips, and agent failures with no turn position. Empty payload uses
    /// `{}`.
    fn host(
        &self,
        request: RpcRequest<serde_json::Value>,
        signal: AbortSignal,
    ) -> Pin<Box<dyn Stream<Item = RpcRequest<HostFrame>> + Send>>;
}

/// Mux stream frames: raw session-event passthrough + control frames +
/// approval/question frames (requested = answerable server-request, the
/// rest are pure pushes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum MuxFrame {
    #[serde(rename = "session/event")]
    SessionEventFrame {
        session_id: SessionId,
        event: SessionEvent,
        #[serde(skip_serializing_if = "Option::is_none")]
        view: Option<ToolEventView>,
    },
    #[serde(rename = "session/subscribed")]
    SessionSubscribed {
        session_id: SessionId,
        last_seq: i64,
    },
    #[serde(rename = "approval/requested")]
    ApprovalRequested {
        session_id: SessionId,
        approval_id: ApprovalRequestId,
        tool_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<CallId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename = "approval/resolved")]
    ApprovalResolved {
        session_id: SessionId,
        approval_id: ApprovalRequestId,
        outcome: ApprovalOutcome,
    },
    #[serde(rename = "question/requested")]
    QuestionRequested {
        session_id: SessionId,
        /// Non-empty by wire contract: the user-questions service rejects
        /// empty batches at ask().
        questions: Vec<AskUserQuestionItem>,
    },
    #[serde(rename = "question/resolved")]
    QuestionResolved {
        session_id: SessionId,
        question_rpc_id: RpcId,
        outcome: QuestionOutcome,
    },
    /// Complete transient inbox state after every enqueue, mutation, claim,
    /// or discard (the whole snapshot makes edit, deletion, cancel, and
    /// reconnect converge through one authoritative signal).
    #[serde(rename = "session/queue")]
    SessionQueue {
        session_id: SessionId,
        items: Vec<QueuedInboxItem>,
    },
    /// Complete set of background jobs this session can see, after every
    /// registry commit that changes it.
    #[serde(rename = "session/jobs")]
    SessionJobs {
        session_id: SessionId,
        jobs: Vec<JobView>,
    },
    /// One projection unit's finished value changed. `value` is the unit's
    /// schema-validated view output; `seq` is the unit's watermark at
    /// emission.
    #[serde(rename = "session/projection")]
    SessionProjection {
        session_id: SessionId,
        key: String,
        value: serde_json::Value,
        seq: i64,
    },
    #[serde(rename = "stream/error")]
    StreamError { error: RpcError },
}

/// `question/resolved` outcome vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionOutcome {
    Answered,
    Cancelled,
}

/// Host stream frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum HostFrame {
    #[serde(rename = "host/session-added")]
    SessionAdded {
        session_id: SessionId,
        blank: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_session_id: Option<SessionId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        origin: Option<HostSessionOrigin>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_preset: Option<String>,
    },
    #[serde(rename = "host/session-removed")]
    SessionRemoved { session_id: SessionId },
    #[serde(rename = "host/session-status")]
    SessionStatus {
        session_id: SessionId,
        running: bool,
    },
    #[serde(rename = "host/agent-error")]
    AgentError {
        session_id: SessionId,
        message: String,
    },
    #[serde(rename = "host/workspace-changed")]
    WorkspaceChanged { workspace: WorkspaceView },
    #[serde(rename = "host/workspace-removed")]
    WorkspaceRemoved {
        workspace_id: crate::api::workspace::WorkspaceId,
    },
    #[serde(rename = "host/workspace-order-changed")]
    WorkspaceOrderChanged {
        workspace_ids: Vec<crate::api::workspace::WorkspaceId>,
    },
    #[serde(rename = "host/archived-sessions-changed")]
    ArchivedSessionsChanged {
        archived_session_ids: Vec<SessionId>,
    },
    /// One allowlisted host cordis event forwarded verbatim; no projection,
    /// no redaction, no renaming.
    #[serde(rename = "host/remote-event")]
    RemoteEvent {
        event: String,
        args: Vec<serde_json::Value>,
    },
    #[serde(rename = "stream/error")]
    StreamError { error: RpcError },
}

/// `host/session-added` origin literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostSessionOrigin {
    Subagent,
}
