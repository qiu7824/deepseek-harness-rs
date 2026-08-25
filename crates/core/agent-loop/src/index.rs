//! Concrete agent-loop plugin: creates scoped ReactLoopAgents, publishes
//! them through the agent/session registries, and owns their ordered
//! teardown. Rust port of `packages/core/agent-loop/src/index.ts`.
//!
//! # Deviations
//!
//! - Launcher-owned configured identities (`CONFIGURED_AGENT_IDENTITIES_KEY`
//!   / `ctx.provide`) are not wired (no cordis provide mechanism yet);
//!   configured agents keep their config identities.
//! - `AbortSignal` collapses to [`dsh_agent::CancellationSignal`] (flag +
//!   reason cell); `raceAbort`/`raceAbortCall` become flag checks around
//!   each await.
//! - The `sessionPersistence` service base has no backend contract yet, so
//!   `resume` always reports the not-configured error until a backend
//!   lands.
//! - The `systemPrompt` variables (`provider`/`model`/`cwd`) reading
//!   `context.agent` are skipped until `AssembleContext.agent` lands.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{ArcValue, BoxFuture, Context, DispatchMode, Disposer, InjectSpec, Service, arc};
use dsh_agent::{
    Agent, AgentFactory, AgentHandle, AgentOptions, AgentSessionStartPayload, AgentSetup,
    CreateAgentOptions, ResumeAgentOptions, SessionStartSource, emit_agent_event,
};
use dsh_session::{Session, SessionId, SessionPreparation, SessionPreparationOptions, session_id};
use dsh_settings::{install_settings_section, settings_namespace};
use indexmap::IndexMap;
use schemastery::{Data, Schema};

use crate::agent::ReactLoopAgent;
use crate::constants::DEFAULT_MAX_PARALLEL_TOOL_CALLS;

/// Context key a launcher sets before any Loader entry mounts (reserved;
/// the provide mechanism lands with the launcher milestone).
pub const CONFIGURED_AGENT_IDENTITIES_KEY: &str = "configuredAgentIdentities";

/// Settings namespace carrying the tool-call parallelism a user owns.
pub fn agent_loop_settings_namespace() -> dsh_settings::SettingsNamespace {
    settings_namespace("agent-loop").expect("valid namespace")
}

/// The schema of the agent-loop settings section.
pub fn agent_loop_settings_schema() -> Schema {
    let mut properties: IndexMap<String, Schema> = IndexMap::new();
    properties.insert("maxParallelToolCalls".to_string(), Schema::number());
    Schema::object(properties)
}

/// Reject an output-token cap that cannot be represented exactly on the
/// request wire.
fn assert_agent_options(options: &AgentOptions) -> Result<(), String> {
    if options.max_tokens == Some(0) {
        return Err("agent maxTokens must be a positive safe integer".to_string());
    }
    if options
        .reasoning_effort
        .as_ref()
        .is_some_and(|effort| effort.as_str().trim().is_empty())
    {
        return Err("agent reasoningEffort must be a non-empty string".to_string());
    }
    Ok(())
}

/// Resolve the deployment-wide scheduler cap at the owning config boundary.
fn resolve_max_parallel_tool_calls(value: Option<u64>) -> Result<u64, String> {
    let cap = value.unwrap_or(DEFAULT_MAX_PARALLEL_TOOL_CALLS);
    if cap < 1 {
        return Err("maxParallelToolCalls must be a positive integer".to_string());
    }
    Ok(cap)
}

/// One declarative agent entry.
#[derive(Clone, Default)]
pub struct ConfiguredAgent {
    /// Stable config label used in logs and as the fresh combined-id prefix.
    pub id: String,
    /// Optional stable identity; remounts resume its materialized history,
    /// while first use creates it fresh.
    pub session_id: Option<SessionId>,
    /// Optional workspace for a fresh session.
    pub cwd: Option<String>,
    /// Persisted session to resume instead of creating a fresh session.
    pub resume_session_id: Option<SessionId>,
    /// Per-agent loop options.
    pub options: AgentOptions,
}

/// Agent-loop plugin configuration.
#[derive(Clone, Default)]
pub struct Config {
    /// Maximum parallel-safe calls in flight per agent step.
    pub max_parallel_tool_calls: Option<u64>,
    /// Agents created or resumed at plugin startup.
    pub agents: Vec<ConfiguredAgent>,
}

/// Reject self-contained identity conflicts before any configured agent
/// starts.
fn validate_configured_agents(agents: &[ConfiguredAgent]) -> Result<(), String> {
    let mut exact_identities: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for agent in agents {
        let has_resume_id = agent
            .resume_session_id
            .as_ref()
            .is_some_and(|id| !id.as_str().is_empty());
        if agent.session_id.is_some() && has_resume_id {
            return Err(format!(
                "agent \"{}\": sessionId and resumeSessionId are mutually exclusive",
                agent.id
            ));
        }
        let exact_identity = if has_resume_id {
            agent.resume_session_id.as_ref()
        } else {
            agent.session_id.as_ref()
        };
        let Some(exact_identity) = exact_identity else {
            continue;
        };
        if let Some(first_id) = exact_identities.get(exact_identity.as_str()) {
            return Err(format!(
                "agents \"{first_id}\" and \"{}\" use duplicate exact session identity \"{}\"",
                agent.id,
                exact_identity.as_str()
            ));
        }
        exact_identities.insert(exact_identity.as_str().to_string(), agent.id.clone());
    }
    Ok(())
}

/// Factory-level ownership: live agent teardowns plus config startup work.
struct FactoryOwnership {
    accepting: AtomicBool,
    teardown: Arc<dsh_agent::CancellationSignal>,
    live_agents: parking_lot::Mutex<Vec<Arc<PreparedAgent>>>,
    startup_tasks: parking_lot::Mutex<Vec<BoxFuture<'static, ()>>>,
}

impl FactoryOwnership {
    fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            teardown: dsh_agent::CancellationSignal::new(),
            live_agents: parking_lot::Mutex::new(Vec::new()),
            startup_tasks: parking_lot::Mutex::new(Vec::new()),
        }
    }

    fn is_active(&self) -> bool {
        self.accepting.load(Ordering::SeqCst) && !self.teardown.aborted()
    }

    /// Track one live agent's shared teardown until it has run. Dispose
    /// futures are memoized and idempotent, so the TS untrack pair collapses
    /// to no-op.
    fn track(&self, prepared: Arc<PreparedAgent>) {
        self.live_agents.lock().push(prepared);
    }

    fn owned(&self, agent: &Arc<dyn Agent>) -> Option<Arc<PreparedAgent>> {
        self.live_agents
            .lock()
            .iter()
            .find(|prepared| Arc::ptr_eq(&(prepared.agent.clone() as Arc<dyn Agent>), agent))
            .cloned()
    }

    /// Join config startup work that begins before an agent exists.
    fn track_startup(&self, job: BoxFuture<'static, ()>) {
        self.startup_tasks.lock().push(job);
    }

    async fn dispose(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        self.teardown
            .abort_with(dsh_agent::AgentCancelCause::Disposed);
        let live = std::mem::take(&mut *self.live_agents.lock());
        let startup = std::mem::take(&mut *self.startup_tasks.lock());
        for prepared in live {
            prepared.dispose().await;
        }
        for task in startup {
            task.await;
        }
    }
}

/// Prepared-but-unpublished agent resources sharing one memoized teardown.
struct PreparedAgent {
    agent: Arc<ReactLoopAgent>,
    session: Session,
    loop_ctx: Context,
    owner_agent: Option<Arc<dyn Agent>>,
    lifecycle: parking_lot::Mutex<PreparedLifecycle>,
    dispose_started: AtomicBool,
    dispose_done: tokio::sync::watch::Sender<bool>,
}

#[derive(Default)]
struct PreparedLifecycle {
    closing: bool,
    detach_agent: Option<Disposer>,
    detach_session: Option<Disposer>,
}

struct DisposeCompletion {
    done: tokio::sync::watch::Sender<bool>,
}

impl Drop for DisposeCompletion {
    fn drop(&mut self) {
        self.done.send_replace(true);
    }
}

impl PreparedAgent {
    /// Reverse teardown: stop the machine, unregister, unwind the scope.
    /// Memoized.
    fn dispose(self: &Arc<Self>) -> BoxFuture<'static, ()> {
        let prepared = Arc::clone(self);
        Box::pin(async move {
            let mut done = prepared.dispose_done.subscribe();
            if !prepared.dispose_started.swap(true, Ordering::SeqCst) {
                let task = Arc::clone(&prepared);
                tokio::spawn(async move {
                    let _completion = DisposeCompletion {
                        done: task.dispose_done.clone(),
                    };
                    let agent = Arc::clone(&task.agent);
                    let (detach_agent, detach_session) = {
                        let mut lifecycle = task.lifecycle.lock();
                        lifecycle.closing = true;
                        (
                            lifecycle.detach_agent.take(),
                            lifecycle.detach_session.take(),
                        )
                    };
                    agent.cancel(
                        dsh_agent::AgentCancelCause::Disposed,
                        Some(&dsh_agent::CancelOptions { keep_inbox: false }),
                    );
                    agent.when_idle().await;
                    (agent.scope().dispose)().await;
                    if let Some(detach_agent) = detach_agent {
                        detach_agent().await;
                    }
                    if let Some(detach_session) = detach_session {
                        detach_session().await;
                    }
                });
            }
            while !*done.borrow() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        })
    }
}

/// Concrete agent factory and driver service.
pub struct AgentLoop {
    ctx: Context,
    ownership: Arc<FactoryOwnership>,
    max_parallel_tool_calls: parking_lot::Mutex<u64>,
}

impl Service for AgentLoop {
    fn service_name(&self) -> &'static str {
        "agentLoop"
    }
}

impl AgentLoop {
    /// Create the service, register it, publish the factory, and start the
    /// configured agents.
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        validate_configured_agents(&config.agents)?;
        let cap = resolve_max_parallel_tool_calls(config.max_parallel_tool_calls)?;
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            ownership: Arc::new(FactoryOwnership::new()),
            max_parallel_tool_calls: parking_lot::Mutex::new(cap),
        });
        ctx.register_service(service.clone());
        let system_prompt = ctx
            .get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "agent-loop requires the systemPrompt service".to_string())?;
        let _cwd_variable = system_prompt.variable(
            ctx,
            "cwd",
            Arc::new(|context| context.field_str("cwd").map(str::to_string)),
        );

        // User-owned parallelism cap (optional settings seam).
        let mut entry: IndexMap<String, Data> = IndexMap::new();
        entry.insert("maxParallelToolCalls".to_string(), Data::Number(cap as f64));
        let _ = install_settings_section(
            ctx,
            agent_loop_settings_namespace(),
            agent_loop_settings_schema(),
            Data::Object(entry),
            dsh_settings::SettingsSectionHooks {
                set_source: {
                    let service = Arc::clone(&service);
                    Arc::new(move |_source: Arc<dyn Fn() -> Data + Send + Sync>| {
                        let service = Arc::clone(&service);
                        std::thread::spawn(move || {
                            let _ = service;
                        });
                    })
                },
                on_change: Arc::new(|| {}),
                validate: Some(Arc::new(|value: &Data| {
                    let Data::Object(object) = value else {
                        return Ok(());
                    };
                    let cap = object
                        .get("maxParallelToolCalls")
                        .and_then(|value| match value {
                            Data::Number(number) => Some(*number as u64),
                            _ => None,
                        })
                        .unwrap_or(DEFAULT_MAX_PARALLEL_TOOL_CALLS);
                    resolve_max_parallel_tool_calls(Some(cap)).map(|_| ())
                })),
            },
        );

        // Publish the factory and own its teardown.
        let factory: Arc<dyn AgentFactory> = service.clone();
        let factory_ctx = ctx.clone();
        let _ = ctx.effect(
            "agentLoop.setFactory()",
            Box::pin(async move {
                let agents = factory_ctx
                    .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
                    .map(|arc| arc.as_ref().clone());
                Some(match agents {
                    Some(agents) => agents.set_factory(factory),
                    None => {
                        return None;
                    }
                })
            }),
        );
        let ownership = Arc::clone(&service.ownership);
        let _ = ctx.effect(
            "agentLoop.transactions()",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let ownership = Arc::clone(&ownership);
                    Box::pin(async move {
                        ownership.dispose().await;
                    })
                }))
            }),
        );

        // Start the configured agents.
        for entry in config.agents {
            service.start_configured(entry)?;
        }
        Ok(service)
    }

    /// The live scheduler cap (read by the tool-call scheduler).
    pub fn max_parallel_tool_calls(&self) -> u64 {
        *self.max_parallel_tool_calls.lock()
    }

    fn start_configured(self: &Arc<Self>, entry: ConfiguredAgent) -> Result<(), String> {
        if entry
            .resume_session_id
            .as_ref()
            .is_some_and(|id| !id.as_str().is_empty())
        {
            let resume_session_id = entry.resume_session_id.expect("checked");
            let options = entry.options.clone();
            let service = Arc::clone(self);
            let ctx = self.ctx.clone();
            let _ = ctx.inject(
                InjectSpec::new(["sessionPersistence"]),
                Arc::new(move |child_ctx: &Context, _config: ArcValue| {
                    let child_ctx = child_ctx.clone();
                    let service = Arc::clone(&service);
                    let resume_session_id = resume_session_id.clone();
                    let options = options.clone();
                    Box::pin(async move {
                        let result = service
                            .resume_with(&child_ctx, &resume_session_id, &options, None)
                            .await;
                        if let Err(error) = result {
                            service.report_configured_startup_failure(
                                "configured",
                                "resume",
                                &resume_session_id,
                                &error,
                            );
                        }
                        Ok(())
                    })
                }),
            );
            return Ok(());
        }
        let configured_id = entry.session_id.clone().unwrap_or_else(|| {
            session_id(format!("{}-session-{}", entry.id, uuid::Uuid::new_v4()))
        });
        self.schedule_configured_create(entry.id, configured_id, entry.options, entry.cwd);
        Ok(())
    }

    fn schedule_configured_create(
        self: &Arc<Self>,
        config_id: String,
        configured_id: SessionId,
        options: AgentOptions,
        cwd: Option<String>,
    ) {
        let service = Arc::clone(self);
        let ctx = self.ctx.clone();
        let _ = ctx.effect(
            "agentLoop.configuredCreate()",
            Box::pin(async move {
                let prepared = (|| {
                    assert_agent_options(&options)?;
                    let sessions = service
                        .ctx
                        .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                        .map(|arc| arc.as_ref().clone())
                        .ok_or_else(|| "agent loop requires the sessions service".to_string())?;
                    let session = sessions.prepare(
                        Some(configured_id.clone()),
                        Some(dsh_session::CreateSessionOptions {
                            meta: Some(dsh_session::CreateSessionMeta {
                                cwd,
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                    )?;
                    service.prepare(&service.ctx, &configured_id, &options, session)
                })();
                let result = match prepared {
                    Ok(prepared) => prepared
                        .publish(SessionStartSource::Startup)
                        .await
                        .map(|_| ()),
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    service.report_configured_startup_failure(
                        &config_id,
                        "create",
                        &configured_id,
                        &error,
                    );
                }
                None
            }),
        );
    }

    /// Report a contained declarative-start failure to identity-bound
    /// consumers.
    fn report_configured_startup_failure(
        &self,
        config_id: &str,
        action: &str,
        session_id: &SessionId,
        error: &str,
    ) {
        if !self.ownership.is_active() {
            return;
        }
        self.ctx
            .named_logger(Some("agentLoop"))
            .warn(vec![arc(format!(
                "agent \"{config_id}\": config-driven {action} of \"{}\" failed: {error}",
                session_id.as_str()
            ))]);
        let payload = arc(serde_json::json!({
            "sessionId": session_id,
            "error": error,
        }));
        let listeners = self.ctx.collect(
            DispatchMode::Emit,
            "agent-loop/config-start-failed",
            &[payload.clone()],
        );
        for (listener_ctx, listener) in listeners {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                futures::executor::block_on(listener(&listener_ctx, vec![payload.clone()]));
            }));
        }
    }

    async fn setup_and_publish(
        &self,
        owner_ctx: &Context,
        id: &SessionId,
        mut preparation: SessionPreparation,
        agent_options: &AgentOptions,
        setup: Option<&AgentSetup>,
        source: SessionStartSource,
    ) -> Result<AgentHandle, String> {
        let session = preparation.session.clone();
        let prepared = self.prepare(owner_ctx, id, agent_options, session)?;
        if let Some(setup) = setup {
            let exact_agent: Arc<dyn Agent> = prepared.agent.clone();
            let commit = setup(prepared.agent.ctx(), exact_agent).await?;
            if let Some(commit) = commit {
                commit.commit();
            }
        }
        preparation.dispose();
        prepared.publish(source).await
    }

    async fn resume_with(
        &self,
        owner_ctx: &Context,
        id: &SessionId,
        options: &AgentOptions,
        setup: Option<&AgentSetup>,
    ) -> Result<AgentHandle, String> {
        let persistence = self
            .ctx
            .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                "sessionPersistence",
                false,
            )
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| {
                "cannot resume: session persistence is not configured (load a dsh-session-persistence backend)"
                    .to_string()
            })?;
        let inspection = persistence.inspect(id).await?;
        let session = Session::from_restore(id.clone(), inspection.events, &inspection.meta)?;
        let preparation = SessionPreparation::create(session, SessionPreparationOptions::default());
        self.setup_and_publish(
            owner_ctx,
            id,
            preparation,
            options,
            setup,
            SessionStartSource::Resume,
        )
        .await
    }

    /// Construct the driver, scope, and one memoized reverse teardown for a
    /// new agent.
    fn prepare(
        &self,
        owner_ctx: &Context,
        id: &SessionId,
        options: &AgentOptions,
        session: Session,
    ) -> Result<Arc<PreparedAgent>, String> {
        assert_agent_options(options)?;
        if !self.ownership.is_active() {
            return Err("agent loop is not active".to_string());
        }
        let agent = ReactLoopAgent::new(&self.ctx, id.clone(), options.clone(), session.clone())?;
        let (dispose_done, _) = tokio::sync::watch::channel(false);
        let prepared = Arc::new(PreparedAgent {
            agent,
            session,
            loop_ctx: self.ctx.clone(),
            owner_agent: owner_ctx
                .get_typed::<Arc<dyn Agent>>("agent", false)
                .map(|arc| arc.as_ref().clone()),
            lifecycle: parking_lot::Mutex::new(PreparedLifecycle::default()),
            dispose_started: AtomicBool::new(false),
            dispose_done,
        });
        self.ownership.track(prepared.clone());
        Ok(prepared)
    }
}

impl PreparedAgent {
    /// Enter registries, announce, notify session-start, and hand out the
    /// published handle.
    async fn publish(self: &Arc<Self>, source: SessionStartSource) -> Result<AgentHandle, String> {
        let sessions = self
            .loop_ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .map(|arc| arc.as_ref().clone())
            .ok_or_else(|| "agent loop requires the sessions service".to_string())?;
        let agents = self
            .loop_ctx
            .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
            .map(|arc| arc.as_ref().clone())
            .ok_or_else(|| "agent loop requires the agents service".to_string())?;
        let agent_dyn: Arc<dyn Agent> = self.agent.clone();
        {
            let mut lifecycle = self.lifecycle.lock();
            if lifecycle.closing {
                return Err("agent loop disposed while publishing an agent".to_string());
            }
            let detach_session = sessions.enter(&self.session)?;
            let detach_agent = match agents.enter(Arc::clone(&agent_dyn), self.owner_agent.clone())
            {
                Ok(detach) => detach,
                Err(error) => {
                    futures::executor::block_on(detach_session());
                    return Err(error);
                }
            };
            lifecycle.detach_session = Some(detach_session);
            lifecycle.detach_agent = Some(detach_agent);
        }
        if let Err(error) = sessions.announce(&self.session).await {
            let (detach_agent, detach_session) = {
                let mut lifecycle = self.lifecycle.lock();
                (
                    lifecycle.detach_agent.take(),
                    lifecycle.detach_session.take(),
                )
            };
            if let Some(detach) = detach_agent {
                detach().await;
            }
            if let Some(detach) = detach_session {
                detach().await;
            }
            return Err(error);
        }
        emit_agent_event(&self.loop_ctx, &agent_dyn, "agent/session-start", |agent| {
            arc(AgentSessionStartPayload {
                agent: Arc::clone(agent),
                source,
            })
        });
        let prepared = Arc::clone(self);
        Ok(AgentHandle {
            agent: Arc::clone(&agent_dyn),
            dispose: prepared.dispose(),
        })
    }
}

#[async_trait::async_trait]
impl AgentFactory for AgentLoop {
    fn can_retire(&self, agent: &Arc<dyn Agent>) -> bool {
        self.ownership.owned(agent).is_some()
    }

    async fn retire(&self, agent: Arc<dyn Agent>) -> Result<bool, String> {
        let Some(prepared) = self.ownership.owned(&agent) else {
            return Ok(false);
        };
        prepared.dispose().await;
        Ok(true)
    }

    async fn create_agent(
        &self,
        owner_ctx: &Context,
        options: CreateAgentOptions,
    ) -> Result<AgentHandle, String> {
        let sessions = self
            .ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .map(|arc| arc.as_ref().clone())
            .ok_or_else(|| "agent loop requires the sessions service".to_string())?;
        let id = options
            .session_id
            .clone()
            .unwrap_or_else(|| session_id(format!("agent-session-{}", uuid::Uuid::new_v4())));
        let session = sessions.prepare(
            Some(id.clone()),
            Some(dsh_session::CreateSessionOptions {
                seed: options.seed.clone(),
                meta: Some(options.meta.clone().unwrap_or_default()),
                ..Default::default()
            }),
        )?;
        let preparation = SessionPreparation::create(session, SessionPreparationOptions::default());
        self.setup_and_publish(
            owner_ctx,
            &id,
            preparation,
            &options.agent_options.clone().unwrap_or_default(),
            options.setup.as_ref(),
            SessionStartSource::Startup,
        )
        .await
    }

    async fn resume(
        &self,
        owner_ctx: &Context,
        options: ResumeAgentOptions,
    ) -> Result<AgentHandle, String> {
        let Some(id) = options.resume_session_id.clone() else {
            return Err("cannot resume: resumeSessionId is required".to_string());
        };
        self.resume_with(
            owner_ctx,
            &id,
            &options.agent_options.clone().unwrap_or_default(),
            options.setup.as_ref(),
        )
        .await
    }
}
