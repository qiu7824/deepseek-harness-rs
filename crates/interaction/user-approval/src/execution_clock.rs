//! Execution budgets exclude the lifetime of awaited approval decisions.
use parking_lot::Mutex;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::time::Instant;

tokio::task_local! {
    static CLOCKS: Vec<Arc<ExecutionClock>>;
}

pub struct ExecutionClock {
    started: Instant,
    state: Mutex<State>,
    paused: tokio::sync::watch::Sender<bool>,
}
/// Explicit propagation for binding dispatch that crosses a Tokio task boundary.
#[derive(Clone)]
pub struct ExecutionClockContext(Vec<Arc<ExecutionClock>>);
impl ExecutionClockContext {
    pub async fn scope<F: Future>(&self, work: F) -> F::Output {
        CLOCKS.scope(self.0.clone(), work).await
    }
}
#[derive(Default)]
struct State {
    paused: Option<Instant>,
    excluded: Duration,
    depth: usize,
}
impl ExecutionClock {
    pub fn start() -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            state: Mutex::new(State::default()),
            paused: tokio::sync::watch::channel(false).0,
        })
    }
    pub fn elapsed(&self) -> Duration {
        let now = Instant::now();
        let state = self.state.lock();
        let excluded = state.excluded
            + state
                .paused
                .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        now.saturating_duration_since(self.started)
            .saturating_sub(excluded)
    }
    pub async fn scope<F: Future>(self: &Arc<Self>, work: F) -> F::Output {
        self.context().scope(work).await
    }
    pub fn context(self: &Arc<Self>) -> ExecutionClockContext {
        let mut clocks = CLOCKS.try_with(Clone::clone).unwrap_or_default();
        clocks.push(self.clone());
        ExecutionClockContext(clocks)
    }
    pub fn subscribe_pause(&self) -> tokio::sync::watch::Receiver<bool> {
        self.paused.subscribe()
    }
}

pub(crate) struct ApprovalPause(Vec<Arc<ExecutionClock>>);
pub(crate) fn pause() -> ApprovalPause {
    let clocks = CLOCKS.try_with(Clone::clone).unwrap_or_default();
    for clock in &clocks {
        let mut state = clock.state.lock();
        if state.depth == 0 {
            state.paused = Some(Instant::now());
            clock.paused.send_replace(true);
        }
        state.depth += 1;
    }
    ApprovalPause(clocks)
}
impl Drop for ApprovalPause {
    fn drop(&mut self) {
        for clock in &self.0 {
            let mut state = clock.state.lock();
            state.depth -= 1;
            if state.depth == 0
                && let Some(at) = state.paused.take()
            {
                state.excluded += Instant::now().saturating_duration_since(at);
                clock.paused.send_replace(false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn nested_execution_clocks_exclude_the_same_approval_once() {
        let outer = ExecutionClock::start();
        outer
            .scope(async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let inner = ExecutionClock::start();
                inner
                    .scope(async {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        let first = pause();
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        let nested = pause();
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        drop(nested);
                        drop(first);
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    })
                    .await;
                assert_eq!(inner.elapsed(), Duration::from_millis(20));
            })
            .await;
        assert_eq!(outer.elapsed(), Duration::from_millis(30));
    }
}
