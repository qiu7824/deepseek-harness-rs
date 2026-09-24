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
//! - The `systemPrompt` variables read the equivalent scalar fields materialized
//!   by `assemble_context_for` instead of retaining the live Agent in the
//!   assembly object.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{ArcValue, BoxFuture, Context, DispatchMode, Disposer, InjectSpec, Service, arc};
use dsh_agent::{
    Agent, AgentFactory, AgentHandle, AgentOptions, AgentSetup, CreateAgentOptions,
    ResumeAgentOptions, SessionStartSource,
};
use dsh_session::{Session, SessionId, SessionPreparation, SessionPreparationOptions, session_id};
use dsh_settings::{install_settings_section, settings_namespace};
use futures::FutureExt;
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
    if options.max_steps == Some(0) || options.timeout_seconds == Some(0) {
        return Err("agent maxSteps and timeoutSeconds must be positive when supplied".into());
    }
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

    fn untrack(&self, prepared: &Arc<PreparedAgent>) {
        let removed = {
            let mut live = self.live_agents.lock();
            live.iter()
                .position(|entry| Arc::ptr_eq(entry, prepared))
                .map(|index| live.swap_remove(index))
        };
        drop(removed);
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
        let live = {
            let live = self.live_agents.lock();
            self.accepting.store(false, Ordering::SeqCst);
            live.clone()
        };
        self.teardown
            .abort_with(dsh_agent::AgentCancelCause::Disposed);
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
    ownership: std::sync::Weak<FactoryOwnership>,
    agent: Arc<ReactLoopAgent>,
    session: Session,
    loop_ctx: Context,
    owner_agent: Option<Arc<dyn Agent>>,
    lifecycle: parking_lot::Mutex<PreparedLifecycle>,
    dispose_started: AtomicBool,
    dispose_done: tokio::sync::watch::Sender<Option<Result<(), String>>>,
}

#[derive(Default)]
struct PreparedLifecycle {
    closing: bool,
    detach_agent: Option<Disposer>,
    detach_session: Option<Disposer>,
}

/// Cancellation of create/resume must start the same owned rollback as errors.
struct PublicationGuard(Option<Arc<PreparedAgent>>);

impl Drop for PublicationGuard {
    fn drop(&mut self) {
        if let Some(prepared) = self.0.take() {
            prepared.start_dispose();
        }
    }
}

impl PreparedAgent {
    fn start_dispose(self: &Arc<Self>) {
        if self.dispose_done.send_if_modified(|status| {
            if matches!(status, Some(Err(_))) {
                *status = None;
                true
            } else {
                false
            }
        }) {
            self.dispose_started.store(false, Ordering::SeqCst);
        }
        if self.dispose_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let task = Arc::clone(self);
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(async {
                task.lifecycle.lock().closing = true;
                task.agent.cancel(
                    dsh_agent::AgentCancelCause::Disposed,
                    Some(&dsh_agent::CancelOptions { keep_inbox: false }),
                );
                task.agent.when_idle().await;
                (task.agent.scope().dispose)().await;
                let (detach_agent, detach_session) = {
                    let lifecycle = task.lifecycle.lock();
                    (
                        lifecycle.detach_agent.clone(),
                        lifecycle.detach_session.clone(),
                    )
                };
                if let Some(detach) = detach_agent {
                    detach().await;
                }
                if let Some(detach) = detach_session {
                    detach().await;
                }
                {
                    let mut lifecycle = task.lifecycle.lock();
                    lifecycle.detach_agent = None;
                    lifecycle.detach_session = None;
                }
                if let Some(ownership) = task.ownership.upgrade() {
                    ownership.untrack(&task);
                }
            })
            .catch_unwind()
            .await
            .map_err(|_| "agent teardown panicked; ownership retained".to_string());
            if let Err(error) = &result {
                task.loop_ctx
                    .named_logger(Some("agentLoop"))
                    .warn(vec![arc(error.clone())]);
            }
            task.dispose_done.send_replace(Some(result));
        });
    }

    /// Reverse teardown: stop the machine, unregister, unwind the scope.
    /// Memoized.
    fn dispose(self: &Arc<Self>) -> BoxFuture<'static, ()> {
        let prepared = Arc::clone(self);
        Box::pin(async move {
            let mut done = prepared.dispose_done.subscribe();
            prepared.start_dispose();
            while done.borrow().is_none() {
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
        let _provider_variable = system_prompt.variable(
            ctx,
            "provider",
            Arc::new(|context| context.field_str("provider").map(str::to_string)),
        );
        let _model_variable = system_prompt.variable(
            ctx,
            "model",
            Arc::new(|context| context.field_str("model").map(str::to_string)),
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
        // Factory availability is synchronous with install, just like the
        // service itself. Deferring this write to an effect task lets an
        // immediately-created parent dispatch a child before the factory exists.
        let factory_disposer = ctx
            .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
            .map(|agents| agents.set_factory(factory));
        let _ = ctx.effect(
            "agentLoop.setFactory()",
            Box::pin(async move { factory_disposer }),
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
                let result = async {
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
                    let preparation = service.prepare_new_session(session).await?;
                    service
                        .setup_and_publish(
                            &service.ctx,
                            &configured_id,
                            preparation,
                            &options,
                            None,
                            SessionStartSource::Startup,
                        )
                        .await
                        .map(|_| ())
                }
                .await;
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

    async fn prepare_new_session(&self, session: Session) -> Result<SessionPreparation, String> {
        let persistence = self
            .ctx
            .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                "sessionPersistence",
                false,
            )
            .map(|slot| slot.as_ref().clone());
        match persistence {
            Some(persistence) => persistence.prepare_new(session).await,
            None => Ok(SessionPreparation::create(
                session,
                SessionPreparationOptions::default(),
            )),
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
        let mut rollback = PublicationGuard(Some(prepared.clone()));
        if let Some(setup) = setup {
            let exact_agent: Arc<dyn Agent> = prepared.agent.clone();
            let commit = match setup(prepared.agent.ctx(), exact_agent).await {
                Ok(commit) => commit,
                Err(error) => {
                    prepared.dispose().await;
                    return Err(error);
                }
            };
            if let Some(commit) = commit {
                commit.commit();
            }
        }
        let published = prepared.publish(source).await;
        preparation.dispose();
        match published {
            Ok(handle) => {
                rollback.0 = None;
                Ok(handle)
            }
            Err(error) => {
                prepared.dispose().await;
                Err(error)
            }
        }
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
        let preparation = persistence.prepare(id).await?;
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
        let mut live = self.ownership.live_agents.lock();
        if !self.ownership.is_active() {
            return Err("agent loop is not active".to_string());
        }
        let agent = ReactLoopAgent::new(&self.ctx, id.clone(), options.clone(), session.clone())?;
        agent.hold_publication();
        let (dispose_done, _) = tokio::sync::watch::channel(None);
        let prepared = Arc::new(PreparedAgent {
            ownership: Arc::downgrade(&self.ownership),
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
        live.push(prepared.clone());
        Ok(prepared)
    }
}

impl PreparedAgent {
    /// Enter registries, announce, notify session-start, and hand out the
    /// published handle.
    async fn publish(self: &Arc<Self>, _source: SessionStartSource) -> Result<AgentHandle, String> {
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
            lifecycle.detach_session = Some(sessions.enter(&self.session)?);
            lifecycle.detach_agent =
                Some(agents.enter(Arc::clone(&agent_dyn), self.owner_agent.clone())?);
        }
        sessions.announce(&self.session).await?;
        agents.announce(&agent_dyn).await?;
        {
            let lifecycle = self.lifecycle.lock();
            if lifecycle.closing {
                return Err("agent closed during publication".into());
            }
            self.agent.release_publication();
        }
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
        match prepared.dispose_done.borrow().as_ref() {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(error.clone()),
            None => return Err("agent teardown retry is still in progress".into()),
        }
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
                inherited_event_count: options.inherited_event_count,
                meta: Some(options.meta.clone().unwrap_or_default()),
            }),
        )?;
        let preparation = self.prepare_new_session(session).await?;
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
