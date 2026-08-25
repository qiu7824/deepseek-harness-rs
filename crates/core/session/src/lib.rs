//! Event-sourced session service: append-only session log, in-memory store,
//! and the derived LLM message history. Rust port of
//! `@deepseek-ai/dsh-session`.
//!
//! Persistence is a plugin concern (subscribe to `session/event`, drain on
//! `session/flush`).

pub mod chunk_rows;
pub mod invariant;
pub mod json;
pub mod known_event_types;
pub mod preparation;
pub mod repair;
pub mod request_header;
pub mod store;
pub mod surface;
pub mod types;

pub use chunk_rows::{
    ChunkRow, StorageRecord, TextRunData, ToolCallRunData, decode_storage_record, pack_chunk_runs,
    visit_decoded_storage_record_events, visit_decoded_storage_record_tail,
    visit_owned_storage_record_events, visit_storage_record_events,
};
pub use json::{JsonValue, is_json_value, snapshot_json_value};
pub use known_event_types::{KNOWN_SESSION_EVENT_TYPES, is_known_session_event_type};
pub use preparation::{SessionPreparation, SessionPreparationOptions};
pub use repair::{TOOL_NOT_STARTED, TOOL_OUTCOME_UNKNOWN, interrupted_turn_closers};
pub use request_header::{
    RequestHeaderPayload, canonical_header, fold_request_header, header_equals,
};
pub use store::{
    ForkError, Session, SessionForkError, SessionForkErrorCode, SessionForkSource, SessionStore,
};
pub use surface::{
    SURFACE_EVENT_TYPES, SessionSurface, StreamingSurfaceFold, SurfaceFoldReplacement,
    SurfaceFoldResult, SurfaceManager, derive_event_message, fold_surface, is_append_surface_event,
    is_replacement_surface_event, is_surface_eligible_type, is_surface_event,
};
pub use types::{
    AgentCancelCause, CreateSessionMeta, CreateSessionOptions, EpochHeader, RequestContext,
    RequestHeaderReason, SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionId,
    SessionIdTag, SurfaceIntent, SurfaceOp, TodoItem, TodoStatus, TurnEndCancelCause,
    TurnEndReason, assistant_chunk_data, assistant_message_data, end_seed_data,
    request_header_data, session_id, snapshot_session_header, step_data, todo_write_data,
    tool_call_data, tool_result_data, turn_end_data, turn_start_data, validate_session_header,
};

// Re-export the message vocabulary dsh-session's events carry.
pub use dsh_llm::{
    AssistantMessage, ContentBlock, Message, MessageSource, Role, TokenUsage, ToolResultMessage,
    UserMessage,
};
