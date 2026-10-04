//! Reminder visibility follows composition; delegated agents never receive it.

use std::{collections::HashMap, sync::Arc};

use cordis::{
    ArcValue, Context, Disposer, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
};
use dsh_agent::{Agent, AgentLifecyclePayload, AgentRegistry};
use dsh_tools::{ToolRestriction, ToolRuntime};
use parking_lot::Mutex;

const REMINDER_TOOLS: [&str; 4] = [
    "schedule_create",
    "schedule_list",
    "schedule_update",
    "schedule_delete",
];

/// The runtime shares this fence between visibility and execution, including
/// local and later optional registrations. `scheduled_task_*` is separate.
pub fn exclude_reminder_tools(ctx: &Context) -> Result<Disposer, String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or("reminder boundary requires tools")?;
    tools.restrict_all(
        ctx,
        ToolRestriction {
            allow: None,
            deny: Some(REMINDER_TOOLS.map(str::to_owned).to_vec()),
        },
    )
}

/// Agent-plane row for compositions that do not offer reminder management.
/// Changing the standing preset changes its inherited fence immediately.
pub struct ExcludeReminderToolsPlugin;

#[async_trait::async_trait]
impl Plugin for ExcludeReminderToolsPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("exclude-reminder-tools")
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["tools"])
    }
    async fn apply(&self, ctx: &Context, _: ArcValue) -> Result<(), PluginError> {
        exclude_reminder_tools(ctx)
            .map(|_| ())
            .map_err(|message| PluginError::new(cordis::arc(message)))
    }
}

#[derive(Default)]
struct Boundaries {
    stopping: bool,
    agents: HashMap<usize, Disposer>,
}

fn attach_child(
    registry: &AgentRegistry,
    boundaries: &Mutex<Boundaries>,
    agent: &Arc<dyn Agent>,
) -> Result<(), String> {
    if agent.session().header().origin.as_deref() != Some("subagent")
        && registry.roots().iter().any(|root| Arc::ptr_eq(root, agent))
    {
        return Ok(());
    }
    let key = Arc::as_ptr(agent).cast::<()>() as usize;
    let mut state = boundaries.lock();
    if !state.stopping && !state.agents.contains_key(&key) {
        state
            .agents
            .insert(key, exclude_reminder_tools(agent.ctx())?);
    }
    Ok(())
}

/// Permanent Host fence, independent of optional reminder delivery. It also
/// hides definitions copied into delegated scopes before reminders unload.
pub async fn install_host_schedule_tool_boundaries(ctx: &Context) -> Result<Disposer, String> {
    let registry = ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or("reminder boundary requires agents")?;
    let boundaries = Arc::new(Mutex::new(Boundaries::default()));
    let attach: Arc<Listener> = Arc::new({
        let registry = registry.clone();
        let boundaries = boundaries.clone();
        move |_, args| {
            let registry = registry.clone();
            let boundaries = boundaries.clone();
            Box::pin(async move {
                if let Some(payload) = args.first().and_then(downcast::<AgentLifecyclePayload>) {
                    attach_child(&registry, &boundaries, &payload.agent)
                        .unwrap_or_else(|message| panic!("{message}"));
                }
                None
            })
        }
    });
    let created = ctx
        .on(
            "agent/created",
            attach.clone(),
            EventOptions::default().global(true),
        )
        .await;
    let started = ctx
        .on(
            "agent/session-start",
            attach,
            EventOptions::default().global(true),
        )
        .await;
    let disposed = ctx
        .on(
            "agent/disposed",
            Arc::new({
                let boundaries = boundaries.clone();
                move |_, args| {
                    let boundaries = boundaries.clone();
                    Box::pin(async move {
                        if let Some(payload) =
                            args.first().and_then(downcast::<AgentLifecyclePayload>)
                        {
                            // Keep the scope-owned fence until its actual scope
                            // disposes; registry retirement cannot expose copies.
                            boundaries
                                .lock()
                                .agents
                                .remove(&(Arc::as_ptr(&payload.agent).cast::<()>() as usize));
                        }
                        None
                    })
                }
            }),
            EventOptions::default().global(true),
        )
        .await;
    let cleanup = cordis::events::make_disposer({
        let boundaries = boundaries.clone();
        move || {
            let listeners = [created.clone(), started.clone(), disposed.clone()];
            let boundaries = boundaries.clone();
            Box::pin(async move {
                let fences = {
                    let mut state = boundaries.lock();
                    state.stopping = true;
                    state
                        .agents
                        .drain()
                        .map(|(_, fence)| fence)
                        .collect::<Vec<_>>()
                };
                for listener in listeners {
                    listener().await;
                }
                for fence in fences {
                    fence().await;
                }
            })
        }
    });
    for agent in registry.list() {
        if let Err(message) = attach_child(&registry, &boundaries, &agent) {
            cleanup().await;
            return Err(message);
        }
    }
    Ok(ctx.effect(
        "schedule.hostBoundaries()",
        Box::pin(async move { Some(cleanup) }),
    ))
}
