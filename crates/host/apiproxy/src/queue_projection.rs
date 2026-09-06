//! Pending input snapshots derived from committed inbox splices.

use std::collections::HashMap;

use dsh_agent::{InboxSplice, InboxTarget};
use dsh_llm::{Message, MessageSource};
use dsh_session::{Session, SessionId};

use crate::api::events::{MuxFrame, QueuedInboxItem, QueuedInboxPlacement};

#[derive(Default)]
struct PendingInput {
    cursor: u64,
    next_turn: Vec<Message>,
    next_step: Vec<Message>,
}

/// One connection's incremental mirror. Read the durable log because inbox
/// notifications run before the Agent's in-memory projection is updated.
#[derive(Default)]
pub(crate) struct QueueProjection {
    sessions: HashMap<SessionId, PendingInput>,
}

impl QueueProjection {
    pub(crate) fn snapshot(&mut self, session: &Session) -> MuxFrame {
        let pending = self
            .sessions
            .entry(session.id().clone())
            .or_insert_with(|| PendingInput {
                cursor: session.inherited_event_count().get(),
                ..Default::default()
            });
        for event in session.events_from(pending.cursor) {
            pending.cursor = event.seq.get() + 1;
            if event.type_ != "agent/inbox/spliced" {
                continue;
            }
            let splice: InboxSplice =
                serde_json::from_value(event.data).expect("committed inbox splice must be valid");
            let list = match splice.target {
                InboxTarget::NextTurn => &mut pending.next_turn,
                InboxTarget::NextStep => &mut pending.next_step,
            };
            let start = splice.start as usize;
            let end = start + splice.removed_count.unwrap_or_default() as usize;
            list.splice(start..end, splice.inserted);
        }
        let items = pending
            .next_turn
            .iter()
            .map(|message| QueuedInboxItem {
                id: message.id.clone(),
                placement: QueuedInboxPlacement::Queued,
                message: message.clone(),
            })
            .chain(pending.next_step.iter().map(|message| QueuedInboxItem {
                id: message.id.clone(),
                placement: if matches!(message.source, MessageSource::User { .. }) {
                    QueuedInboxPlacement::Steering
                } else {
                    QueuedInboxPlacement::Context
                },
                message: message.clone(),
            }))
            .collect();
        MuxFrame::SessionQueue {
            session_id: session.id().clone(),
            items,
        }
    }
}
