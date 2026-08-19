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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_target_wire() {
        assert_eq!(
            serde_json::to_value(InboxTarget::NextTurn).unwrap(),
            serde_json::json!("next-turn")
        );
        assert_eq!(
            serde_json::to_value(InboxTarget::NextStep).unwrap(),
            serde_json::json!("next-step")
        );
        let back: InboxTarget = serde_json::from_value(serde_json::json!("next-turn")).unwrap();
        assert_eq!(back, InboxTarget::NextTurn);
    }

    #[test]
    fn splice_wire_shape() {
        let message = dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::Text { text: "hi".into() }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        let splice = InboxSplice {
            target: InboxTarget::NextStep,
            start: 0,
            removed_count: Some(1),
            inserted: vec![message],
            outcome: Some(InboxSpliceOutcome::Canceled),
        };
        let json = serde_json::to_value(&splice).unwrap();
        assert_eq!(json["target"], "next-step");
        assert_eq!(json["start"], 0);
        assert_eq!(json["removedCount"], 1);
        assert_eq!(json["outcome"], "canceled");
        assert_eq!(json["inserted"][0]["role"], "user");

        let minimal = InboxSplice {
            target: InboxTarget::NextTurn,
            start: 1,
            removed_count: None,
            inserted: vec![],
            outcome: None,
        };
        let json = serde_json::to_value(&minimal).unwrap();
        assert!(!json.as_object().unwrap().contains_key("removedCount"));
        assert!(!json.as_object().unwrap().contains_key("outcome"));
    }
}
