//! Pending input snapshots derived from committed inbox splices.

use std::collections::HashMap;

use dsh_agent::{InboxSplice, InboxTarget};
use dsh_llm::{Message, MessageSource};
use dsh_session::{Session, SessionId};
use serde::Deserialize;

use crate::api::events::{MuxFrame, QueuedInboxItem, QueuedInboxPlacement};

#[derive(Clone, Default)]
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
    pub(crate) fn snapshot(&mut self, session: &Session) -> Result<MuxFrame, String> {
        // Publish the new cursor only after the entire captured prefix has
        // been read and validated; an archive error must not skip mutations.
        let mut pending =
            self.sessions
                .get(session.id())
                .cloned()
                .unwrap_or_else(|| PendingInput {
                    cursor: session.inherited_event_count().get(),
                    ..Default::default()
                });
        session.visit_events(pending.cursor, None, |event| {
            pending.cursor = event.seq.get() + 1;
            if event.type_ != "agent/inbox/spliced" {
                return Ok(true);
            }
            let splice = InboxSplice::deserialize(&event.data)
                .map_err(|error| format!("invalid committed inbox splice: {error}"))?;
            let list = match splice.target {
                InboxTarget::NextTurn => &mut pending.next_turn,
                InboxTarget::NextStep => &mut pending.next_step,
            };
            let start =
                usize::try_from(splice.start).map_err(|_| "inbox splice offset overflows")?;
            let removed = usize::try_from(splice.removed_count.unwrap_or_default())
                .map_err(|_| "inbox splice length overflows")?;
            let end = start
                .checked_add(removed)
                .filter(|end| *end <= list.len())
                .ok_or("committed inbox splice exceeds its queue")?;
            list.splice(start..end, splice.inserted);
            Ok(true)
        })?;
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
        self.sessions.insert(session.id().clone(), pending);
        Ok(MuxFrame::SessionQueue {
            session_id: session.id().clone(),
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::{SessionEvent, SessionLogOffset, SessionSeq};
    use serde_json::json;

    #[test]
    fn archived_queue_is_incremental_and_invalid_splices_do_not_advance_it() {
        let message = |id: &str| json!({"id":id,"role":"user","source":{"kind":"user"},"content":[{"type":"text","text":id}]});
        let records = [
            (
                "agent/inbox/spliced",
                json!({"target":"next-turn","start":0,"inserted":[message("first")]}),
            ),
            ("assistant/chunk", json!({"opaque":"x".repeat(1024 * 1024)})),
            (
                "agent/inbox/spliced",
                json!({"target":"next-turn","start":0,"removedCount":1,"inserted":[message("second")]}),
            ),
            (
                "agent/inbox/spliced",
                json!({"target":"next-step","start":0,"inserted":[message("steer")]}),
            ),
        ];
        let mut archive =
            dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        for (index, (kind, data)) in records.into_iter().enumerate() {
            archive
                .push(&SessionEvent {
                    type_: kind.into(),
                    seq: SessionSeq::new(index as u64).unwrap(),
                    time: index as i64,
                    data,
                    ignorable: None,
                    surface_op: None,
                    source_event_seqs: None,
                })
                .unwrap();
        }
        let header =
            dsh_session::snapshot_session_header(&dsh_session::session_id("archive-queue"), None)
                .unwrap();
        let session = Session::from_event_archive(
            header.id.clone(),
            archive.finish().unwrap(),
            &header,
            SessionLogOffset::ZERO,
            vec![],
        )
        .unwrap();
        let mut projection = QueueProjection::default();
        let MuxFrame::SessionQueue { items, .. } = projection.snapshot(&session).unwrap() else {
            panic!("queue frame");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id.as_str(), "second");
        assert_eq!(items[0].placement, QueuedInboxPlacement::Queued);
        assert_eq!(items[1].id.as_str(), "steer");
        assert_eq!(items[1].placement, QueuedInboxPlacement::Steering);
        session
            .append(
                "agent/inbox/spliced",
                json!({"target":"next-turn","start":0,"removedCount":1,"inserted":[]}),
                None,
            )
            .unwrap();
        let MuxFrame::SessionQueue { items, .. } = projection.snapshot(&session).unwrap() else {
            panic!("queue frame");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.as_str(), "steer");
        let cursor = projection.sessions[session.id()].cursor;
        session
            .append(
                "agent/inbox/spliced",
                json!({"target":"next-turn","start":99,"inserted":[]}),
                None,
            )
            .unwrap();
        assert!(
            projection
                .snapshot(&session)
                .unwrap_err()
                .contains("exceeds its queue")
        );
        assert_eq!(projection.sessions[session.id()].cursor, cursor);
        assert!(projection.snapshot(&session).is_err());
    }
}
