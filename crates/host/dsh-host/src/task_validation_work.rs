//! Request-owned cancellation for task validation and immutable input reads.
use dsh_tools::AbortPredicate;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

type ActiveWork = BTreeMap<(String, String), Vec<Weak<AtomicBool>>>;
#[derive(Default)]
struct State {
    active: parking_lot::Mutex<ActiveWork>,
    commit: parking_lot::ReentrantMutex<()>,
}
#[derive(Clone, Default)]
pub(super) struct Work(Arc<State>);
pub(super) struct Guard {
    work: Work,
    cancelled: Arc<AtomicBool>,
    pub signal: AbortPredicate,
}
pub(super) struct CommitGuard<'a>(parking_lot::ReentrantMutexGuard<'a, ()>);
impl Guard {
    /// Held only through a synchronous final state check and append. Cancellation
    /// cannot acknowledge before that append once this boundary has been entered.
    pub fn commit(&self) -> Result<CommitGuard<'_>, String> {
        let guard = self.work.0.commit.lock();
        if (self.signal)() {
            return Err("Task validation cancelled".into());
        }
        Ok(CommitGuard(guard))
    }
}
fn prune(active: &mut ActiveWork) {
    active.retain(|_, work| {
        work.retain(|flag| flag.strong_count() > 0);
        !work.is_empty()
    });
}
impl Work {
    pub fn begin(&self, owner: &str, task: &str, upstream: AbortPredicate) -> Guard {
        let key = (owner.to_owned(), task.to_owned());
        let cancelled = Arc::new(AtomicBool::new(false));
        let _commit = self.0.commit.lock();
        let mut active = self.0.active.lock();
        prune(&mut active);
        let requests = active.entry(key).or_default();
        for previous in requests.iter().filter_map(Weak::upgrade) {
            previous.store(true, Ordering::Release);
        }
        requests.push(Arc::downgrade(&cancelled));
        let flag = cancelled.clone();
        Guard {
            work: self.clone(),
            cancelled,
            signal: Arc::new(move || flag.load(Ordering::Acquire) || upstream()),
        }
    }
    pub fn cancel(&self, owner: &str, task: &str) {
        let _commit = self.0.commit.lock();
        let mut active = self.0.active.lock();
        prune(&mut active);
        if let Some(requests) = active.get(&(owner.to_owned(), task.to_owned())) {
            for flag in requests.iter().filter_map(Weak::upgrade) {
                flag.store(true, Ordering::Release);
            }
        }
    }
    /// A cancelled request remains active until all blocking workers release
    /// their signal. Hold admission during the short synchronous contract commit.
    pub fn with_idle<T>(&self, owner: &str, operation: impl FnOnce() -> T) -> Option<T> {
        self.with_owners_idle(&[owner], operation)
    }

    pub fn with_owners_idle<T>(&self, owners: &[&str], operation: impl FnOnce() -> T) -> Option<T> {
        let _commit = self.0.commit.lock();
        let mut active = self.0.active.lock();
        prune(&mut active);
        if active
            .keys()
            .any(|(registered_owner, _)| owners.contains(&registered_owner.as_str()))
        {
            return None;
        }
        Some(operation())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _commit = self.work.0.commit.lock();
        self.cancelled.store(true, Ordering::Release);
    }
}
pub(super) async fn until_cancelled<T>(
    future: impl std::future::Future<Output = Result<T, String>>,
    signal: AbortPredicate,
) -> Result<T, String> {
    tokio::pin!(future);
    loop {
        if signal() {
            return Err("Task validation cancelled".into());
        }
        tokio::select! {
            result = &mut future => { if signal() { return Err("Task validation cancelled".into()); } return result; },
            _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requirement_changes_wait_for_cancelled_descendant_validation_workers_to_release() {
        let work = Work::default();
        let child = work.begin("child", "task", Arc::new(|| false));
        assert!(work.with_owners_idle(&["parent", "child"], || ()).is_none());
        work.cancel("child", "task");
        assert!(work.with_owners_idle(&["parent", "child"], || ()).is_none());
        assert_eq!(work.with_owners_idle(&["unrelated"], || 42), Some(42));
        drop(child);
        assert_eq!(work.with_owners_idle(&["parent", "child"], || 42), Some(42));
    }
    #[test]
    fn stop_between_verification_and_commit_keeps_the_same_cancelled_guard() {
        use std::sync::{Barrier, mpsc};
        let work = Work::default();
        let verifying = work.clone();
        let release = Arc::new(Barrier::new(2));
        let worker_release = release.clone();
        let (hashed_tx, hashed_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let guard = verifying.begin("owner", "task", Arc::new(|| false));
            let identity = dsh_task_runtime::digest(b"verified input");
            hashed_tx.send(identity).unwrap();
            worker_release.wait();
            guard
        });
        assert!(!hashed_rx.recv().unwrap().is_empty());
        work.cancel("owner", "task");
        release.wait();
        let same_guard = worker.join().unwrap();
        assert!((same_guard.signal)());
        assert!(
            same_guard.commit().is_err(),
            "handoff to a completion permit must not reset StopValidation"
        );
    }
    #[test]
    fn cancel_ack_cannot_overtake_an_admitted_final_commit() {
        use std::sync::{Barrier, mpsc};
        let work = Work::default();
        let guard = work.begin("owner", "task", Arc::new(|| false));
        let commit = guard.commit().unwrap();
        let started = Arc::new(Barrier::new(2));
        let concurrent = started.clone();
        let cancelling = work.clone();
        let appended = Arc::new(AtomicBool::new(false));
        let observed = appended.clone();
        let (ack_tx, ack_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            concurrent.wait();
            cancelling.cancel("owner", "task");
            ack_tx.send(observed.load(Ordering::Acquire)).unwrap();
        });
        started.wait();
        assert!(matches!(
            ack_rx.recv_timeout(std::time::Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        appended.store(true, Ordering::Release);
        drop(commit);
        assert!(
            ack_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
        );
        worker.join().unwrap();
        assert!((guard.signal)());
    }
    #[test]
    fn synchronous_post_append_notifications_can_cancel_without_deadlock() {
        let work = Work::default();
        let guard = work.begin("owner", "task", Arc::new(|| false));
        let commit = guard.commit().unwrap();
        // The durable append has happened before its synchronous notifications.
        work.cancel("owner", "task");
        assert!((guard.signal)());
        drop(commit);
        assert!(guard.commit().is_err());
    }
    #[test]
    fn replaced_and_dropped_requests_do_not_cancel_their_successor_or_another_owner() {
        let work = Work::default();
        let old = work.begin("a", "t", Arc::new(|| false));
        let next = work.begin("a", "t", Arc::new(|| false));
        let other = work.begin("b", "t", Arc::new(|| false));
        assert!((old.signal)());
        drop(old);
        assert!(!(next.signal)());
        work.cancel("a", "t");
        assert!((next.signal)());
        assert!(!(other.signal)());
        let signal = other.signal.clone();
        drop(other);
        assert!(signal());
        drop(next);
        drop(signal);
        assert_eq!(work.with_idle("a", || 42), Some(42));
        assert!(work.0.active.lock().is_empty());
    }
    #[test]
    fn editing_waits_for_cancelled_and_superseded_workers_to_settle() {
        let work = Work::default();
        let old = work.begin("owner", "task", Arc::new(|| false));
        let worker_signal = old.signal.clone();
        let newer = work.begin("owner", "task", Arc::new(|| false));
        drop(old);
        work.cancel("owner", "task");
        drop(newer);
        assert!(worker_signal());
        assert_eq!(work.with_idle("owner", || true), None);
        assert_eq!(work.with_idle("other", || true), Some(true));
        drop(worker_signal);
        assert_eq!(work.with_idle("owner", || true), Some(true));
    }
    #[tokio::test]
    async fn request_drop_interrupts_pending_provider_setup() {
        let work = Work::default();
        let guard = work.begin("owner", "t", Arc::new(|| false));
        let pending = until_cancelled(
            std::future::pending::<Result<(), String>>(),
            guard.signal.clone(),
        );
        drop(guard);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), pending)
                .await
                .unwrap()
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[tokio::test]
    async fn user_cancel_stops_blocking_checker_io_before_completion() {
        use dsh_task_runtime::{AcceptanceCheck, AcceptanceStatus, Checker, check_file};
        use std::{io::Write, sync::atomic::AtomicUsize};
        let mut file = tempfile::tempfile().unwrap();
        for _ in 0..128 {
            file.write_all(&[b'x'; 65536]).unwrap();
        }
        let work = Work::default();
        let guard = work.begin("owner", "task", Arc::new(|| false));
        let signal = guard.signal.clone();
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let check = AcceptanceCheck {
                id: "text".into(),
                description: "text".into(),
                checker: Checker::Text {
                    path: "file.txt".into(),
                    required: vec!["x".into()],
                    forbidden: vec![],
                },
            };
            let signal = move || {
                counted.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(1));
                signal()
            };
            check_file(&check, &mut file, Some(&signal))
        });
        while reads.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        work.cancel("owner", "task");
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.status, AcceptanceStatus::Failed);
        assert!(result.failure_reason.unwrap().contains("cancelled"));
        assert!(
            reads.load(Ordering::SeqCst) < 128,
            "cancel stopped IO rather than waiting for the complete file"
        );
    }
}
