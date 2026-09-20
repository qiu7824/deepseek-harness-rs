use dsh_agent::CancellationSignal;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_wakes_all_waiters_and_is_retained_for_late_waiters() {
    for _ in 0..100 {
        let signal = CancellationSignal::new();
        let waiters: Vec<_> = (0..16)
            .map(|_| {
                let signal = signal.clone();
                tokio::spawn(async move {
                    signal.cancelled().await;
                })
            })
            .collect();
        tokio::task::yield_now().await;
        signal.abort();
        tokio::time::timeout(Duration::from_secs(1), async {
            for waiter in waiters {
                waiter.await.unwrap();
            }
            signal.cancelled().await;
        })
        .await
        .expect("cancellation wakeup lost");
    }
}
