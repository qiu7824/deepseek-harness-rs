//! The session-log record of which preset a session actually runs.
//! Rust port of `src/session.ts`.
//!
//! The creation header names the preset a session STARTED with. A session may
//! still change preset while it is blank, and that change is a logged
//! `agent-preset/selected` event. Reconstruction reads the log, newest
//! selection winning.

use dsh_session::{SessionEvent, SessionHeader};

/// The `agent-preset/selected` event type (TS `SessionEventMap` extension).
pub const AGENT_PRESET_SELECTED: &str = "agent-preset/selected";

/// Build the event data of a logged preset selection
/// (`{ "agentPreset": id }`).
pub fn selected_data(agent_preset: &str) -> serde_json::Value {
    serde_json::json!({ "agentPreset": agent_preset })
}

/// The preset a session actually runs, newest selection winning
/// (TS `resolveSessionPreset`).
///
/// The header supplies the creation-time value; every later selection is a
/// logged event, so the last one is the answer.
pub fn resolve_session_preset(header: &SessionHeader, events: &[SessionEvent]) -> Option<String> {
    for event in events.iter().rev() {
        if event.type_ == AGENT_PRESET_SELECTED {
            if let Some(preset) = event
                .data
                .get("agentPreset")
                .and_then(|value| value.as_str())
            {
                return Some(preset.to_string());
            }
        }
    }
    header.agent_preset.clone()
}
