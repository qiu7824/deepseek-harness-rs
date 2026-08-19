//! All-human-messages model provider for `ctx.sessionTitle`. Rust port of
//! `@deepseek-ai/dsh-session-title-all-prompts-llm`.

pub mod index;
pub mod invariant;

pub use dsh_session_title_llm::SessionTitleLlmConfig as Config;
pub use index::{INJECT, NAME, SessionTitleAllPromptsLlmPlugin, apply};
