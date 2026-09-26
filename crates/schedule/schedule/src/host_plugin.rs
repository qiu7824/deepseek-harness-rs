//! Optional Cordis lifecycle for Host reminders; the store outlives this plugin.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use cordis::{
    ArcValue, Context, Disposer, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
};
use dsh_agent::{Agent, AgentLifecyclePayload, AgentRegistry};
use parking_lot::Mutex;

use crate::host_service::{Config, ScheduleService};
use crate::host_tools::register_host_schedule_tools;
use crate::host_types::ScheduleError;

#[derive(Default)]
struct Registrations {
    stopping: bool,
    agents: HashMap<usize, Disposer>,
    warned_sessions: HashSet<String>,
}

/// Decode Loader JSON without silently replacing invalid values with defaults.
pub fn decode_host_schedule_config(value: &serde_json::Value) -> Result<Config, ScheduleError> {
    if !value.is_object() {
        return Err(ScheduleError::invalid(
            "Schedule configuration must be a JSON object.",
        ));
    }
    let mut normalized = value.clone();
    if let Some(object) = normalized.as_object_mut() {
        for key in ["deliveryHistoryDays", "deliveryHistoryRecords"] {
            if let Some(value) = object.get_mut(key) {
                if let Some(number) = value.as_f64().filter(|number| {
                    number.is_finite() && number.fract() == 0.0 && (1.0..=10_000.0).contains(number)
                }) {
                    *value = serde_json::json!(number as u64);
                }
            }
        }
    }
    let config: Config = serde_json::from_value(normalized)
        .map_err(|_| ScheduleError::invalid("Schedule configuration accepts deliveryHistoryDays and deliveryHistoryRecords as positive integers."))?;
    crate::host_types::RetentionBounds {
        days: config.delivery_history_days,
        records: config.delivery_history_records,
    }
    .validate()?;
    Ok(config)
}

fn plugin_config(value: &ArcValue) -> Result<Config, ScheduleError> {
    if let Some(config) = value.downcast_ref::<Config>() {
        crate::host_types::RetentionBounds {
            days: config.delivery_history_days,
            records: config.delivery_history_records,
        }
        .validate()?;
        return Ok(config.clone());
    }
    if let Some(value) = value.downcast_ref::<serde_json::Value>() {
        return decode_host_schedule_config(value);
    }
    if value.downcast_ref::<()>().is_some() {
        return Ok(Config::default());
    }
    Err(ScheduleError::invalid(
        "Schedule configuration must be a JSON object.",
    ))
}

fn warn_legacy(
    ctx: &Context,
    registrations: &Arc<Mutex<Registrations>>,
    session: &dsh_session::Session,
) {
    {
        let mut state = registrations.lock();
        if state.stopping
            || !state
                .warned_sessions
                .insert(session.id().as_str().to_owned())
        {
            return;
        }
    }
    let warning = match crate::domain::fold_session_schedules(session) {
        Ok(fold) if fold.active.is_empty() => return,
        Ok(_) => format!(
            "schedule: Session \"{}\" contains legacy reminders; recreate active reminders with schedule_create.",
            session.id().as_str()
        ),
        Err(_) => format!(
            "schedule: Session \"{}\" historical reminders could not be read; legacy reminders are ignored.",
            session.id().as_str()
        ),
    };
    ctx.logger.warn(ctx, vec![cordis::arc(warning)]);
}

fn attach(
    ctx: &Context,
    registry: &Arc<AgentRegistry>,
    registrations: &Arc<Mutex<Registrations>>,
    service: &Arc<ScheduleService>,
    agent: Arc<dyn Agent>,
) -> Result<(), ScheduleError> {
    if agent.session().header().origin.as_deref() == Some("subagent")
        || !registry
            .roots()
            .iter()
            .any(|root| Arc::ptr_eq(root, &agent))
    {
        return Ok(());
    }
    let key = Arc::as_ptr(&agent).cast::<()>() as usize;
    {
        // Publication listeners may run concurrently. No await occurs while the
        // exact Agent's complete catalog is registered and entered in the map.
        let mut state = registrations.lock();
        if state.stopping || state.agents.contains_key(&key) {
            return Ok(());
        }
        let remove_tools =
            register_host_schedule_tools(ctx, agent.ctx(), agent.clone(), service.clone())
                .map_err(|message| ScheduleError::new("internal_error", message))?;
        let weak = Arc::downgrade(registrations);
        let owner = agent.ctx().effect(
            "schedule.hostAgent()",
            Box::pin(async move {
                Some(cordis::events::make_disposer(move || {
                    let weak = weak.clone();
                    let remove_tools = remove_tools.clone();
                    Box::pin(async move {
                        remove_tools().await;
                        if let Some(registrations) = weak.upgrade() {
                            registrations.lock().agents.remove(&key);
                        }
                    })
                }))
            }),
        );
        state.agents.insert(key, owner);
    }
    warn_legacy(ctx, registrations, agent.session());
    Ok(())
}

/// Enable dispatch and attach tools before returning the load boundary.
/// Disabling removes both existing and future registrations, but retains tasks.
pub async fn apply_host_schedule(
    ctx: &Context,
    service: Arc<ScheduleService>,
    config: Config,
) -> Result<Disposer, ScheduleError> {
    let registry = ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| {
            ScheduleError::new("internal_error", "Schedule requires the Agent registry.")
        })?;
    service.enable(config).await?;
    let registrations = Arc::new(Mutex::new(Registrations::default()));
    let listener: Arc<Listener> = Arc::new({
        let ctx = ctx.clone();
        let registry = registry.clone();
        let registrations = registrations.clone();
        let service = service.clone();
        move |_, args| {
            let ctx = ctx.clone();
            let registry = registry.clone();
            let registrations = registrations.clone();
            let service = service.clone();
            Box::pin(async move {
                if let Some(payload) = args.first().and_then(downcast::<AgentLifecyclePayload>) {
                    // A broken catalog must veto publication instead of letting
                    // a root's first turn observe only part of the tool set.
                    attach(
                        &ctx,
                        &registry,
                        &registrations,
                        &service,
                        payload.agent.clone(),
                    )
                    .unwrap_or_else(|error| panic!("{error}"));
                }
                None
            })
        }
    });
    let created = ctx
        .on(
            "agent/created",
            listener.clone(),
            EventOptions::default().global(true),
        )
        .await;
    let started = ctx
        .on(
            "agent/session-start",
            listener,
            EventOptions::default().global(true),
        )
        .await;
    let disposed = ctx
        .on(
            "agent/disposed",
            Arc::new({
                let registrations = registrations.clone();
                move |_, args| {
                    let registrations = registrations.clone();
                    Box::pin(async move {
                        let cleanup = args
                            .first()
                            .and_then(downcast::<AgentLifecyclePayload>)
                            .and_then(|payload| {
                                let key = Arc::as_ptr(&payload.agent).cast::<()>() as usize;
                                registrations.lock().agents.remove(&key)
                            });
                        if let Some(cleanup) = cleanup {
                            // AgentRegistry emits this event synchronously with
                            // block_on. Effect setup may still need the current Tokio
                            // thread, so drain it asynchronously; revoked ownership is
                            // already enforced by the tool's exact-registry guard.
                            tokio::spawn(async move {
                                cleanup().await;
                            });
                        }
                        None
                    })
                }
            }),
            EventOptions::default().global(true),
        )
        .await;
    let session_created = ctx
        .on(
            "session/created",
            Arc::new({
                let ctx = ctx.clone();
                let registrations = registrations.clone();
                move |_, args| {
                    let ctx = ctx.clone();
                    let registrations = registrations.clone();
                    Box::pin(async move {
                        if let Some(session) =
                            args.first().and_then(downcast::<dsh_session::Session>)
                        {
                            warn_legacy(&ctx, &registrations, session);
                        }
                        None
                    })
                }
            }),
            EventOptions::default().global(true),
        )
        .await;
    let lifecycle = cordis::events::make_disposer({
        let registrations = registrations.clone();
        let service = service.clone();
        let completed = Arc::new(tokio::sync::OnceCell::new());
        move || {
            let registrations = registrations.clone();
            let listeners = [
                created.clone(),
                started.clone(),
                disposed.clone(),
                session_created.clone(),
            ];
            let service = service.clone();
            let completed = completed.clone();
            Box::pin(async move {
                completed
                    .get_or_init(|| async move {
                        let cleanups = {
                            let mut state = registrations.lock();
                            state.stopping = true;
                            state
                                .agents
                                .drain()
                                .map(|(_, cleanup)| cleanup)
                                .collect::<Vec<_>>()
                        };
                        service.disable().await;
                        for listener in listeners {
                            listener().await;
                        }
                        for cleanup in cleanups {
                            cleanup().await;
                        }
                    })
                    .await;
            })
        }
    });
    for root in registry.roots() {
        if let Err(error) = attach(ctx, &registry, &registrations, &service, root) {
            lifecycle().await;
            return Err(error);
        }
    }
    Ok(lifecycle)
}

/// Loader entry; the Host profile decides whether it starts enabled.
pub struct HostSchedulePlugin {
    service: Arc<ScheduleService>,
}

impl HostSchedulePlugin {
    pub fn new(service: Arc<ScheduleService>) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl Plugin for HostSchedulePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("schedule")
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new([
            "agents",
            "sessions",
            "tools",
            "storageDomain",
            "sessionPersistence",
        ])
    }

    fn validate(&self, value: ArcValue) -> Result<ArcValue, cordis::ValidationError> {
        plugin_config(&value)
            .map(cordis::arc)
            .map_err(|error| cordis::ValidationError::new([error.to_string()]))
    }

    async fn apply(&self, ctx: &Context, value: ArcValue) -> Result<(), PluginError> {
        let config = plugin_config(&value)
            .map_err(|error| PluginError::new(cordis::arc(error.to_string())))?;
        let cleanup = apply_host_schedule(ctx, self.service.clone(), config)
            .await
            .map_err(|error| PluginError::new(cordis::arc(error.to_string())))?;
        ctx.effect(
            "schedule.hostLifecycle()",
            Box::pin(async move { Some(cleanup) }),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn loader_retention_configuration_is_strict_and_keeps_custom_values() {
        let config = decode_host_schedule_config(&json!({})).unwrap();
        assert_eq!(
            (
                config.delivery_history_days,
                config.delivery_history_records
            ),
            (30, 200)
        );
        let config = decode_host_schedule_config(
            &json!({"deliveryHistoryDays":3650,"deliveryHistoryRecords":10000}),
        )
        .unwrap();
        assert_eq!(
            (
                config.delivery_history_days,
                config.delivery_history_records
            ),
            (3650, 10000)
        );
        assert_eq!(
            decode_host_schedule_config(&json!({"deliveryHistoryDays":30.0}))
                .unwrap()
                .delivery_history_days,
            30
        );
        for value in [
            json!(null),
            json!([]),
            json!({"deliveryHistoryDays":0}),
            json!({"deliveryHistoryDays":3651}),
            json!({"deliveryHistoryRecords":0}),
            json!({"deliveryHistoryRecords":10001}),
            json!({"deliveryHistoryDays":1.5}),
            json!({"deliveryHistoryRecords":"200"}),
            json!({"deliveryHistoryDays":null}),
            json!({"other":true}),
        ] {
            assert!(decode_host_schedule_config(&value).is_err(), "{value}");
        }
    }
}
