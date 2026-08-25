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
    INJECT, MODEL_SELECTION_KEY, MODEL_SELECTION_STATE_VERSION, NAME, RenameFailure,
    SESSION_LIST_METADATA_KEY, SESSION_LIST_METADATA_STATE_VERSION, SessionTitlePlugin,
    SessionTitleService, USER_MESSAGE_RAIL_KEY, USER_MESSAGE_RAIL_STATE_VERSION,
    collect_session_title_messages, fold_session_title, model_selection_projection_definition,
    session_list_metadata_projection_definition, title_event_data, title_projection_definition,
    user_message_rail_row, user_message_rail_rows,
};
pub use normalize::{fallback_session_title, normalize_session_title, truncate_title_utf8};
pub use types::{
    Config, SessionTitleAutomaticMode, SessionTitleError, SessionTitleInvalidError,
    SessionTitleModelProvenance, SessionTitleProvider, SessionTitleProviderId,
    SessionTitleProviderIdTag, SessionTitleProviderRequest, SessionTitleProviderResult,
    SessionTitleSignal, SessionTitleSnapshot, SessionTitleSource, SessionTitleUserMessage,
    session_title_provider_id,
};
