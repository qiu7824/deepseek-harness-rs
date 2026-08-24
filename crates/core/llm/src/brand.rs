//! dsh-llm's owned branded ids: Rust port of
//! `packages/llm/llm/src/brand.ts`.
//!
//! The `Branded<B>` primitive lives in `dsh-brand`; each id below is a
//! nominal string brand.

use dsh_brand::Branded;

/// Stable identity carried by one message across inbox, log, and
/// model-request boundaries.
#[doc(hidden)]
pub enum MessageIdTag {}
pub type MessageId = Branded<MessageIdTag>;

/// Brand a message identifier (plain cast, no validation).
pub fn message_id(id: impl Into<String>) -> MessageId {
    Branded::new(id)
}

/// Correlates a model-issued tool call with its result.
#[doc(hidden)]
pub enum CallIdTag {}
pub type CallId = Branded<CallIdTag>;

/// Brand a tool call identifier (plain cast, no validation).
pub fn call_id(id: impl Into<String>) -> CallId {
    Branded::new(id)
}

/// Provider-issued request identifier retained for diagnostics.
#[doc(hidden)]
pub enum ProviderRequestIdTag {}
pub type ProviderRequestId = Branded<ProviderRequestIdTag>;

/// Brand a provider-issued request identifier.
pub fn provider_request_id(id: impl Into<String>) -> ProviderRequestId {
    Branded::new(id)
}

/// Adapter-owned identifier for one model's selectable reasoning effort.
#[doc(hidden)]
pub enum ReasoningEffortIdTag {}
pub type ReasoningEffortId = Branded<ReasoningEffortIdTag>;

/// Brand an adapter-owned reasoning-effort identifier.
pub fn reasoning_effort_id(id: impl Into<String>) -> ReasoningEffortId {
    Branded::new(id)
}
