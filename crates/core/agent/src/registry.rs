//! Agent registry: live store, factory delegation, and process-local
//! initiator attribution. Rust port of `packages/core/agent/src/index.ts`.
//!
//! # Deviations
//!
//! - `Agent` is `Arc<dyn Agent>` (trait object) instead of the TS interface.
//! - The initiator ambient value uses ONE process-global `tokio::task_local!`
//!   slot instead of per-service-instance `AsyncLocalStorage`; the value
//!   propagates through `tokio::spawn` children only (documented).
//! - `register`/`announce` follow the session store's async-veto pattern;
//!   the exact Cordis effect disposer contract is preserved.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cordis::{
    ArcValue, Context, DispatchMode, Disposer, EventOptions, FiberState, InjectSpec, Listener,
    Service, arc, make_disposer,
};
use dsh_scope::{ScopeCarrier, scope_chain_of, scope_of, scope_target};
use dsh_session::{SessionId, session_id};
use dsh_typert_protocol::{TypertLookup, TypertService};
use parking_lot::Mutex;

use crate::runtime_types::{
    Agent, AgentFactory, AgentHandle, AgentLifecyclePayload, CreateAgentOptions, ResumeAgentOptions,
};

const NO_FACTORY_MESSAGE: &str = "no agent factory registered (load an agent-loop plugin)";
const NO_INITIATOR_MESSAGE: &str = "no initiating agent is active";
const DISPOSED_INITIATOR_MESSAGE: &str = "agent initiator scope is disposed";

/// Render a caught panic payload for logging (shared with dispatch).
pub(crate) fn render_panic(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<&str>() {
        Ok(message) => message.to_string(),
        Err(payload) => match payload.downcast::<String>() {
            Ok(message) => *message,
            Err(_) => "<non-string panic>".to_string(),
        },
    }
}

/// All mutable lifecycle state for one exact registry entry.
struct AgentEntry {
    id: SessionId,
    agent: Arc<dyn Agent>,
    /// Runtime creator-agent ownership; independent of durable session
    /// lineage.
    owner: Option<Arc<dyn Agent>>,
    carrier: ScopeCarrier,
    emit_ctx: Context,
    flags: Mutex<EntryFlags>,
    /// Store-owned detach transition (TS `entry.detach()` closure).
    detach: Arc<dyn Fn(&Arc<AgentEntry>) + Send + Sync>,
}

#[derive(Default)]
struct EntryFlags {
    announced: bool,
    announcing: bool,
    detach_requested: bool,
}

impl AgentEntry {
    fn begin_announce(&self) -> bool {
        let mut flags = self.flags.lock();
        if flags.announced || flags.announcing {
            return false;
        }
        flags.announcing = true;
        flags.announced = true;
        true
    }

    fn finish_announce(&self) -> bool {
        let mut flags = self.flags.lock();
        flags.announcing = false;
        flags.detach_requested
    }

    fn request_detach_if_announcing(&self) -> bool {
        let mut flags = self.flags.lock();
        if flags.announcing {
            flags.detach_requested = true;
            return true;
        }
        false
    }

    fn is_announced(&self) -> bool {
        self.flags.lock().announced
    }

    fn set_detach_requested(&self, value: bool) {
        self.flags.lock().detach_requested = value;
    }

    fn detach_now(self: &Arc<Self>) {
        (self.detach)(self);
    }
}

/// One tracked initiator boundary plus its inherited nesting chain.
struct InitiatorRun {
    active: std::sync::atomic::AtomicBool,
    parent: Option<Arc<InitiatorRun>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InitiatorState {
    Active,
    Closing,
    Disposed,
}

// The process-global ambient initiator slot (TS `AsyncLocalStorage`; one
// slot for the whole process — see the module deviation notes).
tokio::task_local! {
    static AMBIENT_INITIATOR: Option<Arc<dyn Agent>>;
    static AMBIENT_INITIATOR_RUN: Option<Arc<InitiatorRun>>;
}

/// Agent service (`ctx.agents`): tracks live agents and carries the
/// initiating Agent through one process-local asynchronous driver chain.
pub struct AgentRegistry {
    pub ctx: Context,
    store: Arc<Mutex<HashMap<String, Arc<AgentEntry>>>>,
    factory: Arc<Mutex<Option<Arc<dyn AgentFactory>>>>,
    initiator_state: Mutex<InitiatorState>,
    initiator_gate: Mutex<()>,
    active_initiator_runs: AtomicU32,
    initiator_drain: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    initiator_disposal: Arc<tokio::sync::OnceCell<()>>,
}

impl AgentRegistry {
    /// Whether the registered factory owns this exact live lifecycle.
    pub fn can_retire(&self, agent: &Arc<dyn Agent>) -> bool {
        self.factory
            .lock()
            .as_ref()
            .is_some_and(|factory| factory.can_retire(agent))
    }

    /// Ask the registered structural owner to retire this exact lifecycle.
    pub async fn retire(&self, agent: Arc<dyn Agent>) -> Result<bool, String> {
        let factory = self.factory.lock().clone();
        match factory {
            Some(factory) => factory.retire(agent).await,
            None => Ok(false),
        }
    }

    /// Create the registry, register it as the `agents` service, and wire
    /// the typert lookups + initiator lifecycle (TS constructor).
    pub fn install(ctx: &Context) -> Arc<Self> {
        let registry = Arc::new(Self {
            ctx: ctx.clone(),
            store: Arc::new(Mutex::new(HashMap::new())),
            factory: Arc::new(Mutex::new(None)),
            initiator_state: Mutex::new(InitiatorState::Active),
            initiator_gate: Mutex::new(()),
            active_initiator_runs: AtomicU32::new(0),
            initiator_drain: Mutex::new(None),
            initiator_disposal: Arc::new(tokio::sync::OnceCell::new()),
        });
        ctx.register_service(registry.clone());

        // typert lookups: agent + host-context registrations.
        let registry_for_inject = Arc::clone(&registry);
        ctx.inject(
            InjectSpec::new(["typert"]),
            Arc::new(move |type_ctx: &Context, _config: ArcValue| {
                let registry = Arc::clone(&registry_for_inject);
                let type_ctx = type_ctx.clone();
                Box::pin(async move {
                    if let Some(typert) = type_ctx.get_typed::<Arc<TypertService>>("typert", false)
                    {
                        let lookup_registry = Arc::clone(&registry);
                        let lookup_disposer = typert.lookups.register(
                            "agent",
                            TypertLookup {
                                key: "agent".to_string(),
                                parameter: "agent".to_string(),
                                wire: "agentId".to_string(),
                                host_type_symbol: "@deepseek-ai/dsh-agent#Agent".to_string(),
                                wire_type_symbol: "@deepseek-ai/dsh-session/types#SessionId"
                                    .to_string(),
                                resolve: Arc::new(move |id| {
                                    lookup_registry.get(&session_id(id)).map(arc)
                                }),
                            },
                        );
                        let host_registry = Arc::clone(&registry);
                        let host_disposer = typert.host_contexts.register(
                            "agent",
                            TypertLookup {
                                key: "agent".to_string(),
                                parameter: "agent".to_string(),
                                wire: "agentId".to_string(),
                                host_type_symbol: "@deepseek-ai/dsh-agent#Agent".to_string(),
                                wire_type_symbol: "@deepseek-ai/dsh-session/types#SessionId"
                                    .to_string(),
                                resolve: Arc::new(move |id| {
                                    host_registry
                                        .get(&session_id(id))
                                        .map(|agent| arc(agent.ctx().clone()))
                                }),
                            },
                        );
                        let _ = type_ctx.effect(
                            "typert lookups agent",
                            Box::pin(async move {
                                Some(make_disposer(move || {
                                    let lookup_disposer = lookup_disposer.clone();
                                    let host_disposer = host_disposer.clone();
                                    Box::pin(async move {
                                        lookup_disposer().await;
                                        host_disposer().await;
                                    })
                                }))
                            }),
                        );
                    }
                    Ok(())
                })
            }),
        );

        // The `ctx.agent` DX accessor resolves the nearest live Agent whose
        // scope encloses the calling context. Root contexts remain undefined.
        let accessor_registry = Arc::clone(&registry);
        let _ = ctx.accessor(
            "agent",
            Arc::new(cordis::Accessor {
                get: Arc::new(move |caller| {
                    let chain = scope_chain_of(scope_of(caller).as_ref());
                    let store = accessor_registry.store.lock();
                    chain
                        .iter()
                        .find_map(|scope| {
                            store
                                .values()
                                .find(|entry| entry.agent.scope_key() == scope)
                                .map(|entry| arc(entry.agent.clone()))
                        })
                        .unwrap_or_else(|| arc(()))
                }),
                set: None,
            }),
        );

        // internal/status: close initiators when a lifecycle ancestor fiber
        // unloads.
        let registry_for_status = Arc::clone(&registry);
        let status_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
            let registry = Arc::clone(&registry_for_status);
            Box::pin(async move {
                let Some(fiber) = args
                    .first()
                    .and_then(cordis::util::downcast_arc::<cordis::FiberCore>)
                else {
                    return None;
                };
                if fiber.state() == FiberState::Unloading && registry.has_lifecycle_ancestor(&fiber)
                {
                    registry.close_initiators();
                }
                None
            })
        });
        let registry_ctx = registry.ctx.clone();
        let lifecycle_effect = {
            let registry = Arc::clone(&registry);
            let registry_for_drain = Arc::clone(&registry);
            Box::pin(async move {
                Some(make_disposer(move || {
                    let registry = Arc::clone(&registry_for_drain);
                    Box::pin(async move {
                        registry.dispose_initiators().await;
                    })
                }))
            })
        };
        let _ = registry_ctx.effect("agents.initiatorLifecycle()", lifecycle_effect);
        let status_registry = registry.clone();
        futures::executor::block_on(status_registry.ctx.on(
            "internal/status",
            status_listener,
            EventOptions::default().global(true),
        ));
        registry
    }

    /// Read the Agent that initiated the inherited asynchronous driver
    /// chain, or `None` outside an initiator boundary.
    pub fn current_initiator(&self) -> Result<Option<Arc<dyn Agent>>, String> {
        self.assert_initiators_readable()?;
        Ok(AMBIENT_INITIATOR
            .try_with(|slot| slot.clone())
            .unwrap_or(None))
    }

    /// Read the initiating Agent and fail when no initiator boundary is
    /// active.
    pub fn require_initiator(&self) -> Result<Arc<dyn Agent>, String> {
        match self.current_initiator()? {
            Some(agent) => Ok(agent),
            None => Err(NO_INITIATOR_MESSAGE.to_string()),
        }
    }

    /// Run an operation with one exact Agent as its process-local initiator
    /// (async form of TS `withInitiator`; see the module notes).
    pub async fn with_initiator<T>(
        &self,
        agent: Arc<dyn Agent>,
        operation: impl Future<Output = T>,
    ) -> Result<T, String> {
        self.run_with_initiator(Some(agent), operation).await
    }

    /// Run an operation inside a boundary that hides any inherited
    /// initiating Agent.
    pub async fn without_initiator<T>(
        &self,
        operation: impl Future<Output = T>,
    ) -> Result<T, String> {
        self.run_with_initiator(None, operation).await
    }

    async fn run_with_initiator<T>(
        &self,
        agent: Option<Arc<dyn Agent>>,
        operation: impl Future<Output = T>,
    ) -> Result<T, String> {
        let run = {
            let _gate = self.initiator_gate.lock();
            if *self.initiator_state.lock() != InitiatorState::Active {
                return Err(DISPOSED_INITIATOR_MESSAGE.to_string());
            }
            let run = Arc::new(InitiatorRun {
                active: std::sync::atomic::AtomicBool::new(true),
                parent: AMBIENT_INITIATOR_RUN
                    .try_with(|slot| slot.clone())
                    .unwrap_or(None),
            });
            self.active_initiator_runs.fetch_add(1, Ordering::SeqCst);
            run
        };
        let result = AMBIENT_INITIATOR_RUN
            .scope(Some(run.clone()), AMBIENT_INITIATOR.scope(agent, operation))
            .await;
        self.release_initiator_run(&run);
        Ok(result)
    }

    /// Register the agent-creation factory (TS `setFactory`). The duplicate
    /// check is synchronous (the TS effect body runs synchronously); the
    /// slot is cleared when the calling fiber unloads.
    pub fn set_factory(&self, factory: Arc<dyn AgentFactory>) -> Disposer {
        {
            let mut slot = self.factory.lock();
            if slot.is_some() {
                panic!("an agent factory is already registered");
            }
            *slot = Some(factory);
        }
        let slot = self.factory.clone();
        self.ctx.effect(
            "agents.setFactory()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let slot = slot.clone();
                    Box::pin(async move {
                        *slot.lock() = None;
                    })
                }))
            }),
        )
    }

    /// Return the active creation factory.
    fn require_factory(&self) -> Result<Arc<dyn AgentFactory>, String> {
        self.factory
            .lock()
            .clone()
            .ok_or_else(|| NO_FACTORY_MESSAGE.to_string())
    }

    /// Create and publish a new agent through the registered factory
    /// (TS `AgentRegistry.create`).
    pub async fn create(&self, options: CreateAgentOptions) -> Result<AgentHandle, String> {
        self.create_with_context(&self.ctx, options).await
    }

    /// Preserve the caller's creator identity when a child is constructed
    /// through a shared registry service. Root `create` remains unowned.
    pub async fn create_with_context(
        &self,
        owner_ctx: &Context,
        options: CreateAgentOptions,
    ) -> Result<AgentHandle, String> {
        let factory = self.require_factory()?;
        factory.create_agent(owner_ctx, options).await
    }

    /// Load a persisted session and resume an agent on it through the
    /// registered factory (TS `AgentRegistry.resume`).
    pub async fn resume(&self, options: ResumeAgentOptions) -> Result<AgentHandle, String> {
        self.resume_with_context(&self.ctx, options).await
    }

    /// Resume a child with the exact live creator context without changing
    /// the durable session identity or its delegated permission policy.
    pub async fn resume_with_context(
        &self,
        owner_ctx: &Context,
        options: ResumeAgentOptions,
    ) -> Result<AgentHandle, String> {
        let factory = self.require_factory()?;
        factory.resume(owner_ctx, options).await
    }

    /// Register a live agent (TS `AgentRegistry.register`). The effect is
    /// owned by the CALLER's fiber (the TS `this.ctx` rebinding contract).
    /// A failing enter/announce veto panics the effect body (the port's
    /// async effect-body deviation; the factory path uses
    /// [`AgentRegistry::enter`] + [`AgentRegistry::announce`] directly for
    /// propagating vetoes).
    pub fn register(&self, caller: &Context, agent: Arc<dyn Agent>) -> Disposer {
        let registry = self.ctx.clone();
        caller.effect(
            "agents.register()",
            Box::pin(async move {
                let registry = registry
                    .get_typed::<Arc<AgentRegistry>>("agents", false)
                    .expect("agents service");
                let detach = registry
                    .enter(agent.clone(), None)
                    .unwrap_or_else(|error| panic!("{error}"));
                registry
                    .announce(&agent)
                    .await
                    .unwrap_or_else(|error| panic!("{error}"));
                Some(detach)
            }),
        )
    }

    /// Insert an already-constructed agent without announcing it
    /// (TS `AgentRegistry.enter`).
    pub fn enter(
        &self,
        agent: Arc<dyn Agent>,
        owner: Option<Arc<dyn Agent>>,
    ) -> Result<Disposer, String> {
        let id = agent.id().clone();
        if id != *agent.session().id() {
            return Err(format!(
                "agent id \"{}\" does not match session id \"{}\"",
                id.as_str(),
                agent.session().id().as_str()
            ));
        }
        let carrier = scope_target(None, Some(agent.scope_key().clone()));
        let store_map = self.store.clone();
        let detach_fn: Arc<dyn Fn(&Arc<AgentEntry>) + Send + Sync> = Arc::new(move |entry| {
            entry.set_detach_requested(false);
            {
                let mut store = store_map.lock();
                let is_current = store
                    .get(entry.id.as_str())
                    .is_some_and(|live| Arc::ptr_eq(live, entry));
                if !is_current {
                    return;
                }
                store.remove(entry.id.as_str());
            }
            if !entry.is_announced() {
                return;
            }
            emit_disposed(entry);
        });

        let entry = Arc::new(AgentEntry {
            id: id.clone(),
            agent: agent.clone(),
            owner,
            carrier,
            emit_ctx: self.ctx.clone(),
            flags: Mutex::new(EntryFlags::default()),
            detach: detach_fn,
        });
        {
            let mut store = self.store.lock();
            if store.contains_key(id.as_str()) {
                return Err(format!("agent \"{}\" is already registered", id.as_str()));
            }
            store.insert(id.as_str().to_string(), Arc::clone(&entry));
        }

        let entered = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let detach: Disposer = make_disposer(move || {
            let entry = Arc::clone(&entry);
            let entered = Arc::clone(&entered);
            Box::pin(async move {
                if !entered.swap(false, Ordering::SeqCst) {
                    return;
                }
                if entry.request_detach_if_announcing() {
                    return;
                }
                entry.detach_now();
            })
        });
        Ok(detach)
    }

    /// Announce an agent previously inserted with `enter`
    /// (TS `AgentRegistry.announce`).
    pub async fn announce(&self, agent: &Arc<dyn Agent>) -> Result<(), String> {
        let entry = self.live_entry_for(agent)?;
        if !entry.begin_announce() {
            return Err(format!(
                "agent \"{}\" was already announced",
                entry.id.as_str()
            ));
        }
        // Mark before dispatch so a listener cannot recursively create a
        // second lifecycle edge; detach still pairs a partially delivered
        // first edge.

        let dispatch_ctx = self.ctx.with_filter(entry.carrier.filter.clone());
        let payload = arc(AgentLifecyclePayload {
            agent: entry.agent.clone(),
        });
        let listeners = dispatch_ctx.events.collect(
            DispatchMode::Emit,
            Some(&dispatch_ctx),
            "agent/created",
            std::slice::from_ref(&payload),
        );
        let mut veto: Option<String> = None;
        for (listener_ctx, callback) in &listeners {
            let future = callback(listener_ctx, vec![payload.clone()]);
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                futures::executor::block_on(future)
            })) {
                Ok(_) => {}
                Err(error) => {
                    veto = Some(render_panic(error));
                    break;
                }
            }
        }
        if entry.finish_announce() {
            entry.detach_now();
        }
        match veto {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    /// Return the exact live entry for a live agent handle.
    fn live_entry_for(&self, agent: &Arc<dyn Agent>) -> Result<Arc<AgentEntry>, String> {
        let entry = self
            .store
            .lock()
            .get(agent.id().as_str())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "agent \"{}\" is not live in this registry",
                    agent.id().as_str()
                )
            })?;
        if !Arc::ptr_eq(&entry.agent, agent) {
            return Err(format!(
                "agent \"{}\" is not live in this registry",
                agent.id().as_str()
            ));
        }
        Ok(entry)
    }

    /// Look up a live agent.
    pub fn get(&self, id: &SessionId) -> Option<Arc<dyn Agent>> {
        self.store
            .lock()
            .get(id.as_str())
            .map(|entry| entry.agent.clone())
    }

    /// Test whether a live agent was created through one exact parent
    /// agent's scoped context.
    pub fn is_owned_by(&self, id: &SessionId, owner: &Arc<dyn Agent>) -> bool {
        self.store
            .lock()
            .get(id.as_str())
            .and_then(|entry| entry.owner.clone())
            .is_some_and(|entry_owner| Arc::ptr_eq(&entry_owner, owner))
    }

    /// All live agents, in registration order.
    pub fn list(&self) -> Vec<Arc<dyn Agent>> {
        self.store
            .lock()
            .values()
            .map(|entry| entry.agent.clone())
            .collect()
    }

    /// All live top-level agents in registration order.
    pub fn roots(&self) -> Vec<Arc<dyn Agent>> {
        self.store
            .lock()
            .values()
            .filter(|entry| entry.owner.is_none())
            .map(|entry| entry.agent.clone())
            .collect()
    }

    /// Reject new initiator boundaries while inherited continuations drain.
    fn close_initiators(&self) {
        let mut state = self.initiator_state.lock();
        if *state == InitiatorState::Active {
            *state = InitiatorState::Closing;
        }
    }

    /// Wait for returned-future boundaries, then invalidate retained
    /// references (TS `disposeInitiators`).
    async fn dispose_initiators(&self) {
        let _ = self
            .initiator_disposal
            .get_or_init(|| async {
                {
                    let _gate = self.initiator_gate.lock();
                    self.close_initiators();
                }
                self.release_reentrant_initiator_runs();
                let receiver = {
                    let _gate = self.initiator_gate.lock();
                    if self.active_initiator_runs.load(Ordering::SeqCst) != 0 {
                        let (sender, receiver) = tokio::sync::oneshot::channel();
                        *self.initiator_drain.lock() = Some(sender);
                        Some(receiver)
                    } else {
                        None
                    }
                };
                if let Some(receiver) = receiver {
                    let _ = receiver.await;
                }
                *self.initiator_state.lock() = InitiatorState::Disposed;
            })
            .await;
    }

    fn release_reentrant_initiator_runs(&self) {
        let mut run = AMBIENT_INITIATOR_RUN
            .try_with(|slot| slot.clone())
            .unwrap_or(None);
        while let Some(current) = run {
            self.release_initiator_run(&current);
            run = current.parent.clone();
        }
    }

    fn release_initiator_run(&self, run: &Arc<InitiatorRun>) {
        if !run.active.swap(false, Ordering::SeqCst) {
            return;
        }
        let _gate = self.initiator_gate.lock();
        let remaining = self.active_initiator_runs.fetch_sub(1, Ordering::SeqCst);
        if remaining > 1 {
            return;
        }
        // Last run released: resolve the drain.
        if let Some(sender) = self.initiator_drain.lock().take() {
            let _ = sender.send(());
        }
    }

    /// Whether one unloading fiber owns this service's lifecycle.
    fn has_lifecycle_ancestor(&self, candidate: &Arc<cordis::FiberCore>) -> bool {
        let mut fiber = self.ctx.fiber.clone();
        loop {
            if Arc::ptr_eq(&fiber, candidate) {
                return true;
            }
            let parent = { fiber.parent.lock().clone() };
            match parent {
                None => return false,
                Some(ctx) => {
                    let parent_fiber = ctx.fiber.clone();
                    if Arc::ptr_eq(&parent_fiber, &fiber) {
                        return false;
                    }
                    fiber = parent_fiber;
                }
            }
        }
    }

    fn assert_initiators_readable(&self) -> Result<(), String> {
        if *self.initiator_state.lock() == InitiatorState::Disposed {
            return Err(DISPOSED_INITIATOR_MESSAGE.to_string());
        }
        Ok(())
    }
}

/// Emit the paired disposal edge through the entry's stable carrier
/// (TS `AgentRegistry.emitDisposed`).
fn emit_disposed(entry: &Arc<AgentEntry>) {
    // Run inline via the containing task (emit_disposed is invoked from the
    // detach disposer future, so an ambient runtime exists).
    let dispatch_ctx = entry.emit_ctx.with_filter(entry.carrier.filter.clone());
    let payload = arc(AgentLifecyclePayload {
        agent: entry.agent.clone(),
    });
    let listeners = dispatch_ctx.events.collect(
        DispatchMode::Emit,
        Some(&dispatch_ctx),
        "agent/disposed",
        std::slice::from_ref(&payload),
    );
    let logger = entry.emit_ctx.named_logger(Some("agents"));
    for (listener_ctx, callback) in &listeners {
        let future = callback(listener_ctx, vec![payload.clone()]);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures::executor::block_on(future)
        })) {
            Ok(_) => {}
            Err(error) => {
                logger.warn(vec![arc(format!(
                    "agent \"{}\": agent/disposed listener threw: {}",
                    entry.id.as_str(),
                    render_panic(error)
                ))]);
            }
        }
    }
}

impl Service for AgentRegistry {
    fn service_name(&self) -> &'static str {
        "agents"
    }
}
