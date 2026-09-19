//! Keep collection on the thread driving the root future. Tokio's worker
//! park hook does not run on the thread executing Runtime::block_on.

use std::future::Future;
use std::time::Duration;

pub(crate) async fn run<F: Future>(work: F, collect: impl FnMut()) -> F::Output {
    run_with_interval(work, collect, Duration::from_secs(1)).await
}

async fn run_with_interval<F: Future>(
    work: F,
    mut collect: impl FnMut(),
    period: Duration,
) -> F::Output {
    tokio::pin!(work);
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            result = &mut work => {
                collect();
                return result;
            }
            _ = interval.tick() => collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn idle_collection_runs_on_the_block_on_thread_and_returns_the_result() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let caller = std::thread::current().id();
        let calls = AtomicUsize::new(0);
        let result = runtime.block_on(run_with_interval(
            async {
                tokio::time::sleep(Duration::from_millis(35)).await;
                42
            },
            || {
                assert_eq!(std::thread::current().id(), caller);
                calls.fetch_add(1, Ordering::Relaxed);
            },
            Duration::from_millis(5),
        ));
        assert_eq!(result, 42);
        assert!(calls.load(Ordering::Relaxed) >= 2);
        let completed = calls.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(calls.load(Ordering::Relaxed), completed);
    }
}
