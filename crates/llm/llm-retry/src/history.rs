//! Durable request-route lookup for one open model step. Rust port of
//! `packages/llm/llm-retry/src/history.ts`.

use dsh_session::SessionEvent;

/// Find the provider in force for one currently open step (TS
/// `providerForOpenStep`).
pub fn provider_for_open_step(events: &[SessionEvent], turn: u64, step: u64) -> Option<String> {
    let step_start_index = events.iter().rposition(|event| {
        event.type_ == "step/start"
            && event.data.get("turn").and_then(|value| value.as_u64()) == Some(turn)
            && event.data.get("step").and_then(|value| value.as_u64()) == Some(step)
    })?;
    if events[step_start_index + 1..]
        .iter()
        .any(|event| event.type_ == "step/end" || event.type_ == "turn/end")
    {
        return None;
    }
    for event in events.iter().rev() {
        if event.type_ == "request/header" {
            return event
                .data
                .get("header")
                .and_then(|header| header.get("config"))
                .and_then(|config| config.get("provider"))
                .and_then(|provider| provider.as_str())
                .map(str::to_string);
        }
    }
    None
}
