//! Request-lifetime cancellation for prompts waiting on admission.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use dsh_session::SessionId;
use parking_lot::Mutex;

use crate::fetch::handler::AbortSignal;

struct Pending {
    parent: SessionId,
    child: SessionId,
    stop: AbortSignal,
    cutoff: std::sync::Arc<Mutex<()>>,
}

#[derive(Default)]
pub(crate) struct PendingPromptAdmissions {
    next: AtomicU64,
    entries: Mutex<HashMap<u64, Pending>>,
}

pub(crate) struct PendingPrompt<'a> {
    owner: &'a PendingPromptAdmissions,
    id: u64,
    pub stop: AbortSignal,
    cutoff: std::sync::Arc<Mutex<()>>,
}

impl PendingPrompt<'_> {
    pub fn publish<T>(&self, commit: impl FnOnce(bool) -> T) -> T {
        let _cutoff = self.cutoff.lock();
        commit(self.stop.aborted())
    }
}

impl Drop for PendingPrompt<'_> {
    fn drop(&mut self) {
        self.owner.entries.lock().remove(&self.id);
    }
}

impl PendingPromptAdmissions {
    pub fn register(&self, parent: &SessionId, child: &SessionId) -> PendingPrompt<'_> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let stop = AbortSignal::new();
        let cutoff = std::sync::Arc::new(Mutex::new(()));
        self.entries.lock().insert(
            id,
            Pending {
                parent: parent.clone(),
                child: child.clone(),
                stop: stop.clone(),
                cutoff: cutoff.clone(),
            },
        );
        PendingPrompt {
            owner: self,
            id,
            stop,
            cutoff,
        }
    }

    pub fn interrupt(&self, parent: &SessionId, child: &SessionId) -> usize {
        let cutoffs: Vec<_> = self
            .entries
            .lock()
            .values()
            .filter(|pending| &pending.parent == parent && &pending.child == child)
            .map(|pending| {
                pending.stop.abort();
                pending.cutoff.clone()
            })
            .collect();
        // Do not hold the registry while a committed send publishes callbacks.
        for cutoff in &cutoffs {
            let _completed = cutoff.lock();
        }
        cutoffs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::session_id;

    #[test]
    fn early_stop_cancels_all_matching_admissions_and_releases_registration() {
        let pending = PendingPromptAdmissions::default();
        let parent = session_id("parent");
        let child = session_id("child");
        let first = pending.register(&parent, &child);
        let second = pending.register(&parent, &child);
        let unrelated = pending.register(&session_id("different-parent"), &child);
        let sibling = pending.register(&parent, &session_id("sibling"));
        pending.interrupt(&parent, &child);
        assert!(first.stop.aborted());
        assert!(second.stop.aborted());
        assert!(!unrelated.stop.aborted());
        assert!(!sibling.stop.aborted());
        let explicit_later = pending.register(&parent, &child);
        assert!(
            !explicit_later.stop.aborted(),
            "a later explicit followup is independent of the old stop"
        );
        drop((first, second, unrelated, sibling, explicit_later));
        assert!(
            pending.entries.lock().is_empty(),
            "no per-session cancellation map survives completed requests"
        );
    }

    #[tokio::test]
    async fn cold_prompt_can_stop_before_its_parent_admission_lock_opens() {
        let pending = PendingPromptAdmissions::default();
        let parent = session_id("parent");
        let child = session_id("child");
        let gate = tokio::sync::Mutex::new(());
        let held = gate.lock().await;
        let request = pending.register(&parent, &child);
        pending.interrupt(&parent, &child);
        tokio::select! {
            biased;
            _ = request.stop.cancelled() => {},
            _ = gate.lock() => panic!("cancelled request must not wait for or enter cold creation"),
        }
        assert!(request.stop.aborted());
        drop(held);
    }
}
