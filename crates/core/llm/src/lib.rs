//! Canonical provider-neutral message and streaming vocabulary for the loop,
//! session log, and plugins: Rust port of `@deepseek-ai/dsh-llm`.
//!
//! Delivers the wire-compatible value vocabulary plus the adapter runtime
//! (`LlmRuntime`), block assembler, stream invariants, and call
//! configuration utilities. Provider adapters themselves belong to later
//! milestones.

pub mod adapter_failure;
pub mod api_key;
pub mod assembler;
pub mod attribution;
pub mod brand;
pub mod call_config;
pub mod content;
pub mod error;
pub mod invariant;
pub mod message;
pub mod never;
pub mod retry_policy;
pub mod runtime;
pub mod types;

pub use adapter_failure::{failure_snapshot, normalize_llm_failure};
pub use api_key::{ApiKeyCheck, ApiKeyRejection, normalize_api_key};
pub use assembler::BlockAssembler;
pub use attribution::{AppIdentity, app_identity, attribution_headers, user_agent};
pub use brand::{
    CallId, CallIdTag, MessageId, MessageIdTag, ProviderRequestId, ProviderRequestIdTag,
    ReasoningEffortId, ReasoningEffortIdTag, call_id, message_id, provider_request_id,
    reasoning_effort_id,
};
pub use call_config::{
    adapter_defaults_equals, call_config_equals, deep_freeze, is_agent_loop_request,
    mark_agent_loop_request,
};
pub use content::content_has_image;
pub use error::{
    CONTEXT_WINDOW_EXCEEDED_CODE, EMPTY_RESPONSE_CODE, INVALID_CREDENTIAL_CODE,
    QUOTA_EXCEEDED_CODE, HarnessError, error_chain,
};
pub use invariant::{LlmInvariantPlugin, NAME as LLM_INVARIANT_NAME, PACKAGE_NAME as LLM_INVARIANT_PACKAGE_NAME, apply as apply_llm_invariant, validate_stream};
pub use message::{
    CONTEXT_SUMMARY_MAX_CHARS, AssistantMessage, ContextForm, ContextSnapshotSection, Message,
    MessageSource, ModelMessageSource, Role, SkillCatalogEntry, ToolMessageSource,
    ToolResultMessage, ToolResultMessageInput, UserMessage, bound_context_summary,
    create_assistant_message, create_message, create_tool_result_message, create_user_message,
    freeze_message, is_token_delta,
};
pub use never::assert_never;
pub use retry_policy::{
    DEFAULT_JITTER_RATIO, DEFAULT_MAX_DELAY_MS, DEFAULT_MAX_RETRIES, DEFAULT_RETRYABLE_CODES,
    ResolvedAlwaysRetryPolicy, ResolvedNormalRetryPolicy, ResolvedRetryBackoff,
    ResolvedRetryPolicy, resolve_retry_policy,
};
pub use runtime::{
    AdapterRegistrationHandle, ChunkStream, DirectoryRegistrationHandle, LlmAdapter, LlmError,
    LlmErrorOptions, LlmRuntime, PreparedLlmCall, StreamFactory, assert_usable_api_key,
    generate_options_config_equals,
};
pub use types::{
    ContentBlock, FinishReason, GenerateOptions, ImageAttachmentRef, LlmCallConfig,
    LlmCallConfigAdapterDefaults, LlmConfigurableProvider, LlmDiscoveredModel, LlmFailure,
    LlmModelContext, LlmModelDiscoveryRequest, LlmModelInfo, LlmModelReasoningInfo,
    LlmProviderInfo, LlmReasoningEffortInfo, LlmResolvedModelInfo, ModelModality, StreamChunk,
    TokenUsage, ToolCallBlock, ToolSchema,
};
