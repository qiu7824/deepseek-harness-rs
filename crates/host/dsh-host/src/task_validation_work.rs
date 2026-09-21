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
#[derive(Clone, Default)]
pub(super) struct Work(Arc<parking_lot::Mutex<ActiveWork>>);
pub(super) struct Guard {
    cancelled: Arc<AtomicBool>,
    pub signal: AbortPredicate,
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
        let mut active = self.0.lock();
        prune(&mut active);
        let requests = active.entry(key).or_default();
        for previous in requests.iter().filter_map(Weak::upgrade) {
            previous.store(true, Ordering::Release);
        }
        requests.push(Arc::downgrade(&cancelled));
        let flag = cancelled.clone();
        Guard {
            cancelled,
            signal: Arc::new(move || flag.load(Ordering::Acquire) || upstream()),
        }
    }
    pub fn cancel(&self, owner: &str, task: &str) {
        let mut active = self.0.lock();
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
        let mut active = self.0.lock();
        prune(&mut active);
        if active
            .keys()
            .any(|(registered_owner, _)| registered_owner == owner)
        {
            return None;
        }
        Some(operation())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
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
        assert!(work.0.lock().is_empty());
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
