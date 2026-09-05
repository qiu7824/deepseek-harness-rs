//! Canonical provider-neutral message and streaming vocabulary: Rust port of
//! `packages/llm/llm/src/types.ts` (type layer only; the `LlmRuntime`
//! service and adapters belong to a later milestone).
//!
//! Wire shapes are byte-compatible with the TS runtime: internally-tagged
//! enums use kebab-case variant tags and camelCase fields exactly as the
//! provider-neutral JSON records do.

use crate::brand::{CallId, ProviderRequestId, ReasoningEffortId};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Serializable provider or transport failure facts (TS `LlmFailure`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmFailure {
    /// Human-readable provider or transport failure.
    pub message: String,
    /// Stable provider-neutral machine-routing code.
    pub code: String,
    /// HTTP status returned by the provider, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u64>,
    /// Provider-requested delay in milliseconds, when valid and available.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "providerRetryAfterMs"
    )]
    pub provider_retry_after_ms: Option<u64>,
    /// Opaque provider-issued request identifier for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "requestId")]
    pub request_id: Option<ProviderRequestId>,
}

/// Durable raster image metadata. Kept in the provider-neutral core to avoid
/// an llm↔attachment dependency cycle; the attachment service owns validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachmentRef {
    /// Opaque content-addressed storage identity.
    #[serde(alias = "id")]
    pub attachment_id: String,
    /// Media type verified from the stored bytes (absent on legacy references).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Exact encoded byte length (absent on legacy references).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// Intrinsic encoded dimensions (absent on legacy references).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    /// Optional display name stripped of local path information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Any known content block, discriminated by `type` (merge-extensible in TS;
/// Rust models the four core blocks).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ContentBlock {
    /// Plain text visible to the end user.
    Text { text: String },
    /// Reasoning / thinking content, distinct from visible text.
    Reasoning { text: String },
    /// A durable raster image reference.
    Image {
        #[serde(rename = "attachment")]
        attachment: ImageAttachmentRef,
    },
    /// A tool invocation requested by the model.
    ToolCall {
        /// Provider-issued call id; correlates with the matching tool result.
        id: CallId,
        name: String,
        /// Raw JSON string as produced by the model.
        arguments: String,
    },
    /// The result of a tool invocation, sent back to the model.
    ToolResult {
        #[serde(rename = "toolCallId")]
        tool_call_id: CallId,
        content: Vec<ContentBlock>,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "isError")]
        is_error: Option<bool>,
    },
}

impl ContentBlock {
    /// The `type` tag (TS `block.type`).
    pub fn type_tag(&self) -> &'static str {
        match self {
            ContentBlock::Text { .. } => "text",
            ContentBlock::Reasoning { .. } => "reasoning",
            ContentBlock::Image { .. } => "image",
            ContentBlock::ToolCall { .. } => "tool-call",
            ContentBlock::ToolResult { .. } => "tool-result",
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } | ContentBlock::Reasoning { text } => Some(text),
            _ => None,
        }
    }

    pub fn as_tool_call(&self) -> Option<(&CallId, &str, &str)> {
        match self {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
            } => Some((id, name, arguments)),
            _ => None,
        }
    }

    pub fn as_tool_result(&self) -> Option<(&CallId, &[ContentBlock], Option<bool>)> {
        match self {
            ContentBlock::ToolResult {
                tool_call_id,
                content,
                is_error,
            } => Some((tool_call_id, content, *is_error)),
            _ => None,
        }
    }
}

/// Why a model response stopped (discriminated by `kind`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    MaxTokens,
    Aborted { failure: LlmFailure },
    Error { failure: LlmFailure },
}

impl FinishReason {
    pub fn kind(&self) -> &'static str {
        match self {
            FinishReason::Stop => "stop",
            FinishReason::ToolCalls => "tool-calls",
            FinishReason::MaxTokens => "max-tokens",
            FinishReason::Aborted { .. } => "aborted",
            FinishReason::Error { .. } => "error",
        }
    }
}

/// Token accounting for one model call (cache fields are optional; counts
/// are DISJOINT).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "cacheReadTokens"
    )]
    pub cache_read_tokens: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "cacheWriteTokens"
    )]
    pub cache_write_tokens: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "reasoningTokens"
    )]
    pub reasoning_tokens: Option<u64>,
}

/// A tool invocation requested by the model (the standalone block view of
/// `ContentBlock::ToolCall`; TS `ToolCallBlock`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub struct ToolCallBlock {
    /// Provider-issued call id; correlates with the matching tool result.
    pub id: CallId,
    pub name: String,
    /// Raw JSON string as produced by the model.
    pub arguments: String,
}

/// Raw streaming protocol emitted by adapters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StreamChunk {
    BlockStart {
        index: u64,
        #[serde(rename = "blockType")]
        block_type: String,
    },
    TextDelta {
        index: u64,
        text: String,
    },
    ReasoningDelta {
        index: u64,
        text: String,
    },
    ToolCallDelta {
        index: u64,
        id: CallId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(rename = "argumentsDelta")]
        arguments_delta: String,
    },
    BlockEnd {
        index: u64,
        block: ContentBlock,
    },
    Usage {
        usage: TokenUsage,
    },
    Finish {
        reason: FinishReason,
        /// Adapter-private lossless-JSON state for replaying a successful
        /// response.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "replayState"
        )]
        replay_state: Option<JsonValue>,
    },
}

impl StreamChunk {
    pub fn type_tag(&self) -> &'static str {
        match self {
            StreamChunk::BlockStart { .. } => "block-start",
            StreamChunk::TextDelta { .. } => "text-delta",
            StreamChunk::ReasoningDelta { .. } => "reasoning-delta",
            StreamChunk::ToolCallDelta { .. } => "tool-call-delta",
            StreamChunk::BlockEnd { .. } => "block-end",
            StreamChunk::Usage { .. } => "usage",
            StreamChunk::Finish { .. } => "finish",
        }
    }
}

/// JSON-schema description of a tool, as sent to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON Schema object for the arguments.
    pub parameters: JsonValue,
}

/// Provider, model, reasoning effort, and sampling scalars of one
/// conversation's requests (TS `LlmCallConfig`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmCallConfig {
    pub provider: String,
    pub model: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "reasoningEffort"
    )]
    pub reasoning_effort: Option<ReasoningEffortId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "maxTokens")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

/// Effective config fields supplied by exact-model adapter resolution rather
/// than by the caller's request proposal (TS marker object; each present
/// field must be `true`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmCallConfigAdapterDefaults {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "reasoningEffort"
    )]
    pub reasoning_effort: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "maxTokens")]
    pub max_tokens: Option<bool>,
}

/// Display metadata for one registered provider route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmProviderInfo {
    /// Provider route key used by `GenerateOptions.provider`.
    pub id: String,
    /// Human-readable provider name.
    pub name: String,
}

/// Any declared provider model modality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelModality {
    Text,
    Image,
}

impl ModelModality {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelModality::Text => "text",
            ModelModality::Image => "image",
        }
    }
}

/// One provider route an adapter plugin can activate through configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfigurableProvider {
    pub provider: String,
    pub display_name: String,
    pub settings_ns: String,
    pub settings_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared: Option<bool>,
}

/// One interrogation of a provider endpoint that configuration has not
/// stored yet.
#[derive(Clone, Default)]
pub struct LlmModelDiscoveryRequest {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api: Option<String>,
    pub api_key: Option<String>,
    /// Caller cancellation (TS `AbortSignal`); implementations must settle
    /// promptly after it aborts.
    pub signal: Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// One model an endpoint reports about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmDiscoveredModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_descriptions: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_summaries: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_parameters: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<ModelModality>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

/// One adapter-discovered model; catalog membership is advisory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelInfo {
    pub provider: String,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "inputModalities"
    )]
    pub input_modalities: Option<Vec<ModelModality>>,
}

/// Provider-owned context capacity for one exact provider/model route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelContext {
    /// True when this is a runtime budget rather than a provider-disclosed limit.
    #[serde(default)]
    pub estimated: bool,
    pub context_window: u64,
}

/// Display metadata for one adapter-owned reasoning effort.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmReasoningEffortInfo {
    pub id: ReasoningEffortId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Selectable reasoning efforts for one exact provider/model route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelReasoningInfo {
    pub efforts: Vec<LlmReasoningEffortInfo>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "defaultEffort"
    )]
    pub default_effort: Option<ReasoningEffortId>,
}

/// Exact-route model metadata resolved by its owning adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmResolvedModelInfo {
    pub provider: String,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "inputModalities"
    )]
    pub input_modalities: Option<Vec<ModelModality>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<LlmModelContext>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "defaultMaxTokens"
    )]
    pub default_max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<LlmModelReasoningInfo>,
}

/// A single model request, fully assembled.
#[derive(Clone)]
pub struct GenerateOptions {
    /// Registered provider route selecting the adapter instance.
    pub provider: String,
    pub model: String,
    /// Adapter-owned reasoning effort selected for this exact model.
    pub reasoning_effort: Option<ReasoningEffortId>,
    /// Ordered conversation messages, exactly as the provider sees them.
    pub messages: Vec<crate::message::Message>,
    /// System prompt text.
    pub system: Option<String>,
    /// Tool schemas.
    pub tools: Option<Vec<ToolSchema>>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub stop: Option<Vec<String>>,
    /// Cancellation predicate (TS `AbortSignal`); `Some(|| true)` = aborted.
    pub signal: Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Session identity stamped by the loop (TS branded `SessionId`; the
    /// llm crate cannot depend on dsh-session, so it is a plain string).
    pub session_id: Option<String>,
    /// Provider-neutral classification for an auxiliary model call.
    pub purpose: Option<String>,
    /// Process-local identity of requests assembled by dsh-agent-loop (the
    /// TS `WeakSet` membership flag).
    pub agent_loop_request: bool,
}
