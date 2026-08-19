//! Disposable live timer projection for one exact root agent. Rust port of
//! `packages/schedule/schedule/src/runtime.ts`.
//!
//! # Deviations
//!
//! - `ctx.agents.withoutInitiator` has no Rust counterpart; the drive loop
//!   spawns directly.
//! - The Rust `Agent::run_maintenance` erases its result; the task closure
//!   writes its boolean outcome into a shared slot the runtime reads after
//!   the maintenance future resolves.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cordis::{Context, arc};
use dsh_agent::{Agent, AgentRegistry};
use dsh_llm::{ContentBlock, MessageSource, create_user_message};

use crate::domain::{
    FoldedSchedules, fold_schedule_events, render_every_reminder_batch_framing,
    render_reminder_framing, resolve_every_occurrence,
};
use crate::persistence::flush_schedule_persistence;
use crate::transaction::run_schedule_transaction;
use crate::types::{ScheduleChange, ScheduleRecord};

/// Largest delay that Node timers represent without clamping.
pub const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;

struct EveryDue {
    record: ScheduleRecord,
    occurrence_at: String,
}

enum DueDecision {
    OneShot {
        record: ScheduleRecord,
    },
    Every {
        reminders: Vec<EveryDue>,
        accepted_at: String,
    },
    Wait {
        target: Option<i64>,
    },
}

/// Select one due one-shot, one complete fixed-rate batch, or the next wake.
fn due_decision(folded: &FoldedSchedules, now: i64) -> DueDecision {
    let parse = |record: &ScheduleRecord| {
        crate::domain::parse_canonical_instant(record.scheduled_at()).unwrap_or(i64::MIN)
    };
    let mut one_shot: Option<ScheduleRecord> = None;
    let mut one_shot_rank: Option<(i64, usize)> = None;
    let mut every: Vec<(i64, usize, ScheduleRecord)> = Vec::new();
    for (index, record) in folded.active.iter().enumerate() {
        let target = parse(record);
        match record {
            ScheduleRecord::Every { .. } => {
                if target <= now {
                    every.push((target, index, record.clone()));
                }
            }
            _ => {
                if target <= now {
                    let rank = (target, index);
                    if one_shot_rank.is_none_or(|existing| rank < existing) {
                        one_shot_rank = Some(rank);
                        one_shot = Some(record.clone());
                    }
                }
            }
        }
    }
    if let Some(record) = one_shot {
        return DueDecision::OneShot { record };
    }
    if !every.is_empty() {
        every.sort_by_key(|(target, index, _)| (*target, *index));
        let accepted_at = crate::domain::format_canonical_instant(now)
            .expect("decision now is a representable instant");
        let reminders = every
            .into_iter()
            .map(|(_, _, record)| {
                let occurrence_at = resolve_every_occurrence(&record, now)
                    .map(|occurrence| occurrence.occurrence_at)
                    .unwrap_or_default();
                EveryDue {
                    record,
                    occurrence_at,
                }
            })
            .collect();
        return DueDecision::Every {
            reminders,
            accepted_at,
        };
    }
    let target = folded
        .active
        .iter()
        .map(parse)
        .filter(|candidate| *candidate > now)
        .min();
    DueDecision::Wait { target }
}

fn render_thrown(message: &str) -> String {
    message.to_string()
}

fn install_if_vacant<T>(slot: &parking_lot::Mutex<Option<T>>, create: impl FnOnce() -> T) -> bool {
    let mut slot = slot.lock();
    if slot.is_some() {
        return false;
    }
    *slot = Some(create());
    true
}

/// One process-local, disposable projection of an exact agent's durable
/// schedules.
pub struct ScheduleRuntime {
    ctx: Context,
    agent: Arc<dyn Agent>,
    requested: AtomicBool,
    stopping: AtomicBool,
    faulted: AtomicBool,
    stop: Arc<tokio::sync::Notify>,
    timer: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    run: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    arc: std::sync::OnceLock<std::sync::Weak<Self>>,
}

impl ScheduleRuntime {
    /// Construct an inactive runtime; [`ScheduleRuntime::start`] begins the
    /// first preflight.
    pub fn new(ctx: &Context, agent: Arc<dyn Agent>) -> Arc<Self> {
        let runtime = Arc::new(Self {
            ctx: ctx.clone(),
            agent,
            requested: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            faulted: AtomicBool::new(false),
            stop: Arc::new(tokio::sync::Notify::new()),
            timer: parking_lot::Mutex::new(None),
            run: parking_lot::Mutex::new(None),
            arc: std::sync::OnceLock::new(),
        });
        runtime.arc.set(Arc::downgrade(&runtime)).expect("once");
        runtime
    }

    fn self_arc(&self) -> Arc<Self> {
        self.arc
            .get()
            .and_then(std::sync::Weak::upgrade)
            .expect("the runtime must be held by an Arc")
    }

    /// Begin the initial durability preflight and timer derivation.
    pub fn start(&self) {
        self.request_drive();
    }

    /// Recompute the live projection after a committed mutation or idle
    /// transition.
    pub fn request_drive(&self) {
        if self.stopping.load(Ordering::SeqCst) || self.faulted.load(Ordering::SeqCst) {
            return;
        }
        self.clear_timer();
        self.requested.store(true, Ordering::SeqCst);
        let runtime = self.self_arc();
        install_if_vacant(&self.run, || {
            tokio::spawn(async move { runtime.run_requested().await })
        });
    }

    /// Stop future work, cancel timers, and await every outstanding runtime
    /// task.
    pub async fn dispose(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.requested.store(false, Ordering::SeqCst);
        self.clear_timer();
        self.stop.notify_waiters();
        let run = self.run.lock().take();
        if let Some(run) = run {
            let _ = run.await;
        }
    }

    /// Drain coalesced triggers serially.
    async fn run_requested(self: Arc<Self>) {
        while self.requested.load(Ordering::SeqCst)
            && !self.stopping.load(Ordering::SeqCst)
            && !self.faulted.load(Ordering::SeqCst)
        {
            self.requested.store(false, Ordering::SeqCst);
            run_schedule_transaction(self.agent.as_ref(), || {
                let runtime = self.self_arc();
                async move { runtime.drive_once().await }
            })
            .await;
        }
        // Retire this exact run and honor a trigger that landed during its
        // final microtask.
        self.run.lock().take();
        if self.requested.load(Ordering::SeqCst)
            && !self.stopping.load(Ordering::SeqCst)
            && !self.faulted.load(Ordering::SeqCst)
        {
            self.request_drive();
        }
    }

    /// Whether this exact root lifecycle remains authoritative.
    fn is_live(&self) -> bool {
        let registry = self
            .ctx
            .get_typed::<Arc<AgentRegistry>>("agents", false)
            .map(|slot| slot.as_ref().clone());
        match registry {
            Some(registry) => {
                let same = registry
                    .get(self.agent.id())
                    .is_some_and(|current| Arc::ptr_eq(&current, &self.agent));
                let root = registry
                    .roots()
                    .iter()
                    .any(|root| Arc::ptr_eq(root, &self.agent));
                same && root
            }
            None => false,
        }
    }

    /// Whether this runtime may start or continue Schedule work.
    fn is_runnable(&self) -> bool {
        !self.stopping.load(Ordering::SeqCst) && self.is_live()
    }

    /// Cancel the currently armed timer, if any.
    fn clear_timer(&self) {
        if let Some(timer) = self.timer.lock().take() {
            timer.abort();
        }
    }

    /// Arm one bounded timer segment; every wake rechecks the wall clock.
    fn arm(&self, target: i64, now: i64) {
        let delay_ms = (target - now).min(MAX_TIMER_DELAY_MS as i64).max(0) as u64;
        let handle = tokio::spawn({
            let runtime = self.self_arc();
            async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                {
                    let mut timer = runtime.timer.lock();
                    timer.take();
                }
                runtime.request_drive();
            }
        });
        *self.timer.lock() = Some(handle);
    }

    /// Await one public idle boundary without holding admission or creating
    /// a retry timer.
    fn wait_for_idle(&self) {
        let runtime = self.self_arc();
        let agent = self.agent.clone();
        let stop = self.stop.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = agent.when_idle() => {
                    runtime.request_drive();
                }
                _ = stop.notified() => {}
            }
        });
    }

    /// Fold the current exact runtime suffix and contain a corrupt durable
    /// stream.
    fn read_folded(&self) -> Option<FoldedSchedules> {
        match fold_schedule_events(
            &self.agent.session().events(),
            self.agent.session().header().seed_length.unwrap_or(0) as usize,
        ) {
            Ok(folded) => Some(folded),
            Err(error) => {
                self.faulted.store(true, Ordering::SeqCst);
                self.ctx.logger.warn(
                    &self.ctx,
                    vec![arc(format!(
                        "schedule: corrupt schedule log for agent \"{}\": {}",
                        self.agent.id().as_str(),
                        error.message
                    ))],
                );
                None
            }
        }
    }

    /// Contain an invalid wall-clock decision without permanently faulting
    /// this runtime.
    fn decide(&self, folded: &FoldedSchedules, now: i64) -> Option<DueDecision> {
        Some(due_decision(folded, now))
    }

    /// Preflight, fold, arm, or dispatch the next one-shot or fixed-rate
    /// batch.
    async fn drive_once(&self) {
        self.clear_timer();
        if !self.is_runnable() {
            return;
        }
        if let Err(error) = flush_schedule_persistence(&self.ctx, self.agent.session()).await {
            if self.is_live() {
                self.ctx.logger.warn(
                    &self.ctx,
                    vec![arc(format!(
                        "schedule: preflight failed for agent \"{}\": {}",
                        self.agent.id().as_str(),
                        render_thrown(&error.message)
                    ))],
                );
            }
            return;
        }
        if !self.is_runnable() {
            return;
        }
        let Some(folded) = self.read_folded() else {
            return;
        };
        let wake_now = chrono::Utc::now().timestamp_millis();
        let Some(wake_decision) = self.decide(&folded, wake_now) else {
            return;
        };
        if let DueDecision::Wait {
            target: Some(target),
        } = wake_decision
        {
            self.arm(target, wake_now);
            return;
        }
        if matches!(wake_decision, DueDecision::Wait { target: None }) {
            return;
        }

        // Run the dispatch inside one non-turn maintenance task.
        let outcome = Arc::new(parking_lot::Mutex::new(None));
        let task_outcome = outcome.clone();
        let runtime = self.self_arc();
        let task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync> =
            Arc::new(move || {
                let runtime = runtime.clone();
                let task_outcome = task_outcome.clone();
                Box::pin(async move {
                    let outcome = runtime.maintenance_decision().await;
                    *task_outcome.lock() = Some(outcome);
                })
            });
        self.agent.run_maintenance(task).await;
        let Some(dispatched) = *outcome.lock() else {
            // The maintenance task never ran (the Rust trait erases its
            // result); wait for the next idle boundary like the TS busy
            // rejection path.
            if self.is_live() {
                self.wait_for_idle();
            }
            return;
        };
        if !dispatched {
            return;
        }
        if let Err(error) = flush_schedule_persistence(&self.ctx, self.agent.session()).await {
            if self.is_live() {
                self.ctx.logger.warn(
                    &self.ctx,
                    vec![arc(format!(
                        "schedule: dispatch barrier failed for agent \"{}\": {}",
                        self.agent.id().as_str(),
                        render_thrown(&error.message)
                    ))],
                );
            }
            return;
        }
        if self.is_runnable() {
            self.request_drive();
        }
    }

    /// The maintenance-task body: decide against the current wall clock and
    /// dispatch one one-shot or one complete fixed-rate batch.
    async fn maintenance_decision(&self) -> bool {
        if !self.is_runnable() {
            return false;
        }
        let Some(claimed) = self.read_folded() else {
            return false;
        };
        let decision_now = chrono::Utc::now().timestamp_millis();
        let Some(decision) = self.decide(&claimed, decision_now) else {
            return false;
        };
        if let DueDecision::Wait {
            target: Some(target),
        } = decision
        {
            self.arm(target, decision_now);
            return false;
        }
        if matches!(decision, DueDecision::Wait { target: None }) {
            return false;
        }
        let text = match &decision {
            DueDecision::OneShot { record } => render_reminder_framing(record),
            DueDecision::Every { reminders, .. } => render_every_reminder_batch_framing(
                &reminders
                    .iter()
                    .map(|due| (due.record.clone(), due.occurrence_at.clone()))
                    .collect::<Vec<_>>(),
            ),
            DueDecision::Wait { .. } => unreachable!("handled above"),
        };
        let message = create_user_message(
            vec![ContentBlock::Text { text }],
            MessageSource::Plugin {
                plugin: "schedule".to_string(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        );
        self.agent.followup(message);
        let appended = match &decision {
            DueDecision::OneShot { record } => self
                .agent
                .session()
                .append(
                    "schedule/change",
                    serde_json::to_value(&ScheduleChange::Dispatch {
                        version: 1,
                        id: record.id().clone(),
                        accepted_at: None,
                    })
                    .expect("dispatch json"),
                    None,
                )
                .map(|_| ()),
            DueDecision::Every {
                reminders,
                accepted_at,
            } => {
                let mut result = Ok(());
                for due in reminders {
                    if result.is_ok() {
                        result = self
                            .agent
                            .session()
                            .append(
                                "schedule/change",
                                serde_json::to_value(&ScheduleChange::Dispatch {
                                    version: 1,
                                    id: due.record.id().clone(),
                                    accepted_at: Some(accepted_at.clone()),
                                })
                                .expect("dispatch json"),
                                None,
                            )
                            .map(|_| ());
                    }
                }
                result
            }
            DueDecision::Wait { .. } => unreachable!("handled above"),
        };
        if let Err(error) = appended {
            self.faulted.store(true, Ordering::SeqCst);
            self.clear_timer();
            self.ctx.logger.warn(
                &self.ctx,
                vec![arc(format!(
                    "schedule: dispatch append failed for agent \"{}\": {}",
                    self.agent.id().as_str(),
                    render_thrown(&error)
                ))],
            );
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vacant_run_slot_is_claimed_once_under_concurrency() {
        const CONTENDERS: usize = 16;
        let slot = Arc::new(parking_lot::Mutex::new(None));
        let start = Arc::new(std::sync::Barrier::new(CONTENDERS));
        let initializers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut threads = Vec::new();

        for value in 0..CONTENDERS {
            let slot = Arc::clone(&slot);
            let start = Arc::clone(&start);
            let initializers = Arc::clone(&initializers);
            threads.push(std::thread::spawn(move || {
                start.wait();
                install_if_vacant(&slot, || {
                    initializers.fetch_add(1, Ordering::SeqCst);
                    value
                });
            }));
        }

        for thread in threads {
            thread.join().expect("slot contender");
        }
        assert_eq!(initializers.load(Ordering::SeqCst), 1);
        assert!(slot.lock().is_some());
    }
}
