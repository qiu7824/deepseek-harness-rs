//! Log-backed session title service, deterministic fallback, and provider
//! contract. Rust port of `@deepseek-ai/dsh-session-title`.
//!
//! The `title` projection key (the package's one-home declaration in TS
//! `src/types.ts`) lives in [`index::title_projection_definition`]; the
//! invariant companion mirrors `src/invariant.ts`.

pub mod index;
pub mod invariant;
pub mod normalize;
pub mod types;

pub use index::{
    INJECT, NAME, RenameFailure, SessionTitlePlugin, SessionTitleService,
    collect_session_title_messages, fold_session_title, title_event_data,
    title_projection_definition,
};
pub use normalize::{
    fallback_session_title, normalize_session_title, truncate_title_utf8,
};
pub use types::{
    Config, SessionTitleAutomaticMode, SessionTitleError, SessionTitleInvalidError,
    SessionTitleModelProvenance, SessionTitleProvider, SessionTitleProviderId,
    SessionTitleProviderRequest, SessionTitleProviderResult, SessionTitleSignal,
    SessionTitleSnapshot, SessionTitleSource, SessionTitleUserMessage,
    SessionTitleProviderIdTag, session_title_provider_id,
};
