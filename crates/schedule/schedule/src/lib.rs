//! Agent-scoped durable one-shot and fixed-rate reminders over the session
//! event log. Rust port of `packages/schedule/schedule/src/index.ts`.
//!
//! # Deviations
//!
//! - `ctx.agents.withoutInitiator` has no Rust counterpart; the runtime
//!   drive loop spawns directly.
//! - The Rust `Agent::run_maintenance` erases its boolean result; the
//!   runtime reads the task outcome from a shared slot after the
//!   maintenance future resolves.

pub mod domain;
pub mod invariant;
pub mod persistence;
pub mod projection;
pub mod runtime;
pub mod tools;
pub mod transaction;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
};
use dsh_agent::{AgentLifecyclePayload, AgentRegistry, AgentStatus, AgentStatusPayload};
use dsh_session_projection::SessionProjectionRegistry;

pub use crate::domain::{
    MIN_EVERY_INTERVAL_SECONDS, SCHEDULE_CHANGE_VERSION, ScheduleInputError, ScheduleLogError,
    allocate_schedule_id, apply_change, canonicalize_time_zone, create_after_schedule_record,
    create_at_schedule_record, create_every_schedule_record, decode_schedule_change,
    fold_schedule_events, render_every_reminder_batch_framing, render_reminder_framing,
    resolve_every_occurrence, schedule_view,
};
pub use crate::persistence::{SchedulePersistenceError, flush_schedule_persistence};
pub use crate::projection::schedule_projection_definition;
pub use crate::runtime::ScheduleRuntime;
pub use crate::tools::register_schedule_tools;
pub use crate::transaction::run_schedule_transaction;
pub use crate::types::*;

/// Cordis function-plugin name.
pub const NAME: &str = "schedule";

/// Services required before future root agents can receive Schedule.
pub const INJECT: [&str; 4] = ["agents", "sessions", "tools", "sessionPersistence"];

/// Install Schedule only for root agents published after this plugin loads
/// (TS `apply`).
pub fn apply(ctx: &Context) {
    if let Some(registry) = ctx
        .get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
        .map(|slot| slot.as_ref().clone())
    {
        registry
            .register(ctx, schedule_projection_definition())
            .expect("schedule projection registration");
    }

    let runtimes: Arc<parking_lot::Mutex<HashMap<usize, cordis::Disposer>>> =
        Arc::new(parking_lot::Mutex::new(HashMap::new()));
    let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let created_listener: Arc<Listener> = Arc::new({
        let ctx = ctx.clone();
        let runtimes = runtimes.clone();
        let stopping = stopping.clone();
        move |_listener_ctx, args| {
            let ctx = ctx.clone();
            let runtimes = runtimes.clone();
            let stopping = stopping.clone();
            Box::pin(async move {
                let Some(payload) = args
                    .first()
                    .and_then(|value| downcast::<AgentLifecyclePayload>(value))
                    .cloned()
                else {
                    return None;
                };
                let agent = payload.agent;
                let registry = ctx
                    .get_typed::<Arc<AgentRegistry>>("agents", false)
                    .map(|slot| slot.as_ref().clone());
                let is_root = registry.as_ref().is_some_and(|registry| {
                    registry
                        .roots()
                        .iter()
                        .any(|root| Arc::ptr_eq(root, &agent))
                });
                let key = Arc::as_ptr(&agent).cast::<()>() as usize;
                if stopping.load(std::sync::atomic::Ordering::SeqCst)
                    || runtimes.lock().contains_key(&key)
                    || !is_root
                {
                    return None;
                }
                // Agent-scoped ownership: register tools + status listener,
                // start the runtime, and dispose everything together.
                let runtime = ScheduleRuntime::new(&ctx, agent.clone());
                let owner: cordis::Disposer = agent.ctx().effect(
                    "schedule.runtime()",
                    Box::pin({
                        let ctx = ctx.clone();
                        let agent = agent.clone();
                        let runtime = runtime.clone();
                        let runtimes = runtimes.clone();
                        async move {
                            let disposers = register_schedule_tools(
                                &ctx,
                                agent.ctx(),
                                agent.clone(),
                                Arc::new({
                                    let runtime = runtime.clone();
                                    move || runtime.request_drive()
                                }),
                            );
                            let stop_status: cordis::Disposer = agent
                                .ctx()
                                .on(
                                    "agent/status",
                                    Arc::new({
                                        let agent = agent.clone();
                                        let runtime = runtime.clone();
                                        move |_ctx, args| {
                                            let agent = agent.clone();
                                            let runtime = runtime.clone();
                                            Box::pin(async move {
                                                let Some(payload) = args
                                                    .first()
                                                    .and_then(|value| {
                                                        downcast::<AgentStatusPayload>(value)
                                                    })
                                                    .cloned()
                                                else {
                                                    return None;
                                                };
                                                if !Arc::ptr_eq(&payload.agent, &agent) {
                                                    return None;
                                                }
                                                if payload.status == AgentStatus::Idle
                                                    && agent.session().events().iter().any(
                                                        |event| event.type_ == "schedule/change",
                                                    )
                                                {
                                                    runtime.request_drive();
                                                }
                                                None
                                            })
                                        }
                                    }),
                                    EventOptions::default(),
                                )
                                .await;
                            runtime.start();
                            Some(cordis::events::make_disposer({
                                let runtime = runtime.clone();
                                let runtimes = runtimes.clone();
                                let key = Arc::as_ptr(&agent).cast::<()>() as usize;
                                move || {
                                    let stop_status = stop_status.clone();
                                    let disposers = disposers.clone();
                                    let runtime = runtime.clone();
                                    let runtimes = runtimes.clone();
                                    Box::pin(async move {
                                        stop_status().await;
                                        disposers().await;
                                        runtime.dispose().await;
                                        runtimes.lock().remove(&key);
                                    })
                                }
                            }))
                        }
                    }),
                );
                runtimes.lock().insert(key, owner);
                None
            })
        }
    });

    let stop_created = futures::executor::block_on(ctx.on(
        "agent/created",
        created_listener,
        EventOptions::default(),
    ));

    let disposer: cordis::Disposer = cordis::events::make_disposer(move || {
        let stop_created = stop_created.clone();
        let runtimes = runtimes.clone();
        let stopping = stopping.clone();
        Box::pin(async move {
            stopping.store(true, std::sync::atomic::Ordering::SeqCst);
            stop_created().await;
            let cleanups: Vec<cordis::Disposer> =
                runtimes.lock().drain().map(|(_, value)| value).collect();
            for cleanup in cleanups {
                cleanup().await;
            }
        })
    });
    ctx.effect(
        "schedule.lifecycle()",
        Box::pin(async move { Some(disposer) }),
    );
}

/// The Cordis plugin form of the schedule service.
pub struct SchedulePlugin;

#[async_trait::async_trait]
impl Plugin for SchedulePlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
