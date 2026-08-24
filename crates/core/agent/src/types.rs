//! Durable agent session-event vocabulary. Rust port of
//! `packages/core/agent/src/types.ts`.

use serde::{Deserialize, Serialize};

/// One of the two ordered pending-message lists owned by an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InboxTarget {
    NextTurn,
    NextStep,
}

impl InboxTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            InboxTarget::NextTurn => "next-turn",
            InboxTarget::NextStep => "next-step",
        }
    }
}

/// One normalized mutation of an agent's durable pending-message lists
/// (the `agent/inbox/spliced` event payload).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxSplice {
    pub target: InboxTarget,
    pub start: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "removedCount"
    )]
    pub removed_count: Option<u64>,
    pub inserted: Vec<dsh_llm::Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<InboxSpliceOutcome>,
}

/// Why a splice removed messages: only cancellations carry the marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InboxSpliceOutcome {
    Canceled,
}

/// Read the payload of an `agent/inbox/spliced` event.
pub fn inbox_splice_of(event: &dsh_session::SessionEvent) -> Option<InboxSplice> {
    if event.type_ != "agent/inbox/spliced" {
        return None;
    }
    serde_json::from_value(event.data.clone()).ok()
}
