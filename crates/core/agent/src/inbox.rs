//! Incremental projection of durable agent inbox events. Rust port of
//! `packages/core/agent/src/inbox.ts`.

use std::sync::Arc;

use dsh_llm::MessageId;
use dsh_session::{Session, UserMessage};
use parking_lot::Mutex;

use crate::types::{InboxSplice, InboxSpliceOutcome, InboxTarget};

/// Live notifications committed by inbox mutations.
#[derive(Clone, Default)]
pub struct InboxNotifications {
    /// Publish one inserted message.
    pub inserted: Option<Arc<dyn Fn(&UserMessage) + Send + Sync>>,
    /// Publish one discarded message.
    pub discarded: Option<Arc<dyn Fn(&UserMessage) + Send + Sync>>,
    /// Publish one claimed message inside its owning turn.
    pub claimed: Option<Arc<dyn Fn(&UserMessage, u64) + Send + Sync>>,
}

/// Mutable state privately owned by an [`Inbox`].
#[derive(Default)]
struct InboxState {
    next_turn: Vec<UserMessage>,
    next_step: Vec<UserMessage>,
}

/// A replay-once projection that incrementally consumes later inbox splices
/// (TS `Inbox`).
pub struct Inbox {
    session: Session,
    notifications: InboxNotifications,
    state: Mutex<InboxState>,
}

impl Inbox {
    /// Create the projection and replay the durable inbox splices from the
    /// seed boundary.
    pub fn new(session: &Session, notifications: InboxNotifications) -> Result<Inbox, String> {
        let inbox = Inbox {
            session: session.clone(),
            notifications,
            state: Mutex::new(InboxState::default()),
        };
        let seed_length = session.header().seed_length.unwrap_or(0) as usize;
        for event in session.events().iter().skip(seed_length) {
            if event.type_ != "agent/inbox/spliced" {
                continue;
            }
            let splice: InboxSplice = serde_json::from_value(event.data.clone())
                .map_err(|error| error.to_string())
                .map_err(|error| {
                    format!(
                        "invalid persisted inbox splice at session seq {}: {error}",
                        event.seq
                    )
                })?;
            inbox.apply(&splice).map_err(|error| {
                format!(
                    "invalid persisted inbox splice at session seq {}: {error}",
                    event.seq
                )
            })?;
        }
        Ok(inbox)
    }

    /// Prompts awaiting individual turns.
    pub fn next_turn(&self) -> Vec<UserMessage> {
        self.state.lock().next_turn.clone()
    }

    /// Input awaiting the next step boundary.
    pub fn next_step(&self) -> Vec<UserMessage> {
        self.state.lock().next_step.clone()
    }

    /// Whether either pending-message list contains work.
    pub fn has_pending(&self) -> bool {
        let state = self.state.lock();
        !state.next_turn.is_empty() || !state.next_step.is_empty()
    }

    /// Durably cancel all pending input, clearing next-step before
    /// next-turn.
    pub fn clear(&self) -> Result<(), String> {
        let next_step_len = self.state.lock().next_step.len();
        self.splice(InboxTarget::NextStep, 0.0, next_step_len as f64, Vec::new())?;
        let next_turn_len = self.state.lock().next_turn.len();
        self.splice(InboxTarget::NextTurn, 0.0, next_turn_len as f64, Vec::new())?;
        Ok(())
    }

    /// Remove and return the complete batch proposed for one step,
    /// publishing each claimed message (TS `Inbox.claim`).
    pub fn claim(&self, target: InboxTarget, turn: u64) -> Result<Vec<UserMessage>, String> {
        let step_len = self.state.lock().next_step.len();
        let mut claimed = self.mutate(
            InboxTarget::NextStep,
            0.0,
            step_len as f64,
            Vec::new(),
            false,
        )?;
        if target == InboxTarget::NextTurn {
            claimed.extend(self.mutate(InboxTarget::NextTurn, 0.0, 1.0, Vec::new(), false)?);
        }
        if let Some(notify) = &self.notifications.claimed {
            for message in &claimed {
                notify(message, turn);
            }
        }
        Ok(claimed)
    }

    /// Append one message to a pending list and durably record the
    /// insertion.
    pub fn append(&self, target: InboxTarget, message: UserMessage) -> Result<(), String> {
        let length = self.list_len(target);
        self.splice(target, length as f64, 0.0, vec![message])?;
        Ok(())
    }

    /// Prepend one message to a pending list and durably record the
    /// insertion.
    pub fn prepend(&self, target: InboxTarget, message: UserMessage) -> Result<(), String> {
        self.splice(target, 0.0, 0.0, vec![message])?;
        Ok(())
    }

    /// Replace one pending message in place, possibly changing its identity.
    pub fn replace(
        &self,
        message_id: &MessageId,
        new_message: UserMessage,
    ) -> Result<bool, String> {
        let Some(location) = self.locate(message_id) else {
            return Ok(false);
        };
        self.splice(
            location.target,
            location.index as f64,
            1.0,
            vec![new_message],
        )?;
        Ok(true)
    }

    /// Remove one pending message and durably record its cancellation.
    pub fn remove(&self, message_id: &MessageId) -> Result<bool, String> {
        let Some(location) = self.locate(message_id) else {
            return Ok(false);
        };
        self.splice(location.target, location.index as f64, 1.0, Vec::new())?;
        Ok(true)
    }

    /// Apply standard splice semantics and durably record the normalized
    /// result.
    pub fn splice(
        &self,
        target: InboxTarget,
        start: f64,
        delete_count: f64,
        inserted: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, String> {
        self.mutate(target, start, delete_count, inserted, true)
    }

    fn list_len(&self, target: InboxTarget) -> usize {
        let state = self.state.lock();
        match target {
            InboxTarget::NextTurn => state.next_turn.len(),
            InboxTarget::NextStep => state.next_step.len(),
        }
    }

    /// Locate one pending identity across both owned lists.
    fn locate(&self, message_id: &MessageId) -> Option<Location> {
        let state = self.state.lock();
        if let Some(index) = state
            .next_turn
            .iter()
            .position(|message| message.id == *message_id)
        {
            return Some(Location {
                target: InboxTarget::NextTurn,
                index,
            });
        }
        if let Some(index) = state
            .next_step
            .iter()
            .position(|message| message.id == *message_id)
        {
            return Some(Location {
                target: InboxTarget::NextStep,
                index,
            });
        }
        None
    }

    /// Commit one normalized mutation and publish its live notifications
    /// (TS `Inbox.mutate`). The durable event commits BEFORE the live
    /// projection mutates.
    fn mutate(
        &self,
        target: InboxTarget,
        start: f64,
        delete_count: f64,
        inserted: Vec<UserMessage>,
        discard_removed: bool,
    ) -> Result<Vec<UserMessage>, String> {
        let list_len = self.list_len(target);
        let truncated_start = start.trunc();
        let offset = if truncated_start.is_nan() {
            0.0
        } else {
            truncated_start
        };
        let actual_start = if offset < 0.0 {
            (list_len as f64 + offset).max(0.0) as usize
        } else {
            (offset as usize).min(list_len)
        };
        let truncated_delete = delete_count.trunc();
        let actual_delete = ((if truncated_delete.is_nan() {
            0.0
        } else {
            truncated_delete
        })
        .max(0.0) as usize)
            .min(list_len - actual_start);
        if actual_delete == 0 && inserted.is_empty() {
            return Ok(Vec::new());
        }
        let outcome = if discard_removed && actual_delete > 0 {
            Some(InboxSpliceOutcome::Canceled)
        } else {
            None
        };
        let splice = InboxSplice {
            target,
            start: actual_start as u64,
            removed_count: if actual_delete == 0 {
                None
            } else {
                Some(actual_delete as u64)
            },
            inserted: inserted.clone(),
            outcome,
        };
        self.validate(&splice)?;
        let data = serde_json::to_value(&splice)
            .map_err(|error| format!("inbox splice is not JSON-serializable: {error}"))?;
        // The durable event commits BEFORE the live projection mutates (the
        // TS ordering: `session.append` first, then the local splice).
        let event = self.session.append("agent/inbox/spliced", data, None)?;
        let logged: InboxSplice =
            serde_json::from_value(event.data).map_err(|error| error.to_string())?;
        let removed = {
            let mut state = self.state.lock();
            let list = match target {
                InboxTarget::NextTurn => &mut state.next_turn,
                InboxTarget::NextStep => &mut state.next_step,
            };
            list.splice(
                actual_start..actual_start + actual_delete,
                logged.inserted.clone(),
            )
            .collect()
        };
        if discard_removed {
            if let Some(notify) = &self.notifications.discarded {
                for message in &removed {
                    notify(message);
                }
            }
        }
        if let Some(notify) = &self.notifications.inserted {
            for message in &logged.inserted {
                notify(message);
            }
        }
        Ok(removed)
    }

    /// Apply one normalized durable splice to the projection (TS
    /// `Inbox.apply`).
    fn apply(&self, splice: &InboxSplice) -> Result<Vec<UserMessage>, String> {
        self.validate(splice)?;
        let removed_count = splice.removed_count.unwrap_or(0) as usize;
        let start = splice.start as usize;
        let mut state = self.state.lock();
        let list = match splice.target {
            InboxTarget::NextTurn => &mut state.next_turn,
            InboxTarget::NextStep => &mut state.next_step,
        };
        Ok(list
            .splice(start..start + removed_count, splice.inserted.clone())
            .collect())
    }

    /// Validate one normalized splice against the current projection.
    fn validate(&self, splice: &InboxSplice) -> Result<(), String> {
        let list_len = self.list_len(splice.target);
        let removed_count = splice.removed_count.unwrap_or(0);
        if splice.start > list_len as u64 || splice.start + removed_count > list_len as u64 {
            return Err("invalid inbox splice".to_string());
        }
        let state = self.state.lock();
        let mut candidate = match splice.target {
            InboxTarget::NextTurn => state.next_turn.clone(),
            InboxTarget::NextStep => state.next_step.clone(),
        };
        candidate.splice(
            splice.start as usize..splice.start as usize + removed_count as usize,
            splice.inserted.clone(),
        );
        let mut ids = std::collections::HashSet::new();
        let ordered: Vec<&UserMessage> = match splice.target {
            InboxTarget::NextTurn => candidate.iter().chain(state.next_step.iter()).collect(),
            InboxTarget::NextStep => state.next_turn.iter().chain(candidate.iter()).collect(),
        };
        for message in ordered {
            let id = message.id.as_str().to_string();
            if !ids.insert(id.clone()) {
                return Err(format!("message \"{id}\" is already pending"));
            }
        }
        Ok(())
    }
}

/// One located pending identity.
struct Location {
    target: InboxTarget,
    index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_llm::{ContentBlock, MessageSource};
    use dsh_session::{SurfaceIntent, SurfaceOp, session_id};

    fn message(id: &str) -> UserMessage {
        dsh_llm::create_message(
            dsh_llm::Role::User,
            vec![ContentBlock::Text {
                text: id.to_string(),
            }],
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        )
        // Override the fresh random identity with a deterministic one.
        .let_override(id)
    }

    trait IdOverride {
        fn let_override(self, id: &str) -> UserMessage;
    }

    impl IdOverride for UserMessage {
        fn let_override(mut self, id: &str) -> UserMessage {
            self.id = dsh_llm::message_id(id);
            self
        }
    }

    fn test_inbox() -> Inbox {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        Inbox::new(&session, InboxNotifications::default()).unwrap()
    }

    #[test]
    fn append_and_lists() {
        let inbox = test_inbox();
        inbox.append(InboxTarget::NextTurn, message("m1")).unwrap();
        inbox.append(InboxTarget::NextStep, message("m2")).unwrap();
        assert_eq!(inbox.next_turn().len(), 1);
        assert_eq!(inbox.next_step().len(), 1);
        assert!(inbox.has_pending());
    }

    #[test]
    fn splice_logs_before_mutation() {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        let inbox = Inbox::new(&session, InboxNotifications::default()).unwrap();
        inbox.append(InboxTarget::NextTurn, message("m1")).unwrap();
        // The durable event exists on the session log.
        assert_eq!(session.events().len(), 1);
        assert_eq!(session.events()[0].type_, "agent/inbox/spliced");
        assert_eq!(session.events()[0].data["target"], "next-turn");
        assert_eq!(session.events()[0].data["inserted"][0]["id"], "m1");
    }

    #[test]
    fn duplicate_identity_rejects() {
        let inbox = test_inbox();
        inbox.append(InboxTarget::NextTurn, message("dup")).unwrap();
        let error = inbox
            .append(InboxTarget::NextStep, message("dup"))
            .unwrap_err();
        assert!(error.contains("already pending"), "{error}");
    }

    #[test]
    fn replace_publishes_discard_and_insert() {
        let discarded = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let inserted = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let discarded_listener = discarded.clone();
        let inserted_listener = inserted.clone();
        let session = Session::create(session_id("s1"), None, None).unwrap();
        let inbox = Inbox::new(
            &session,
            InboxNotifications {
                discarded: Some(Arc::new(move |message: &UserMessage| {
                    discarded_listener
                        .lock()
                        .push(message.id.as_str().to_string());
                })),
                inserted: Some(Arc::new(move |message: &UserMessage| {
                    inserted_listener
                        .lock()
                        .push(message.id.as_str().to_string());
                })),
                claimed: None,
            },
        )
        .unwrap();
        inbox.append(InboxTarget::NextTurn, message("old")).unwrap();
        assert!(
            inbox
                .replace(&dsh_llm::message_id("old"), message("new"))
                .unwrap()
        );
        assert!(
            inbox
                .replace(&dsh_llm::message_id("gone"), message("x"))
                .unwrap()
                == false
        );
        assert_eq!(inbox.next_turn()[0].id.as_str(), "new");
        assert_eq!(&*discarded.lock(), &["old"]);
        assert_eq!(&*inserted.lock(), &["old", "new"]);
    }

    #[test]
    fn remove_and_clear() {
        let inbox = test_inbox();
        inbox.append(InboxTarget::NextTurn, message("m1")).unwrap();
        inbox.append(InboxTarget::NextStep, message("m2")).unwrap();
        assert!(inbox.remove(&dsh_llm::message_id("m1")).unwrap());
        assert!(!inbox.remove(&dsh_llm::message_id("m1")).unwrap());
        inbox.clear().unwrap();
        assert!(!inbox.has_pending());
    }

    #[test]
    fn claim_consumes_next_step_then_one_turn() {
        let inbox = test_inbox();
        inbox.append(InboxTarget::NextTurn, message("t1")).unwrap();
        inbox.append(InboxTarget::NextTurn, message("t2")).unwrap();
        inbox.append(InboxTarget::NextStep, message("s1")).unwrap();
        let claimed = inbox.claim(InboxTarget::NextTurn, 3).unwrap();
        assert_eq!(
            claimed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["s1", "t1"]
        );
        assert_eq!(inbox.next_turn().len(), 1);
        assert_eq!(inbox.next_step().len(), 0);

        let claimed = inbox.claim(InboxTarget::NextStep, 4).unwrap();
        assert!(claimed.is_empty());
        assert_eq!(
            inbox.next_turn().len(),
            1,
            "next-step claim consumes no queued turn"
        );
    }

    #[test]
    fn splice_coordinates_clamp_like_array_splice() {
        let inbox = test_inbox();
        // TS `Array.prototype.splice` semantics: an out-of-range start clamps
        // to the list length, so 5.0 on an empty list inserts at 0.
        inbox
            .splice(InboxTarget::NextTurn, 5.0, 0.0, vec![message("m1")])
            .unwrap();
        assert_eq!(inbox.next_turn()[0].id.as_str(), "m1");

        // A start beyond the list clamps to the end; the deleteCount then
        // applies past the tail, so nothing is removed and the insert lands
        // at the end.
        let inbox = test_inbox();
        inbox.append(InboxTarget::NextTurn, message("x")).unwrap();
        let removed = inbox
            .splice(InboxTarget::NextTurn, 2.0, 3.0, vec![message("y")])
            .unwrap();
        assert_eq!(
            removed.len(),
            0,
            "start clamps to the list end before deletes apply"
        );
        assert_eq!(inbox.next_turn().len(), 2);
        assert_eq!(inbox.next_turn()[1].id.as_str(), "y");
    }

    #[test]
    fn replay_applies_durable_splices() {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        let inbox = Inbox::new(&session, InboxNotifications::default()).unwrap();
        inbox.append(InboxTarget::NextTurn, message("m1")).unwrap();
        inbox.append(InboxTarget::NextStep, message("m2")).unwrap();

        // A second projection over the same session replays the durable log.
        let replay = Inbox::new(&session, InboxNotifications::default()).unwrap();
        assert_eq!(replay.next_turn()[0].id.as_str(), "m1");
        assert_eq!(replay.next_step()[0].id.as_str(), "m2");
    }

    #[test]
    fn seeded_sessions_start_at_seed_boundary() {
        let seed = vec![dsh_session::SessionEvent {
            type_: "agent/inbox/spliced".to_string(),
            seq: 0,
            time: 0,
            data: serde_json::json!({
                "target": "next-turn",
                "start": 0,
                "inserted": [{
                    "id": "seed-message",
                    "role": "user",
                    "content": [{"type": "text", "text": "seed"}],
                    "source": {"kind": "user"},
                }],
            }),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }];
        let header = dsh_session::SessionHeader {
            version: dsh_session::SESSION_FORMAT_VERSION,
            id: session_id("s1"),
            created_at: 1,
            cwd: None,
            parent_session: None,
            seed_length: Some(1),
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        let session = Session::create(session_id("s1"), Some(seed), Some(&header)).unwrap();
        // Events BEFORE the seed boundary are not replayed by the projection.
        let inbox = Inbox::new(&session, InboxNotifications::default()).unwrap();
        assert!(
            !inbox.has_pending(),
            "seed-boundary splices are not replayed"
        );
    }

    #[test]
    fn surface_intent_is_not_required_for_inbox_events() {
        let _ = SurfaceIntent {
            surface_op: SurfaceOp::Append,
            source_event_seqs: None,
        };
        let session = Session::create(session_id("s1"), None, None).unwrap();
        let inbox = Inbox::new(&session, InboxNotifications::default()).unwrap();
        // append() with no surface intent — the inbox splice is log-only.
        inbox.append(InboxTarget::NextTurn, message("m1")).unwrap();
        let event = &session.events()[0];
        assert!(event.surface_op.is_none());
        assert!(event.source_event_seqs.is_none());
    }
}
