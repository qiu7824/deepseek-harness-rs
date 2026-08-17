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

#[cfg(test)]
mod tests {
    use super::*;

    fn header(agent_preset: Option<&str>) -> SessionHeader {
        let mut value = serde_json::json!({
            "version": 0,
            "id": "s",
            "createdAt": 1,
            "delegationDepth": 0,
        });
        if let Some(agent_preset) = agent_preset {
            value["agentPreset"] = serde_json::Value::String(agent_preset.to_string());
        }
        dsh_session::validate_session_header(&dsh_session::SessionId::new("s".to_string()), &value)
            .unwrap()
    }

    fn selected(agent_preset: &str, seq: u64) -> SessionEvent {
        SessionEvent {
            type_: AGENT_PRESET_SELECTED.to_string(),
            seq,
            time: seq as i64,
            data: selected_data(agent_preset),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    #[test]
    fn reads_the_creation_time_value_when_nothing_was_switched() {
        assert_eq!(
            resolve_session_preset(&header(Some("standard")), &[]),
            Some("standard".to_string())
        );
    }

    #[test]
    fn prefers_a_logged_switch_over_the_header() {
        assert_eq!(
            resolve_session_preset(&header(Some("standard")), &[selected("minimal", 0)]),
            Some("minimal".to_string())
        );
    }

    #[test]
    fn takes_the_last_switch_when_a_session_was_moved_twice() {
        assert_eq!(
            resolve_session_preset(
                &header(Some("standard")),
                &[selected("minimal", 0), selected("cordis", 1)],
            ),
            Some("cordis".to_string())
        );
    }

    #[test]
    fn finds_a_switch_behind_later_events() {
        let later = SessionEvent {
            type_: "turn/end".to_string(),
            seq: 2,
            time: 2,
            data: serde_json::json!({ "turn": 1 }),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        };
        assert_eq!(
            resolve_session_preset(&header(None), &[selected("minimal", 0), later]),
            Some("minimal".to_string())
        );
    }

    #[test]
    fn reports_none_when_the_deployment_composes_no_presets() {
        assert_eq!(resolve_session_preset(&header(None), &[]), None);
    }
}
