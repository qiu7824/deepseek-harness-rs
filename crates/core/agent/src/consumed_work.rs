//! How one agent log accounts for the work it consumed. Rust port of
//! `packages/core/agent/src/consumed-work.ts`.

use dsh_session::{SessionEvent, TurnEndReason};

/// How one agent log accounts for the work it consumed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConsumedWork {
    /// The latest closed turn that accounts for consumed work.
    pub end: Option<SessionEvent>,
    /// Whether accepted work was cancelled out of the inbox, unrun, after
    /// that turn.
    pub dropped_unrun: bool,
}

/// Whether a turn that consumed input but never reached a step ends in a way
/// that accounts for that input (TS `accountsForClaim`).
fn accounts_for_claim(reason: &TurnEndReason) -> bool {
    match reason {
        TurnEndReason::Completed => false,
        TurnEndReason::Blocked
        | TurnEndReason::Aborted { .. }
        | TurnEndReason::Interrupted
        | TurnEndReason::Error { .. } => true,
        // Unreachable for max-tokens (it requires a step); merge-extensible
        // unknown endings over consumed input must not read as success.
        TurnEndReason::MaxTokens => true,
    }
}

/// Fold one agent log, or an owned suffix of one, into its account of
/// consumed work (TS `foldConsumedWork`).
pub fn fold_consumed_work(events: &[SessionEvent]) -> ConsumedWork {
    let mut stepped = std::collections::HashSet::new();
    let mut claimed = std::collections::HashSet::new();
    let mut open: Option<u64> = None;
    let mut end: Option<SessionEvent> = None;
    let mut dropped_unrun = false;
    for event in events {
        match event.type_.as_str() {
            "turn/start" => {
                open = event.data.get("turn").and_then(|value| value.as_u64());
            }
            "step/start" => {
                if let Some(turn) = event.data.get("turn").and_then(|value| value.as_u64()) {
                    stepped.insert(turn);
                }
            }
            "agent/inbox/spliced" => {
                let removed_count = event
                    .data
                    .get("removedCount")
                    .and_then(|value| value.as_u64());
                let inserted = event
                    .data
                    .get("inserted")
                    .and_then(|value| value.as_array())
                    .map(|value| value.len())
                    .unwrap_or(0);
                let Some(_removed) = removed_count else {
                    continue;
                };
                let outcome = event.data.get("outcome").and_then(|value| value.as_str());
                if outcome == Some("canceled") {
                    // A replacement keeps the work pending under a new
                    // identity, so only a cancellation that leaves nothing
                    // behind drops it.
                    dropped_unrun |= inserted == 0;
                } else if let Some(turn) = open {
                    claimed.insert(turn);
                }
            }
            "turn/end" => {
                let turn = event.data.get("turn").and_then(|value| value.as_u64());
                let reason: Option<TurnEndReason> = event
                    .data
                    .get("reason")
                    .and_then(|value| serde_json::from_value(value.clone()).ok());
                open = None;
                if let Some(turn) = turn {
                    let was_stepped = stepped.remove(&turn);
                    let was_claimed = claimed.remove(&turn);
                    if was_stepped
                        || (was_claimed && reason.as_ref().is_some_and(accounts_for_claim))
                    {
                        end = Some(event.clone());
                        // Anything dropped before this turn closed is what
                        // its own ending reports; only a later drop is still
                        // unaccounted for.
                        dropped_unrun = false;
                    }
                }
            }
            _ => {}
        }
    }
    ConsumedWork { end, dropped_unrun }
}
