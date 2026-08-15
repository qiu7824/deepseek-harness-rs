//! Provider-routed model-request retry policy for the DeepSeek Harness.
//! Rust port of `@deepseek-ai/dsh-llm-retry`.

pub mod brand;
pub mod history;
pub mod index;
pub mod invariant;
pub mod types;

pub use brand::{RetryId, RetryIdTag, retry_id};
pub use history::provider_for_open_step;
pub use index::{
    INJECT, NAME, CancellationSignal, Config, RequestErrorPayload, RetryInternals, apply,
    validate_executor_config,
};
pub use types::{LlmRetryEventData, LlmRetryStartedEventData};
