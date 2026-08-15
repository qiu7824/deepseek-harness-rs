//! Durable request-route lookup for one open model step. Rust port of
//! `packages/llm/llm-retry/src/history.ts`.

use dsh_session::SessionEvent;

/// Find the provider in force for one currently open step (TS
/// `providerForOpenStep`).
pub fn provider_for_open_step(
    events: &[SessionEvent],
    turn: u64,
    step: u64,
) -> Option<String> {
    let step_start_index = events.iter().rposition(|event| {
        event.type_ == "step/start"
            && event.data.get("turn").and_then(|value| value.as_u64()) == Some(turn)
            && event.data.get("step").and_then(|value| value.as_u64()) == Some(step)
    })?;
    if events[step_start_index + 1..].iter().any(|event| {
        event.type_ == "step/end" || event.type_ == "turn/end"
    }) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event(type_: &str, seq: u64, data: serde_json::Value) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time: 0,
            data,
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    #[test]
    fn finds_the_provider_in_force_for_an_open_step() {
        let events = vec![
            event("request/header", 0, serde_json::json!({
                "header": {"config": {"provider": "first"}}
            })),
            event("turn/start", 1, serde_json::json!({"turn": 1})),
            event("step/start", 2, serde_json::json!({"turn": 1, "step": 1})),
            event("request/header", 3, serde_json::json!({
                "header": {"config": {"provider": "second"}}
            })),
        ];
        assert_eq!(provider_for_open_step(&events, 1, 1), Some("second".to_string()));
        // A closed step has no provider in force.
        let mut closed = events.clone();
        closed.push(event("step/end", 4, serde_json::json!({"turn": 1, "step": 1})));
        assert_eq!(provider_for_open_step(&closed, 1, 1), None);
        // An unknown step is undefined.
        assert_eq!(provider_for_open_step(&events, 2, 9), None);
        // No header anywhere: undefined.
        let headerless = vec![
            event("turn/start", 0, serde_json::json!({"turn": 1})),
            event("step/start", 1, serde_json::json!({"turn": 1, "step": 1})),
        ];
        assert_eq!(provider_for_open_step(&headerless, 1, 1), None);
    }
}
