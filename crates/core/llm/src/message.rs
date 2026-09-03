//! Message value types, identity, and immutable construction helpers.
//! Rust port of `packages/llm/llm/src/message.ts`.
//!
//! Rust values are owned, so `deepFreeze`/`structuredClone` collapse to the
//! identity function: the detached-immutable contract is the type system's
//! default. `create*` helpers still mint fresh UUID identities to match the
//! TS runtime behavior.

use crate::brand::{CallId, MessageId};
use crate::types::ContentBlock;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Provider/model identity and adapter-private replay data for an assistant
/// message (TS `AssistantProvenance` / `ModelMessageSource`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelMessageSource {
    /// Provider route that produced the message.
    pub provider: String,
    /// Provider model id that produced the message.
    pub model: String,
    /// Lossless-JSON adapter state needed to replay the provider response.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "replayState"
    )]
    pub replay_state: Option<JsonValue>,
}

/// Required source of a user-role message carrying one tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolMessageSource {
    #[serde(rename = "callId")]
    pub call_id: CallId,
}

/// Producer-declared context form (TS `ContextForm`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextForm {
    Instructions,
    Catalog,
    Snapshot,
    Notice,
    Relay,
    Recall,
}

/// One durable workspace-instruction scope transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInstructionChange {
    pub action: String,
    pub scope: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// One named contribution to a `snapshot`-form context, in assembly order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSnapshotSection {
    /// The contributing subsystem's name.
    pub name: String,
    /// That contribution's model-facing text, exactly as assembled.
    pub text: String,
}

/// One durable entry of a published session skill catalog (TS
/// `SkillCatalogSource['entries']`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillCatalogEntry {
    /// Kebab-case skill name.
    pub name: String,
    /// Normalized, length-bounded catalog description.
    pub description: String,
}

/// Where a message (or injected content) came from. Merge-extensible sum
/// type in TS; Rust models the four core `kind`s. Plugin form fields are
/// carried as lenient optionals (the TS discriminated `form` union).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MessageSource {
    User {
        /// Client RPC correlation of the producing request (the TS sum type
        /// is merge-augmented by the host apiproxy package).
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "rpcId")]
        rpc_id: Option<String>,
        /// Host-canonicalized browser IANA zone of the producing request.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "clientTimeZone"
        )]
        client_time_zone: Option<String>,
    },
    Plugin {
        plugin: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        form: Option<ContextForm>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sections: Option<Vec<ContextSnapshotSection>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// Compaction transaction identity (the TS compaction checkpoint
        /// augmentation on the `plugin` source kind).
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "compactionId"
        )]
        compaction_id: Option<String>,
        /// Initiating manual command identity (the TS compaction checkpoint
        /// augmentation).
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "sourceCommandId"
        )]
        source_command_id: Option<String>,
    },
    Model {
        /// Provider route that produced the message.
        provider: String,
        /// Provider model id that produced the message.
        model: String,
        /// Lossless-JSON adapter state needed to replay the provider response.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "replayState"
        )]
        replay_state: Option<JsonValue>,
    },
    Tool {
        #[serde(rename = "callId")]
        call_id: CallId,
    },
    /// Automatic same-session goal continuation attribution. Goal-id
    /// branding and positive-number validation belong to dsh-goal, avoiding
    /// a reverse dependency from the core message vocabulary.
    Goal {
        #[serde(rename = "goalId")]
        goal_id: String,
        revision: u64,
        round: u64,
    },
    /// Durable record of a published session skill catalog (TS
    /// `skill-catalog` source augmentation on the model-facing `skill`
    /// loader).
    SkillCatalog {
        form: ContextForm,
        /// Marks a replacement catalog rather than this session's first
        /// publication.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        update: Option<bool>,
        /// Exactly the entries this message published, in catalog order.
        entries: Vec<SkillCatalogEntry>,
    },
    /// A user-explicit skill invocation injected by the host (TS
    /// `skill-invocation` source augmentation).
    SkillInvocation {
        /// Invoked skill name, validated user-invocable at the injecting
        /// boundary.
        name: String,
        /// Injected skill bodies are instructions for the model to follow.
        form: ContextForm,
    },
    /// Workspace instruction baseline or update (`AGENTS.md` / `CLAUDE.md`).
    AgentInstructions {
        form: ContextForm,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        baseline: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "baselineIdentity"
        )]
        baseline_identity: Option<String>,
        changes: Vec<AgentInstructionChange>,
    },
    /// Durable attribution for one model-authored message between adjacent
    /// Agents.
    AgentMessage {
        form: ContextForm,
        #[serde(rename = "senderSessionId")]
        sender_session_id: String,
    },
    /// Durable attribution for the runtime's account of a continuable child
    /// settling (TS `subagent-settled` source augmentation).
    SubagentSettled {
        form: ContextForm,
        summary: String,
        #[serde(rename = "senderSessionId")]
        sender_session_id: String,
    },
}

impl MessageSource {
    pub fn kind(&self) -> &'static str {
        match self {
            MessageSource::User { .. } => "user",
            MessageSource::Plugin { .. } => "plugin",
            MessageSource::Model { .. } => "model",
            MessageSource::Tool { .. } => "tool",
            MessageSource::Goal { .. } => "goal",
            MessageSource::SkillCatalog { .. } => "skill-catalog",
            MessageSource::SkillInvocation { .. } => "skill-invocation",
            MessageSource::AgentInstructions { .. } => "agent-instructions",
            MessageSource::AgentMessage { .. } => "agent-message",
            MessageSource::SubagentSettled { .. } => "subagent-settled",
        }
    }
}

/// Provider-neutral conversation role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// One immutable message representation shared by delivery, durable history,
/// and model requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Stable identity preserved across every representation boundary.
    pub id: MessageId,
    /// Provider-neutral conversation role.
    pub role: Role,
    /// Exact model-facing blocks.
    pub content: Vec<ContentBlock>,
    /// Required source fields supplied by the producer.
    pub source: MessageSource,
}

/// A user-role specialization of the one shared message representation.
pub type UserMessage = Message;
/// A model-produced assistant specialization of the shared message
/// representation.
pub type AssistantMessage = Message;
/// A tool-result specialization whose model-facing block retains call
/// correlation.
pub type ToolResultMessage = Message;

/// Bound for a `notice` summary (TS `CONTEXT_SUMMARY_MAX_CHARS`).
pub const CONTEXT_SUMMARY_MAX_CHARS: usize = 120;

/// Bound one `notice` summary to [`CONTEXT_SUMMARY_MAX_CHARS`].
pub fn bound_context_summary(summary: &str) -> String {
    if summary.chars().count() <= CONTEXT_SUMMARY_MAX_CHARS {
        return summary.to_string();
    }
    let truncated: String = summary
        .chars()
        .take(CONTEXT_SUMMARY_MAX_CHARS - 1)
        .collect();
    format!("{truncated}…")
}

/// Detach and deep-freeze a message whose identity already exists. Rust
/// values are owned, so this is the identity function (documented collapse
/// of the TS `structuredClone` + `deepFreeze`).
pub fn freeze_message<T>(message: T) -> T {
    message
}

/// Create one identified message and freeze it before publication.
pub fn create_message(role: Role, content: Vec<ContentBlock>, source: MessageSource) -> Message {
    Message {
        id: crate::brand::message_id(uuid_v4()),
        role,
        content,
        source,
    }
}

/// Create one identified user-role message.
pub fn create_user_message(content: Vec<ContentBlock>, source: MessageSource) -> UserMessage {
    create_message(Role::User, content, source)
}

/// Create one identified model-produced assistant message.
pub fn create_assistant_message(
    content: Vec<ContentBlock>,
    source: ModelMessageSource,
) -> AssistantMessage {
    create_message(
        Role::Assistant,
        content,
        MessageSource::Model {
            provider: source.provider,
            model: source.model,
            replay_state: source.replay_state,
        },
    )
}

/// Input whose acceptance creates one tool-result message.
#[derive(Debug, Clone)]
pub struct ToolResultMessageInput {
    pub call_id: CallId,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
}

/// Create and freeze one identified tool-result message.
pub fn create_tool_result_message(input: ToolResultMessageInput) -> ToolResultMessage {
    create_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_call_id: input.call_id.clone(),
            content: input.content,
            is_error: Some(input.is_error),
        }],
        MessageSource::Tool {
            call_id: input.call_id,
        },
    )
}

/// Whether a stream chunk carries visible model output (the first-token
/// boundary shared by client step timing and whole-log session stats).
pub fn is_token_delta(chunk: &crate::types::StreamChunk) -> bool {
    match chunk {
        crate::types::StreamChunk::TextDelta { text, .. }
        | crate::types::StreamChunk::ReasoningDelta { text, .. } => !text.is_empty(),
        crate::types::StreamChunk::ToolCallDelta {
            arguments_delta,
            name,
            ..
        } => !arguments_delta.is_empty() || name.is_some(),
        _ => false,
    }
}

fn uuid_v4() -> String {
    // The TS runtime uses `crypto.randomUUID()`. The workspace already
    // depends on `uuid` for the v4 feature in later milestones; keep the
    // dependency local for now with a counter-free pure-Rust v4 fallback.
    uuid::Uuid::new_v4().to_string()
}
