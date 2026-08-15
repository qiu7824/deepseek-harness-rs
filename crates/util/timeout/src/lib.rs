//! Shared timeout arithmetic, signal fusion, and classification. Rust port
//! of `@deepseek-ai/dsh-timeout`.
//!
//! The library only notifies through fused cancellation signals; each
//! capability still owns the mechanism that stops its work.
//!
//! # Deviations
//!
//! - `AbortSignal` becomes [`DeadlineSignal`] (a `Notify`-based fused signal
//!   carrying an optional [`TimeoutReason`]).
//! - `Symbol.dispose` becomes `Drop`-based timer cleanup.
//! - `idleWatchdog.next(iterator)` takes a [`std::future::Future`] demand
//!   instead of an async iterator (same protocol: one outstanding demand).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

/// Internal abort reason carrying a capability-owned code and elapsed
/// deadline (TS `TimeoutReason`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeoutReason {
    pub code: String,
    pub timeout_ms: u64,
}

impl TimeoutReason {
    pub fn new(code: impl Into<String>, timeout_ms: u64) -> Self {
        Self { code: code.into(), timeout_ms }
    }
}

impl fmt::Display for TimeoutReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} after {}ms", self.code, self.timeout_ms)
    }
}

impl std::error::Error for TimeoutReason {}

/// Largest delay Node schedules without clamping it to one millisecond.
pub const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;

fn assert_timer_delay(timeout_ms: u64, name: &str) {
    if timeout_ms == 0 || timeout_ms > MAX_TIMER_DELAY_MS {
        panic!("{name} must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}");
    }
}

/// Validate a caller's optional timeout hint, use the backend default, then
/// cap it (TS `clampTimeout`).
pub fn clamp_timeout(requested: Option<u64>, def: u64, max: u64, name: &str) -> u64 {
    if requested.is_some_and(|requested| requested == 0) {
        panic!("{name} must be a positive finite number");
    }
    requested.unwrap_or(def).min(max)
}

/// A fused cancellation signal carrying an optional [`TimeoutReason`].
pub struct DeadlineSignal {
    inner: Arc<SignalInner>,
}

struct SignalInner {
    cancelled: AtomicBool,
    reason: Mutex<Option<TimeoutReason>>,
    notify: Notify,
}

impl DeadlineSignal {
    fn new() -> Self {
        Self {
            inner: Arc::new(SignalInner {
                cancelled: AtomicBool::new(false),
                reason: Mutex::new(None),
                notify: Notify::new(),
            }),
        }
    }

    /// A signal that never aborts.
    pub fn never() -> Self {
        Self::new()
    }

    /// Abort this signal (idempotent), optionally with a timeout reason.
    /// The first reason wins (TS `AbortSignal.any` adopts the first abort).
    pub fn cancel(&self, reason: Option<TimeoutReason>) {
        self.cancel_after_observation(reason, || {});
    }

    fn cancel_after_observation(&self, reason: Option<TimeoutReason>, observed: impl FnOnce()) {
        if self.is_cancelled() {
            return;
        }
        observed();
        let mut current_reason = self.inner.reason.lock();
        if self.inner.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }
        *current_reason = reason;
        drop(current_reason);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// The abort reason (only set when a timeout caused the abort).
    pub fn reason(&self) -> Option<TimeoutReason> {
        self.inner.reason.lock().clone()
    }

    /// Resolve once cancelled (spurious-safe loop).
    pub async fn cancelled(&self) {
        self.cancelled_after_observation(|| {}).await;
    }

    async fn cancelled_after_observation(&self, observed: impl FnOnce()) {
        let mut observed = Some(observed);
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            if let Some(observed) = observed.take() {
                observed();
            }
            notified.await;
        }
    }

    /// Wait for cancellation, returning the timeout reason when present.
    pub async fn cancelled_with_reason(&self) -> Option<TimeoutReason> {
        self.cancelled().await;
        self.reason()
    }

    fn clone_signal(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

/// A deadline signal plus the cleanup that clears its timer (TS `Deadline`).
pub struct Deadline {
    pub signal: DeadlineSignal,
    timers: Option<TimerGroup>,
}

/// Drops abort every registered timer (TS `Symbol.dispose` cleanup).
struct TimerGroup(Arc<Mutex<Vec<AbortHandle>>>);

impl TimerGroup {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn add(&self, handle: AbortHandle) {
        self.0.lock().push(handle);
    }

    fn clear(&self) {
        for handle in self.0.lock().drain(..) {
            handle.abort();
        }
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        self.dispose();
    }
}

impl Deadline {
    /// Clear the timer (safe to call once).
    pub fn dispose(&mut self) {
        if let Some(timers) = self.timers.take() {
            timers.clear();
        }
    }
}

/// Fuse upstream cancellation with an identifiable timeout (TS `deadline`).
///
/// `timeout_ms == 0` is the internal no-timer sentinel: the result forwards
/// only the upstream signal (or a never-aborting one).
pub fn deadline(
    upstream: Option<&DeadlineSignal>,
    timeout_ms: u64,
    code: &str,
) -> Deadline {
    if timeout_ms == 0 {
        let signal = match upstream {
            Some(upstream) => upstream.clone_signal(),
            None => DeadlineSignal::new(),
        };
        return Deadline { signal, timers: None };
    }
    assert_timer_delay(timeout_ms, "deadline timeoutMs");

    let fused = DeadlineSignal::new();
    let timers = TimerGroup::new();
    let timeout_signal = fused.clone_signal();
    let code = code.to_string();
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
        timeout_signal.cancel(Some(TimeoutReason::new(code, timeout_ms)));
    });
    timers.add(task.abort_handle());
    if let Some(upstream) = upstream {
        let fused_for_upstream = fused.clone_signal();
        let upstream = upstream.clone_signal();
        let task = tokio::spawn(async move {
            upstream.cancelled().await;
            fused_for_upstream.cancel(None);
        });
        timers.add(task.abort_handle());
    }
    Deadline {
        signal: fused,
        timers: Some(timers),
    }
}

/// Failure returned by [`IdleWatchdog::next`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DemandError {
    /// The idle timer fired (the reason carries the capability code).
    Timeout(TimeoutReason),
    /// Upstream cancellation fired first (no timeout reason).
    Cancelled,
    /// The watchdog was disposed before the demand started.
    Disposed,
    /// Another demand is already outstanding.
    AlreadyOutstanding,
}

impl fmt::Display for DemandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DemandError::Timeout(reason) => write!(f, "{reason}"),
            DemandError::Cancelled => write!(f, "cancelled"),
            DemandError::Disposed => write!(f, "idleWatchdog is disposed"),
            DemandError::AlreadyOutstanding => {
                write!(f, "idleWatchdog next is already outstanding")
            }
        }
    }
}

impl std::error::Error for DemandError {}

struct WatchdogState {
    timer: Option<AbortHandle>,
    outstanding: bool,
    disposed: bool,
}

/// Rearmable timeout around one outstanding demand (TS `idleWatchdog`).
///
/// The stable fused signal aborts on upstream cancellation or the idle
/// timeout; a fired timer aborts it permanently (matching the TS stable
/// signal semantics).
pub struct IdleWatchdog {
    signal: DeadlineSignal,
    timeout_ms: u64,
    code: String,
    upstream_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    state: Arc<Mutex<WatchdogState>>,
}

impl IdleWatchdog {
    pub fn new(
        upstream: Option<&DeadlineSignal>,
        timeout_ms: u64,
        code: &str,
    ) -> Self {
        assert_timer_delay(timeout_ms, "idleWatchdog timeoutMs");
        let signal = DeadlineSignal::new();
        let upstream_task = upstream.map(|upstream| {
            let fused = signal.clone_signal();
            let upstream = upstream.clone_signal();
            tokio::spawn(async move {
                upstream.cancelled().await;
                fused.cancel(None);
            })
        });
        Self {
            signal,
            timeout_ms,
            code: code.to_string(),
            upstream_task: Mutex::new(upstream_task),
            state: Arc::new(Mutex::new(WatchdogState {
                timer: None,
                outstanding: false,
                disposed: false,
            })),
        }
    }

    /// The stable fused signal (TS `signal`).
    pub fn signal(&self) -> DeadlineSignal {
        self.signal.clone_signal()
    }

    fn arm(&self) {
        let mut state = self.state.lock();
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
        let fused = self.signal.clone_signal();
        let timeout_ms = self.timeout_ms;
        let code = self.code.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
            fused.cancel(Some(TimeoutReason::new(code, timeout_ms)));
        });
        state.timer = Some(task.abort_handle());
    }

    fn disarm(state: &Mutex<WatchdogState>) {
        let mut state = state.lock();
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
        state.outstanding = false;
    }

    /// Await one demand while the idle timer is armed (TS `next`).
    ///
    /// The outstanding marker is taken synchronously at call time (the TS
    /// async body runs to its first await on call); the returned future is
    /// `'static`.
    pub fn next<T: Send + 'static>(
        &self,
        demand: impl std::future::Future<Output = T> + Send + 'static,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, DemandError>> + Send>> {
        {
            let mut state = self.state.lock();
            if state.disposed {
                return Box::pin(async { Err(DemandError::Disposed) });
            }
            if state.outstanding {
                return Box::pin(async { Err(DemandError::AlreadyOutstanding) });
            }
            state.outstanding = true;
        }
        self.arm();

        let signal = self.signal.clone_signal();
        let state = self.state.clone();
        Box::pin(async move {
            tokio::select! {
                result = demand => {
                    Self::disarm(&state);
                    Ok(result)
                }
                _ = signal.cancelled() => {
                    Self::disarm(&state);
                    let reason = signal.reason();
                    Err(match reason {
                        Some(reason) => DemandError::Timeout(reason),
                        None => DemandError::Cancelled,
                    })
                }
            }
        })
    }

    /// Rearm an outstanding demand (TS `pulse`).
    pub fn pulse(&self) {
        let state = self.state.lock();
        if state.disposed || !state.outstanding {
            return;
        }
        drop(state);
        self.arm();
    }
}

impl Clone for IdleWatchdog {
    fn clone(&self) -> Self {
        // The upstream watcher task stays owned by the original; clones only
        // need the shared signal and state to pulse/observe.
        Self {
            signal: self.signal.clone_signal(),
            timeout_ms: self.timeout_ms,
            code: self.code.clone(),
            upstream_task: Mutex::new(None),
            state: self.state.clone(),
        }
    }
}

impl Drop for IdleWatchdog {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        state.disposed = true;
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
        if let Some(task) = self.upstream_task.lock().take() {
            task.abort();
        }
    }
}

/// Recover a timeout reason from a reason-bearing object (TS `timeoutOf`).
pub fn timeout_of(reason: Option<&TimeoutReason>, code: Option<&str>) -> Option<TimeoutReason> {
    let reason = reason?;
    match code {
        None => Some(reason.clone()),
        Some(code) if reason.code == code => Some(reason.clone()),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_timeout_rules() {
        assert_eq!(clamp_timeout(None, 100, 500, "timeoutMs"), 100);
        assert_eq!(clamp_timeout(Some(50), 100, 500, "timeoutMs"), 50);
        assert_eq!(clamp_timeout(Some(900), 100, 500, "timeoutMs"), 500);
        assert!(std::panic::catch_unwind(|| clamp_timeout(Some(0), 100, 500, "timeoutMs")).is_err());
    }

    #[tokio::test]
    async fn deadline_times_out_with_reason() {
        let mut deadline = deadline(None, 30, "TEST_TIMEOUT");
        let reason = deadline.signal.cancelled_with_reason().await;
        assert_eq!(reason.unwrap().code, "TEST_TIMEOUT");
        deadline.dispose();
    }

    #[tokio::test]
    async fn deadline_zero_never_fires() {
        let deadline = deadline(None, 0, "X");
        assert!(!deadline.signal.is_cancelled());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!deadline.signal.is_cancelled());
    }

    #[tokio::test]
    async fn upstream_wins_without_reason() {
        let upstream = DeadlineSignal::new();
        let deadline = deadline(Some(&upstream), 10_000, "X");
        upstream.cancel(None);
        let reason = deadline.signal.cancelled_with_reason().await;
        assert_eq!(reason, None);
    }

    #[test]
    fn concurrent_cancel_preserves_the_first_reason() {
        let signal = std::sync::Arc::new(DeadlineSignal::new());
        let observed = std::sync::Arc::new(std::sync::Barrier::new(2));
        let (first_done, wait_for_first) = std::sync::mpsc::sync_channel(0);

        let first_signal = std::sync::Arc::clone(&signal);
        let first_observed = std::sync::Arc::clone(&observed);
        let first = std::thread::spawn(move || {
            first_signal.cancel_after_observation(
                Some(TimeoutReason::new("FIRST", 10)),
                || {
                    first_observed.wait();
                },
            );
            first_done.send(()).expect("report first cancellation");
        });

        let second_signal = std::sync::Arc::clone(&signal);
        let second_observed = std::sync::Arc::clone(&observed);
        let second = std::thread::spawn(move || {
            second_signal.cancel_after_observation(
                Some(TimeoutReason::new("SECOND", 20)),
                || {
                    second_observed.wait();
                    wait_for_first.recv().expect("wait for first cancellation");
                },
            );
        });

        first.join().expect("first cancellation thread");
        second.join().expect("second cancellation thread");
        assert_eq!(signal.reason().expect("first reason retained").code, "FIRST");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_between_check_and_wait_is_not_lost() {
        let signal = std::sync::Arc::new(DeadlineSignal::new());
        let (observed, wait_for_observation) = std::sync::mpsc::sync_channel(0);
        let (release, wait_for_release) = std::sync::mpsc::sync_channel(0);
        let waiting_signal = std::sync::Arc::clone(&signal);
        let waiter = tokio::spawn(async move {
            waiting_signal
                .cancelled_after_observation(move || {
                    observed.send(()).expect("report cancellation check");
                    wait_for_release.recv().expect("release cancellation waiter");
                })
                .await;
        });

        wait_for_observation.recv().expect("waiter checked signal");
        signal.cancel(Some(TimeoutReason::new("BETWEEN", 30)));
        release.send(()).expect("release waiter");

        tokio::time::timeout(Duration::from_millis(200), waiter)
            .await
            .expect("registered waiter must observe cancellation")
            .expect("waiter task");
    }

    #[tokio::test]
    async fn watchdog_times_out_demand() {
        let watchdog = IdleWatchdog::new(None, 30, "IDLE");
        let result: Result<(), DemandError> =
            watchdog.next(std::future::pending::<()>()).await;
        assert!(matches!(result, Err(DemandError::Timeout(_))));
    }

    #[tokio::test]
    async fn watchdog_pulse_rearms() {
        let watchdog = IdleWatchdog::new(None, 25, "IDLE");
        let pulser = watchdog.clone();
        let demand = async move {
            tokio::time::sleep(Duration::from_millis(15)).await;
            pulser.pulse();
            tokio::time::sleep(Duration::from_millis(15)).await;
            pulser.pulse();
            tokio::time::sleep(Duration::from_millis(15)).await;
            7
        };
        let result = watchdog.next(demand).await;
        assert_eq!(result, Ok(7));
    }

    #[tokio::test]
    async fn watchdog_rejects_second_demand() {
        let watchdog = IdleWatchdog::new(None, 1000, "IDLE");
        let first = watchdog.next(std::future::pending::<()>());
        let second = watchdog.next(std::future::ready(()));
        assert!(matches!(second.await, Err(DemandError::AlreadyOutstanding)));
        drop(first);
    }

    #[test]
    fn timeout_of_filters_by_code() {
        let reason = TimeoutReason::new("A", 10);
        assert_eq!(timeout_of(Some(&reason), None).unwrap().code, "A");
        assert!(timeout_of(Some(&reason), Some("A")).is_some());
        assert!(timeout_of(Some(&reason), Some("B")).is_none());
        assert!(timeout_of(None, None).is_none());
    }
}
