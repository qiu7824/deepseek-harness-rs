//! First-human-message model provider for `ctx.sessionTitle`. Rust port of
//! `@deepseek-ai/dsh-session-title-first-prompt-llm`.

pub mod index;
pub mod invariant;

pub use dsh_session_title_llm::SessionTitleLlmConfig as Config;
pub use index::{INJECT, NAME, SessionTitleFirstPromptLlmPlugin, apply};
