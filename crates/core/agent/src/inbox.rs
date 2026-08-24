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
    mutation_owner: Mutex<Option<std::thread::ThreadId>>,
    mutation_released: parking_lot::Condvar,
}

struct MutationGuard<'a>(&'a Inbox);

impl Drop for MutationGuard<'_> {
    fn drop(&mut self) {
        *self.0.mutation_owner.lock() = None;
        self.0.mutation_released.notify_one();
    }
}

impl Inbox {
    fn begin_mutation(&self) -> Result<MutationGuard<'_>, String> {
        let current = std::thread::current().id();
        let mut owner = self.mutation_owner.lock();
        loop {
            match *owner {
                None => {
                    *owner = Some(current);
                    return Ok(MutationGuard(self));
                }
                Some(active) if active == current => {
                    return Err(
                        "inbox mutation cannot reenter while another mutation is being published"
                            .to_string(),
                    );
                }
                Some(_) => self.mutation_released.wait(&mut owner),
            }
        }
    }

    /// Create the projection and replay the durable inbox splices from the
    /// seed boundary.
    pub fn new(session: &Session, notifications: InboxNotifications) -> Result<Inbox, String> {
        let inbox = Inbox {
            session: session.clone(),
            notifications,
            state: Mutex::new(InboxState::default()),
            mutation_owner: Mutex::new(None),
            mutation_released: parking_lot::Condvar::new(),
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
        let _mutation = self.begin_mutation()?;
        let next_step_len = self.state.lock().next_step.len();
        self.mutate_locked(
            InboxTarget::NextStep,
            0.0,
            next_step_len as f64,
            Vec::new(),
            true,
        )?;
        let next_turn_len = self.state.lock().next_turn.len();
        self.mutate_locked(
            InboxTarget::NextTurn,
            0.0,
            next_turn_len as f64,
            Vec::new(),
            true,
        )?;
        Ok(())
    }

    /// Remove and return the complete batch proposed for one step,
    /// publishing each claimed message (TS `Inbox.claim`).
    pub fn claim(&self, target: InboxTarget, turn: u64) -> Result<Vec<UserMessage>, String> {
        let _mutation = self.begin_mutation()?;
        let step_len = self.state.lock().next_step.len();
        let mut claimed = self.mutate_locked(
            InboxTarget::NextStep,
            0.0,
            step_len as f64,
            Vec::new(),
            false,
        )?;
        if target == InboxTarget::NextTurn {
            claimed.extend(self.mutate_locked(
                InboxTarget::NextTurn,
                0.0,
                1.0,
                Vec::new(),
                false,
            )?);
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
        let _mutation = self.begin_mutation()?;
        let length = self.list_len(target);
        self.mutate_locked(target, length as f64, 0.0, vec![message], true)?;
        Ok(())
    }

    /// Prepend one message to a pending list and durably record the
    /// insertion.
    pub fn prepend(&self, target: InboxTarget, message: UserMessage) -> Result<(), String> {
        let _mutation = self.begin_mutation()?;
        self.mutate_locked(target, 0.0, 0.0, vec![message], true)?;
        Ok(())
    }

    /// Replace one pending message in place, possibly changing its identity.
    pub fn replace(
        &self,
        message_id: &MessageId,
        new_message: UserMessage,
    ) -> Result<bool, String> {
        let _mutation = self.begin_mutation()?;
        let Some(location) = self.locate(message_id) else {
            return Ok(false);
        };
        self.mutate_locked(
            location.target,
            location.index as f64,
            1.0,
            vec![new_message],
            true,
        )?;
        Ok(true)
    }

    /// Remove one pending message and durably record its cancellation.
    pub fn remove(&self, message_id: &MessageId) -> Result<bool, String> {
        let _mutation = self.begin_mutation()?;
        let Some(location) = self.locate(message_id) else {
            return Ok(false);
        };
        self.mutate_locked(
            location.target,
            location.index as f64,
            1.0,
            Vec::new(),
            true,
        )?;
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
        let _mutation = self.begin_mutation()?;
        self.mutate_locked(target, start, delete_count, inserted, true)
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
    fn mutate_locked(
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
        if discard_removed && let Some(notify) = &self.notifications.discarded {
            for message in &removed {
                notify(message);
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
