//! Timer service for cordis: Rust port of `@deepseek-ai/cordis-plugin-timer`
//! v1.1.3.
//!
//! The TS service mixes `timeout`/`interval`/`throttle`/`debounce` onto every
//! context and registers every timer as a fiber effect (auto-disposed on
//! unload). Rust exposes the same helpers on [`TimerService`], which reads the
//! *caller* context explicitly.
//!
//! # Deviations
//!
//! - `timeout(delay)` returns a boxed future instead of a JS `Promise`.
//! - `interval(delay)` returns an `mpsc::UnboundedReceiver` of ticks instead
//!   of an async iterator; disposal closes the channel rather than rejecting
//!   the next `next()` call.
//! - Timers use `tokio::time` (millisecond `u64` delays).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cordis::{ArcValue, Context, Disposer, Plugin, PluginError, Service, make_disposer};
use tokio::task::JoinHandle;

type TimerTrigger = Arc<dyn Fn(Vec<ArcValue>, &AtomicBool) -> Option<JoinHandle<()>> + Send + Sync>;

/// Timer cancellation error (TS `Error('Context has been disposed')`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerError {
    Disposed,
}

impl std::fmt::Display for TimerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimerError::Disposed => write!(f, "Context has been disposed"),
        }
    }
}

impl std::error::Error for TimerError {}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Disposable timer helpers (TS `TimerService`).
pub struct TimerService;

impl TimerService {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }

    /// @deprecated use [`TimerService::timeout`] instead.
    pub fn set_timeout(
        &self,
        caller: &Context,
        callback: Arc<dyn Fn() + Send + Sync>,
        delay_ms: u64,
    ) -> Disposer {
        self.timeout(caller, callback, delay_ms)
    }

    /// @deprecated use [`TimerService::interval`] instead.
    pub fn set_interval(
        &self,
        caller: &Context,
        callback: Arc<dyn Fn() + Send + Sync>,
        delay_ms: u64,
    ) -> Disposer {
        self.interval(caller, callback, delay_ms)
    }

    /// Run a callback once after `delay_ms`; the returned disposer cancels it.
    /// The timer is owned by the calling fiber.
    pub fn timeout(
        &self,
        caller: &Context,
        callback: Arc<dyn Fn() + Send + Sync>,
        delay_ms: u64,
    ) -> Disposer {
        caller.effect(
            "ctx.timeout()",
            Box::pin(async move {
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    callback();
                });
                let abort = handle.abort_handle();
                Some(make_disposer(move || {
                    let abort = abort.clone();
                    Box::pin(async move {
                        abort.abort();
                    })
                }))
            }),
        )
    }

    /// Resolve after `delay_ms`, or reject with [`TimerError::Disposed`] when
    /// the calling fiber unloads first (TS `timeout(delay)` promise).
    ///
    /// Uses a oneshot channel: tokio `Notify` futures may complete spuriously
    /// and cannot distinguish real disposal from a spurious wake.
    pub fn timeout_future(
        &self,
        caller: &Context,
        delay_ms: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TimerError>> + Send>> {
        let (tx, rx) = tokio::sync::oneshot::channel::<TimerError>();
        let tx_cell: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<TimerError>>>> =
            Arc::new(std::sync::Mutex::new(Some(tx)));
        let tx_for_dispose = tx_cell.clone();
        let _ = caller.effect(
            "ctx.timeout()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let tx = tx_for_dispose.clone();
                    Box::pin(async move {
                        if let Some(tx) = tx.lock().unwrap().take() {
                            let _ = tx.send(TimerError::Disposed);
                        }
                    })
                }))
            }),
        );
        Box::pin(async move {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => Ok(()),
                result = rx => match result {
                    Ok(error) => Err(error),
                    Err(_closed) => Err(TimerError::Disposed),
                },
            }
        })
    }

    /// Run a callback repeatedly every `delay_ms`; the returned disposer
    /// cancels it. The timer is owned by the calling fiber.
    pub fn interval(
        &self,
        caller: &Context,
        callback: Arc<dyn Fn() + Send + Sync>,
        delay_ms: u64,
    ) -> Disposer {
        caller.effect(
            "ctx.interval()",
            Box::pin(async move {
                let handle = tokio::spawn(async move {
                    let mut tick = tokio::time::interval(Duration::from_millis(delay_ms));
                    loop {
                        tick.tick().await;
                        callback();
                    }
                });
                let abort = handle.abort_handle();
                Some(make_disposer(move || {
                    let abort = abort.clone();
                    Box::pin(async move {
                        abort.abort();
                    })
                }))
            }),
        )
    }

    /// Yield ticks every `delay_ms`; disposal closes the channel
    /// (TS `interval(delay)` async iterator).
    pub fn interval_stream(
        &self,
        caller: &Context,
        delay_ms: u64,
    ) -> tokio::sync::mpsc::UnboundedReceiver<()> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = caller.effect(
            "ctx.interval()",
            Box::pin(async move {
                let handle = tokio::spawn(async move {
                    let mut tick = tokio::time::interval(Duration::from_millis(delay_ms));
                    loop {
                        tick.tick().await;
                        if tx.send(()).is_err() {
                            break;
                        }
                    }
                });
                let abort = handle.abort_handle();
                Some(make_disposer(move || {
                    let abort = abort.clone();
                    Box::pin(async move {
                        abort.abort();
                    })
                }))
            }),
        );
        rx
    }

    /// Shared scheduling wrapper (TS `_schedule`): the produced callable
    /// clears any pending timer and re-schedules via `trigger`; the disposer
    /// marks the wrapper disposed and clears the pending timer.
    fn schedule(
        &self,
        caller: &Context,
        label: &str,
        trigger: TimerTrigger,
        initially_disposed: bool,
    ) -> Throttled {
        let disposed = Arc::new(AtomicBool::new(initially_disposed));
        let timer: Arc<std::sync::Mutex<Option<JoinHandle<()>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let timer_for_dispose = timer.clone();
        let disposed_for_dispose = disposed.clone();
        let label = label.to_string();
        let caller_for_dispose = caller.clone();
        let dispose = caller_for_dispose.effect(
            &label,
            Box::pin(async move {
                Some(make_disposer(move || {
                    let (timer, disposed) =
                        (timer_for_dispose.clone(), disposed_for_dispose.clone());
                    Box::pin(async move {
                        disposed.store(true, Ordering::SeqCst);
                        if let Some(handle) = timer.lock().unwrap().take() {
                            handle.abort();
                        }
                    })
                }))
            }),
        );
        let call: Arc<dyn Fn(Vec<ArcValue>) + Send + Sync> =
            Arc::new(move |args: Vec<ArcValue>| {
                // TS clears the pending timer before rescheduling.
                if let Some(old) = timer.lock().unwrap().take() {
                    old.abort();
                }
                *timer.lock().unwrap() = trigger(args, &disposed);
            });
        Throttled { call, dispose }
    }

    /// Return a throttled function whose timer is disposed with the current
    /// fiber (TS `throttle`; `no_trailing` suppresses the trailing call).
    pub fn throttle(
        &self,
        caller: &Context,
        callback: Arc<dyn Fn(Vec<ArcValue>) + Send + Sync>,
        delay_ms: u64,
        no_trailing: bool,
    ) -> Throttled {
        let last_call = Arc::new(std::sync::atomic::AtomicI64::new(i64::MIN / 4));
        let last_call_for_execute = last_call.clone();
        let execute: Arc<dyn Fn(Vec<ArcValue>) + Send + Sync> = Arc::new(move |args| {
            last_call_for_execute.store(now_ms(), Ordering::SeqCst);
            callback(args);
        });
        let execute_for_trigger = execute.clone();
        let last_call_for_trigger = last_call.clone();
        let trigger: TimerTrigger = Arc::new(move |args, is_disposed| {
            let now = now_ms();
            let remaining = delay_ms as i64 - now + last_call_for_trigger.load(Ordering::SeqCst);
            if remaining <= 0 {
                execute_for_trigger(args);
                None
            } else if !is_disposed.load(Ordering::SeqCst) {
                let execute = execute_for_trigger.clone();
                Some(tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(remaining.max(0) as u64)).await;
                    execute(args);
                }))
            } else {
                None
            }
        });
        self.schedule(caller, "ctx.throttle()", trigger, no_trailing)
    }

    /// Return a debounced function whose timer is disposed with the current
    /// fiber (TS `debounce`).
    pub fn debounce(
        &self,
        caller: &Context,
        callback: Arc<dyn Fn(Vec<ArcValue>) + Send + Sync>,
        delay_ms: u64,
    ) -> Throttled {
        let callback_for_trigger = callback.clone();
        let trigger: TimerTrigger = Arc::new(move |args, is_disposed| {
            if is_disposed.load(Ordering::SeqCst) {
                return None;
            }
            let callback = callback_for_trigger.clone();
            Some(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                callback(args);
            }))
        });
        self.schedule(caller, "ctx.debounce()", trigger, false)
    }
}

impl Service for TimerService {
    fn service_name(&self) -> &'static str {
        "timer"
    }
}

/// Callable timer wrapper with its disposer (TS `WithDispose<F>`).
pub struct Throttled {
    /// Invoke the wrapped (throttled/debounced) callback.
    pub call: Arc<dyn Fn(Vec<ArcValue>) + Send + Sync>,
    /// Dispose the pending timer (idempotent).
    pub dispose: Disposer,
}

/// The timer plugin entrypoint (`export default TimerService` in TS).
pub fn plugin() -> Arc<dyn Plugin> {
    Arc::new(TimerPlugin)
}

struct TimerPlugin;

#[async_trait::async_trait]
impl Plugin for TimerPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("timer")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        ctx.register_service(TimerService::new());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    type CapturedTimerFuture =
        Arc<std::sync::Mutex<Option<cordis::BoxFuture<'static, Result<(), TimerError>>>>>;

    async fn timer_ctx() -> Context {
        let ctx = Context::root();
        let fiber = ctx.plugin(plugin(), cordis::arc(()));
        fiber.settle().await.expect("timer plugin loads");
        ctx
    }

    fn service(ctx: &Context) -> Arc<TimerService> {
        ctx.get_typed::<Arc<TimerService>>("timer", true)
            .expect("timer service")
            .as_ref()
            .clone()
    }

    #[tokio::test]
    async fn timeout_fires_once() {
        let ctx = timer_ctx().await;
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        service(&ctx).timeout(
            &ctx,
            Arc::new(move || {
                h.fetch_add(1, Ordering::SeqCst);
            }),
            30,
        );
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn timeout_dispose_cancels() {
        let ctx = timer_ctx().await;
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let dispose = service(&ctx).timeout(
            &ctx,
            Arc::new(move || {
                h.fetch_add(1, Ordering::SeqCst);
            }),
            30,
        );
        dispose().await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn timeout_future_resolves_and_rejects_on_dispose() {
        let ctx = timer_ctx().await;
        let ok = service(&ctx).timeout_future(&ctx, 10).await;
        assert_eq!(ok, Ok(()));

        // A fiber-owned future rejects when the owning plugin unloads.
        struct Capturer(CapturedTimerFuture);
        #[async_trait::async_trait]
        impl Plugin for Capturer {
            async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
                let timer = ctx
                    .get_typed::<Arc<TimerService>>("timer", true)
                    .expect("timer");
                let future = timer.timeout_future(ctx, 10_000);
                *self.0.lock().unwrap() = Some(future);
                Ok(())
            }
        }
        let slot = Arc::new(std::sync::Mutex::new(None));
        let capturer: Arc<dyn Plugin> = Arc::new(Capturer(slot.clone()));
        let fiber = ctx.plugin(capturer, cordis::arc(()));
        fiber.settle().await.unwrap();
        let future = slot.lock().unwrap().take().expect("captured future");
        fiber.dispose().await;
        assert_eq!(future.await, Err(TimerError::Disposed));
    }

    #[tokio::test]
    async fn interval_ticks_and_disposes() {
        let ctx = timer_ctx().await;
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let dispose = service(&ctx).interval(
            &ctx,
            Arc::new(move || {
                h.fetch_add(1, Ordering::SeqCst);
            }),
            20,
        );
        tokio::time::sleep(Duration::from_millis(90)).await;
        let count = hits.load(Ordering::SeqCst);
        assert!(count >= 3, "expected >=3 ticks, got {count}");
        dispose().await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            hits.load(Ordering::SeqCst),
            count,
            "ticks continue after dispose"
        );
    }

    #[tokio::test]
    async fn interval_stream_yields_ticks() {
        let ctx = timer_ctx().await;
        let mut rx = service(&ctx).interval_stream(&ctx, 20);
        let first = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(first.is_ok() && first.unwrap().is_some());
        let second = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(second.is_ok() && second.unwrap().is_some());
    }

    #[tokio::test]
    async fn throttle_leading_and_trailing() {
        let ctx = timer_ctx().await;
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let throttled = service(&ctx).throttle(
            &ctx,
            Arc::new(move |_args: Vec<ArcValue>| {
                h.fetch_add(1, Ordering::SeqCst);
            }),
            40,
            false,
        );
        (throttled.call)(vec![]);
        (throttled.call)(vec![]);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "leading call fires immediately"
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 2, "trailing call fires once");
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn throttle_no_trailing_suppresses() {
        let ctx = timer_ctx().await;
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let throttled = service(&ctx).throttle(
            &ctx,
            Arc::new(move |_args: Vec<ArcValue>| {
                h.fetch_add(1, Ordering::SeqCst);
            }),
            40,
            true,
        );
        (throttled.call)(vec![]);
        (throttled.call)(vec![]);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "no trailing call with noTrailing"
        );
    }

    #[tokio::test]
    async fn debounce_coalesces() {
        let ctx = timer_ctx().await;
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let debounced = service(&ctx).debounce(
            &ctx,
            Arc::new(move |_args: Vec<ArcValue>| {
                h.fetch_add(1, Ordering::SeqCst);
            }),
            40,
        );
        for _ in 0..3 {
            (debounced.call)(vec![]);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn debounce_dispose_cancels() {
        let ctx = timer_ctx().await;
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let debounced = service(&ctx).debounce(
            &ctx,
            Arc::new(move |_args: Vec<ArcValue>| {
                h.fetch_add(1, Ordering::SeqCst);
            }),
            40,
        );
        (debounced.call)(vec![]);
        (debounced.dispose)().await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }
}
