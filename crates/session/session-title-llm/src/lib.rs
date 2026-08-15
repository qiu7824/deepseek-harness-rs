//! Shared LLM route, framing, timeout, assembly, and validation policy for
//! model-backed session-title providers. Rust port of
//! `@deepseek-ai/dsh-session-title-llm`.

pub mod index;
pub mod invariant;

pub use index::{
    SESSION_TITLE_TIMEOUT_CODE, SessionTitleLlmConfig, SessionTitleLlmError,
    SessionTitleLlmMessageSelector, config_schema, frame_messages,
    generate_session_title_with_llm, register_session_title_llm_provider,
    resolve_session_title_llm_config, system_prompt,
};
