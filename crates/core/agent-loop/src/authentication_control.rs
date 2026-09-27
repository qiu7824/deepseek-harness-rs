//! Replay of the account-only continuation pause. No credential identity is durable.

use dsh_agent::ACCOUNT_SIGNED_OUT_REASON;
use dsh_session::SessionEvent;
use serde::{Deserialize, Serialize};

pub(crate) const EVENT: &str = "agent/account-continuation";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Control {
    pub owner: String,
    pub turn: u64,
    pub revision: u64,
    pub paused: bool,
    pub reason: String,
}

#[derive(Default)]
pub(crate) struct Replay {
    pub turn: u64,
    pub revision: u64,
    pub paused: bool,
    control_turn: Option<u64>,
}

impl Replay {
    pub fn observe(&mut self, owner: &str, event: &SessionEvent, own: bool) -> Result<(), String> {
        if event.type_ == "turn/start" {
            self.turn = event.data["turn"].as_u64().unwrap_or(0);
            self.paused = false;
        } else if event.type_ == "turn/end" && own && self.control_turn != Some(self.turn) {
            self.paused = event
                .data
                .pointer("/reason/kind")
                .and_then(serde_json::Value::as_str)
                == Some("aborted")
                && event
                    .data
                    .pointer("/reason/reason/kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("hook")
                && event
                    .data
                    .pointer("/reason/reason/reason")
                    .and_then(serde_json::Value::as_str)
                    == Some(ACCOUNT_SIGNED_OUT_REASON);
        } else if event.type_ == EVENT {
            let control: Control = serde_json::from_value(event.data.clone())
                .map_err(|error| format!("invalid account continuation control: {error}"))?;
            if control.owner != owner || !own {
                return Ok(());
            }
            if control.reason != ACCOUNT_SIGNED_OUT_REASON
                || control.turn == 0
                || control.revision == 0
                || control.revision > 9_007_199_254_740_991
                || control.turn > self.turn
            {
                return Err("invalid account continuation control ownership or revision".into());
            }
            if control.revision <= self.revision {
                return Ok(());
            }
            self.revision = control.revision;
            if control.turn == self.turn {
                self.paused = control.paused;
                self.control_turn = Some(control.turn);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(kind: &str, data: serde_json::Value) -> SessionEvent {
        serde_json::from_value(json!({"type":kind,"seq":0,"time":0,"data":data,"ignorable":true}))
            .unwrap()
    }
    fn control(turn: u64, revision: u64, paused: bool) -> SessionEvent {
        event(
            EVENT,
            json!({"owner":"session-a","turn":turn,"revision":revision,"paused":paused,"reason":ACCOUNT_SIGNED_OUT_REASON}),
        )
    }
    #[test]
    fn late_old_revision_and_old_turn_cannot_override_explicit_resume_or_new_work() {
        let mut state = Replay::default();
        for row in [
            event("turn/start", json!({"turn":1})),
            control(1, 1, true),
            control(1, 2, false),
            control(1, 1, true),
            event(
                "turn/end",
                json!({"turn":1,"reason":{"kind":"aborted","reason":{"kind":"hook","reason":ACCOUNT_SIGNED_OUT_REASON}}}),
            ),
            event("turn/start", json!({"turn":2})),
            control(1, 50, true),
        ] {
            state.observe("session-a", &row, true).unwrap();
        }
        assert!(!state.paused);
        assert_eq!(state.revision, 50);
        assert_eq!(state.turn, 2);
        state
            .observe("session-a", &control(2, 51, true), true)
            .unwrap();
        assert!(state.paused);
    }
    #[test]
    fn inherited_or_other_owner_controls_cannot_pause_a_fresh_child() {
        let mut state = Replay::default();
        state
            .observe("child", &event("turn/start", json!({"turn":1})), false)
            .unwrap();
        state
            .observe("child", &control(1, 99, true), false)
            .unwrap();
        state
            .observe("child", &control(1, 100, true), true)
            .unwrap();
        assert!(!state.paused);
        assert_eq!(state.revision, 0);
    }
}
