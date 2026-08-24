//! Bounded per-session write batching for the shared persistence coordinator.
//! Rust port of `packages/session/session-persistence/src/write-behind.ts`.
//!
//! The TS `setTimeout` deadline becomes a tokio timer task feeding one
//! controller pump (timers cannot be cancelled individually, so a fired
//! deadline no-ops when the queue is already drained — the TS
//! `cancelTimer` equivalent).

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use dsh_session::SessionEvent;
use futures::FutureExt;
use parking_lot::Mutex;

/// The durable write future: resolves only after backend durability.
pub type WriteFuture = Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;

/// The flush/barrier future.
pub type FlushFuture = Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;

/// Await one shared barrier receiver (async fn form so the receiver is a
/// plain owned parameter, matching the drain-barrier await).
async fn await_barrier(
    receiver: tokio::sync::oneshot::Receiver<Result<(), String>>,
) -> Result<(), String> {
    match receiver.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error),
        Err(_) => Err("write-behind barrier was dropped".to_string()),
    }
}

/// Dependencies and scheduling policy for one live session's write
/// controller.
pub struct SessionWriteBehindOptions {
    /// Maximum intentional batching wait after an idle queue receives work.
    pub max_delay_ms: u64,
    /// Persist one stable ordered prefix; resolves only after backend
    /// durability.
    pub write: Arc<dyn Fn(Vec<SessionEvent>) -> WriteFuture + Send + Sync>,
    /// Observe a detached background write failure without rejecting the
    /// producer.
    pub report_background_failure: Arc<dyn Fn(&str) + Send + Sync>,
}

#[derive(Default)]
struct State {
    pending: Vec<SessionEvent>,
    active: Option<tokio::sync::oneshot::Receiver<Result<(), String>>>,
    barrier: Option<futures::future::Shared<FlushFuture>>,
    deadline_expired: bool,
    automatic_paused: bool,
}

/// Owns one live session's pending events, fixed batching deadline, active
/// write, failure retention, and explicit quiescence barrier
/// (TS `SessionWriteBehind`).
pub struct SessionWriteBehind {
    options: SessionWriteBehindOptions,
    state: Mutex<State>,
    deadline_tx: tokio::sync::mpsc::UnboundedSender<()>,
}

impl SessionWriteBehind {
    pub fn new(options: SessionWriteBehindOptions) -> Arc<Self> {
        let (deadline_tx, deadline_rx) = tokio::sync::mpsc::unbounded_channel();
        let controller = Arc::new(Self {
            options,
            state: Mutex::new(State::default()),
            deadline_tx,
        });
        {
            let controller = Arc::clone(&controller);
            tokio::spawn(async move {
                let mut receiver = deadline_rx;
                loop {
                    match receiver.recv().await {
                        Some(()) => controller.on_deadline(),
                        None => return,
                    }
                }
            });
        }
        controller
    }

    /// Whether this controller owns queued events or an active durable write.
    pub fn has_work(&self) -> bool {
        let state = self.state.lock();
        !state.pending.is_empty() || state.active.is_some()
    }

    /// Copy one event into the persistence-owned queue and start a fixed
    /// deadline when the automatic path is idle.
    pub fn enqueue(self: &Arc<Self>, event: SessionEvent) {
        let (was_empty, has_barrier, automatic_paused) = {
            let mut state = self.state.lock();
            let was_empty = state.pending.is_empty();
            state.pending.push(event);
            (was_empty, state.barrier.is_some(), state.automatic_paused)
        };
        if has_barrier {
            return;
        }
        if automatic_paused {
            {
                let mut state = self.state.lock();
                state.automatic_paused = false;
                state.deadline_expired = false;
            }
            self.arm_timer();
        } else if was_empty {
            self.arm_timer();
        }
    }

    /// Cancel the batching wait and durably drain through a quiescent point.
    /// Concurrent callers join the same barrier.
    pub fn flush(self: &Arc<Self>) -> FlushFuture {
        let barrier = {
            let mut state = self.state.lock();
            match &state.barrier {
                Some(barrier) => barrier.clone(),
                None => {
                    state.deadline_expired = false;
                    state.automatic_paused = false;
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let controller = Arc::clone(self);
                    let barrier_future: FlushFuture = Box::pin(await_barrier(rx));
                    let shared = barrier_future.shared();
                    state.barrier = Some(shared.clone());
                    tokio::spawn(async move {
                        controller.drain_barrier(tx).await;
                    });
                    shared
                }
            }
        };
        Box::pin(barrier)
    }

    /// Cancel the current automatic deadline without draining retained work.
    /// (Timers cannot be cancelled; the pump's deadline no-ops once the queue
    /// is drained — the observable TS behavior.)
    pub fn cancel_automatic_wait(&self) {
        let mut state = self.state.lock();
        state.deadline_expired = false;
        state.automatic_paused = false;
    }

    /// Start the one fixed window for the current pending prefix.
    fn arm_timer(self: &Arc<Self>) {
        let controller = Arc::clone(self);
        let delay = self.options.max_delay_ms;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let _ = controller.deadline_tx.send(());
        });
    }

    /// Start a background write now, or remember that an active write used
    /// the budget (TS `onDeadline`).
    fn on_deadline(self: &Arc<Self>) {
        let (has_active, pending_empty) = {
            let state = self.state.lock();
            (state.active.is_some(), state.pending.is_empty())
        };
        if has_active {
            self.state.lock().deadline_expired = true;
            return;
        }
        if pending_empty {
            return;
        }
        self.start_background();
    }

    /// Start one detached write whose failure is reported and retained.
    fn start_background(self: &Arc<Self>) {
        let active = self.start_write(true);
        let controller = Arc::clone(self);
        tokio::spawn(async move {
            let _ = active.await;
            controller.continue_automatic();
        });
    }

    /// Continue immediately after an over-budget active write, otherwise keep
    /// its timer.
    fn continue_automatic(self: &Arc<Self>) {
        let (has_barrier, pending_empty, expired) = {
            let state = self.state.lock();
            (
                state.barrier.is_some(),
                state.pending.is_empty(),
                state.deadline_expired,
            )
        };
        if has_barrier || pending_empty {
            return;
        }
        if expired {
            self.state.lock().deadline_expired = false;
            self.start_background();
        }
    }

    /// Await overlapping work, drain to quiescence, and settle the shared
    /// barrier (TS `drainBarrier`).
    async fn drain_barrier(self: &Arc<Self>, tx: tokio::sync::oneshot::Sender<Result<(), String>>) {
        // Await the overlapping active write, if any.
        loop {
            let overlapping = self.state.lock().active.take();
            match overlapping {
                Some(receiver) => match receiver.await {
                    Ok(Ok(())) => {
                        self.state.lock().automatic_paused = false;
                    }
                    Ok(Err(error)) => {
                        // Preserve the durable backend failure as the
                        // authority for every waiter on this barrier.
                        self.state.lock().barrier = None;
                        let _ = tx.send(Err(error));
                        return;
                    }
                    Err(_) => {
                        // The writer disappeared without publishing an
                        // outcome; this is the only genuinely generic case.
                        self.state.lock().barrier = None;
                        let _ = tx.send(Err("overlapping active write failed".to_string()));
                        return;
                    }
                },
                None => break,
            }
        }
        while !self.state.lock().pending.is_empty() {
            match self.start_write(false).await {
                Ok(()) => {}
                Err(error) => {
                    self.state.lock().barrier = None;
                    let _ = tx.send(Err(error));
                    return;
                }
            }
        }
        // Close admission to this barrier in the same job that observes the
        // empty queue, before resolving callers.
        self.state.lock().barrier = None;
        let _ = tx.send(Ok(()));
    }

    /// Start one stable pending prefix, retaining it in order if durability
    /// fails (TS `startWrite`).
    fn start_write(self: &Arc<Self>, background: bool) -> WriteFuture {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let batch = {
            let mut state = self.state.lock();
            let batch = std::mem::take(&mut state.pending);
            state.deadline_expired = false;
            state.active = Some(rx);
            batch
        };
        let write = self.options.write.clone();
        let controller = Arc::clone(self);
        let report = self.options.report_background_failure.clone();
        Box::pin(async move {
            let result = write(batch.clone()).await;
            match &result {
                Ok(()) => {}
                Err(error) => {
                    // Retain the batch in order and pause the automatic path.
                    {
                        let mut state = controller.state.lock();
                        let mut retained = batch;
                        retained.extend(std::mem::take(&mut state.pending));
                        state.pending = retained;
                        state.deadline_expired = false;
                        state.automatic_paused = true;
                    }
                    if background {
                        report(error);
                    }
                }
            }
            controller.state.lock().active = None;
            let _ = tx.send(result.clone());
            result
        })
    }
}
