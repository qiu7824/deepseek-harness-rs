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
    fn is_announced(&self) -> bool {
        self.flags.lock().announced
    }

    fn is_announcing(&self) -> bool {
        self.flags.lock().announcing
    }

    fn is_detach_requested(&self) -> bool {
        self.flags.lock().detach_requested
    }

    fn set_announced(&self, value: bool) {
        self.flags.lock().announced = value;
    }

    fn set_announcing(&self, value: bool) {
        self.flags.lock().announcing = value;
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
    active_initiator_runs: AtomicU32,
    initiator_drain: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    initiator_disposal: Arc<tokio::sync::OnceCell<()>>,
}

impl AgentRegistry {
    /// Create the registry, register it as the `agents` service, and wire
    /// the typert lookups + initiator lifecycle (TS constructor).
    pub fn install(ctx: &Context) -> Arc<Self> {
        let registry = Arc::new(Self {
            ctx: ctx.clone(),
            store: Arc::new(Mutex::new(HashMap::new())),
            factory: Arc::new(Mutex::new(None)),
            initiator_state: Mutex::new(InitiatorState::Active),
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
                                    lookup_registry.get(&session_id(id)).map(|agent| arc(agent))
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
                    .and_then(|value| cordis::util::downcast_arc::<cordis::FiberCore>(value))
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
        let owner_ctx = self.ctx.clone();
        let factory = self.require_factory()?;
        factory.create_agent(&owner_ctx, options).await
    }

    /// Load a persisted session and resume an agent on it through the
    /// registered factory (TS `AgentRegistry.resume`).
    pub async fn resume(&self, options: ResumeAgentOptions) -> Result<AgentHandle, String> {
        let owner_ctx = self.ctx.clone();
        let factory = self.require_factory()?;
        factory.resume(&owner_ctx, options).await
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
        {
            let store = self.store.lock();
            if store.contains_key(id.as_str()) {
                return Err(format!("agent \"{}\" is already registered", id.as_str()));
            }
        }

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
        self.store
            .lock()
            .insert(id.as_str().to_string(), Arc::clone(&entry));

        let entered = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let detach: Disposer = make_disposer(move || {
            let entry = Arc::clone(&entry);
            let entered = Arc::clone(&entered);
            Box::pin(async move {
                if !entered.swap(false, Ordering::SeqCst) {
                    return;
                }
                if entry.is_announcing() {
                    entry.set_detach_requested(true);
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
        if entry.is_announced() || entry.is_announcing() {
            return Err(format!(
                "agent \"{}\" was already announced",
                entry.id.as_str()
            ));
        }
        // Mark before dispatch so a listener cannot recursively create a
        // second lifecycle edge; detach still pairs a partially delivered
        // first edge.
        entry.set_announcing(true);
        entry.set_announced(true);
        let dispatch_ctx = self.ctx.with_filter(entry.carrier.filter.clone());
        let payload = arc(AgentLifecyclePayload {
            agent: entry.agent.clone(),
        });
        let listeners = dispatch_ctx.events.collect(
            DispatchMode::Emit,
            Some(&dispatch_ctx),
            "agent/created",
            &[payload.clone()],
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
        entry.set_announcing(false);
        if entry.is_detach_requested() {
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
                self.close_initiators();
                self.release_reentrant_initiator_runs();
                if self.active_initiator_runs.load(Ordering::SeqCst) != 0 {
                    let (sender, receiver) = tokio::sync::oneshot::channel();
                    *self.initiator_drain.lock() = Some(sender);
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
        &[payload.clone()],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::InboxNotifications;
    use crate::runtime_types::{AgentOptions, AgentStatusPayload, CancelOptions};
    use cordis::downcast;
    use dsh_llm::{ContentBlock, MessageSource, Role};
    use dsh_scope::ScopeKey;
    use dsh_session::{AgentCancelCause, Session};
    use std::sync::atomic::{AtomicU32, Ordering as MemOrder};

    /// A minimal live agent for registry tests.
    struct TestAgent {
        id: dsh_session::SessionId,
        options: AgentOptions,
        session: Session,
        inbox: crate::Inbox,
        status: parking_lot::Mutex<crate::AgentStatus>,
        ctx: Context,
        scope_key: ScopeKey,
        cancels: parking_lot::Mutex<Vec<AgentCancelCause>>,
        sends: parking_lot::Mutex<Vec<(String, crate::InboxTarget, bool)>>,
    }

    impl crate::Agent for TestAgent {
        fn id(&self) -> &dsh_session::SessionId {
            &self.id
        }

        fn options(&self) -> &AgentOptions {
            &self.options
        }

        fn session(&self) -> &Session {
            &self.session
        }

        fn inbox(&self) -> &crate::Inbox {
            &self.inbox
        }

        fn status(&self) -> crate::AgentStatus {
            *self.status.lock()
        }

        fn ctx(&self) -> &Context {
            &self.ctx
        }

        fn scope_key(&self) -> &ScopeKey {
            &self.scope_key
        }

        fn cancel(&self, cause: AgentCancelCause, _options: Option<&CancelOptions>) {
            self.cancels.lock().push(cause);
        }

        fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
            Box::pin(async {})
        }

        fn run_maintenance(
            &self,
            task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
        ) -> cordis::BoxFuture<'static, ()> {
            task()
        }

        fn send(
            &self,
            message: dsh_session::UserMessage,
            target: crate::InboxTarget,
            wakeup: bool,
        ) {
            self.sends
                .lock()
                .push((message.id.as_str().to_string(), target, wakeup));
        }

        fn followup(&self, message: dsh_session::UserMessage) {
            self.send(message, crate::InboxTarget::NextTurn, true);
        }

        fn steer(&self, message: dsh_session::UserMessage) {
            self.send(message, crate::InboxTarget::NextStep, true);
        }

        fn inject(&self, message: dsh_session::UserMessage) {
            self.send(message, crate::InboxTarget::NextStep, false);
        }
    }

    fn test_agent(ctx: &Context, id: &str) -> Arc<TestAgent> {
        let session = Session::create(dsh_session::session_id(id), None, None).unwrap();
        let inbox = crate::Inbox::new(&session, InboxNotifications::default()).unwrap();
        Arc::new(TestAgent {
            id: dsh_session::session_id(id),
            options: AgentOptions::default(),
            session,
            inbox,
            status: parking_lot::Mutex::new(crate::AgentStatus::Idle),
            ctx: ctx.clone(),
            scope_key: ScopeKey::new(),
            cancels: parking_lot::Mutex::new(Vec::new()),
            sends: parking_lot::Mutex::new(Vec::new()),
        })
    }

    fn user_message(id: &str) -> dsh_session::UserMessage {
        dsh_llm::Message {
            id: dsh_llm::message_id(id),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: id.to_string(),
            }],
            source: MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn install_exposes_service_and_typert_lookups() {
        let ctx = Context::root();
        let _ = dsh_typert_protocol::TypertService::install(&ctx);
        let registry = AgentRegistry::install(&ctx);
        let read: Option<Arc<Arc<AgentRegistry>>> = ctx.get_typed("agents", false);
        assert!(read.is_some());
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        // typert lookups registered (inject fiber settles after typert).
        let typert: Arc<Arc<dsh_typert_protocol::TypertService>> =
            ctx.get_typed("typert", false).unwrap();
        assert_eq!(typert.lookups.keys(), vec!["agent"]);
        assert_eq!(typert.host_contexts.keys(), vec!["agent"]);

        let agent: Arc<dyn crate::Agent> = test_agent(&ctx, "a1");
        let detach = registry.enter(agent.clone(), None).unwrap();
        let resolved = typert.lookups.resolve("agent", "a1");
        assert!(resolved.is_some(), "lookup resolves live agents");
        let host = typert.host_contexts.resolve("agent", "a1");
        assert!(host.is_some(), "host context resolves live agent ctx");
        detach().await;
        assert!(typert.lookups.resolve("agent", "a1").is_none());
        assert_eq!(registry.list().len(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn enter_announce_detach_lifecycle() {
        let ctx = Context::root();
        let registry = AgentRegistry::install(&ctx);
        let created = Arc::new(AtomicU32::new(0));
        let created_listener = created.clone();
        ctx.on(
            "agent/created",
            Arc::new(move |_ctx, args| {
                let created = created_listener.clone();
                Box::pin(async move {
                    let payload = downcast::<AgentLifecyclePayload>(&args[0]).expect("payload");
                    assert_eq!(payload.agent.id().as_str(), "a1");
                    created.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        let disposed = Arc::new(AtomicU32::new(0));
        let disposed_listener = disposed.clone();
        ctx.on(
            "agent/disposed",
            Arc::new(move |_ctx, _args| {
                let disposed = disposed_listener.clone();
                Box::pin(async move {
                    disposed.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;

        let agent: Arc<dyn crate::Agent> = test_agent(&ctx, "a1");
        let detach = registry.enter(agent.clone(), None).unwrap();
        assert_eq!(registry.get(&dsh_session::session_id("a1")).is_some(), true);
        assert_eq!(created.load(MemOrder::SeqCst), 0, "enter does not announce");

        registry.announce(&agent).await.unwrap();
        assert_eq!(created.load(MemOrder::SeqCst), 1);
        // Re-announcing rejects.
        assert!(
            registry
                .announce(&agent)
                .await
                .unwrap_err()
                .contains("already announced")
        );

        detach().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(registry.get(&dsh_session::session_id("a1")).is_none());
        assert_eq!(disposed.load(MemOrder::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn created_veto_rolls_back_and_pairs_disposal() {
        let ctx = Context::root();
        let registry = AgentRegistry::install(&ctx);
        let disposed = Arc::new(AtomicU32::new(0));
        let disposed_listener = disposed.clone();
        ctx.on(
            "agent/disposed",
            Arc::new(move |_ctx, _args| {
                let disposed = disposed_listener.clone();
                Box::pin(async move {
                    disposed.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        ctx.on(
            "agent/created",
            Arc::new(|_ctx, _args| {
                Box::pin(async move {
                    panic!("veto");
                })
            }),
            EventOptions::default(),
        )
        .await;

        let agent: Arc<dyn crate::Agent> = test_agent(&ctx, "a1");
        let detach = registry.enter(agent.clone(), None).unwrap();
        let error = registry.announce(&agent).await.unwrap_err();
        assert_eq!(error, "veto");
        // The veto throw itself does not detach (TS: the rollback comes from
        // the caller's effect chain running the yielded disposer); the entry
        // stays live until the owner detaches.
        assert!(registry.get(&dsh_session::session_id("a1")).is_some());
        detach().await;
        assert!(registry.get(&dsh_session::session_id("a1")).is_none());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(disposed.load(MemOrder::SeqCst), 1, "paired disposal edge");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_ids_and_mismatched_session_reject() {
        let ctx = Context::root();
        let registry = AgentRegistry::install(&ctx);
        let agent = test_agent(&ctx, "a1");
        let detach = registry.enter(agent, None).unwrap();
        let other = test_agent(&ctx, "a1");
        let error = registry
            .enter(other, None)
            .err()
            .expect("duplicate rejects");
        assert!(error.contains("already registered"), "{error}");
        detach().await;

        // mismatched session identity rejects
        let session = Session::create(dsh_session::session_id("other"), None, None).unwrap();
        let inbox = crate::Inbox::new(&session, InboxNotifications::default()).unwrap();
        let mismatched: Arc<TestAgent> = Arc::new(TestAgent {
            id: dsh_session::session_id("a2"),
            options: AgentOptions::default(),
            session,
            inbox,
            status: parking_lot::Mutex::new(crate::AgentStatus::Idle),
            ctx: ctx.clone(),
            scope_key: ScopeKey::new(),
            cancels: parking_lot::Mutex::new(Vec::new()),
            sends: parking_lot::Mutex::new(Vec::new()),
        });
        let error = registry
            .enter(mismatched, None)
            .err()
            .expect("mismatch rejects");
        assert!(error.contains("does not match session id"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn register_effect_owned_by_calling_fiber() {
        let ctx = Context::root();
        let registry = AgentRegistry::install(&ctx);

        struct Registrar;
        #[async_trait::async_trait]
        impl cordis::Plugin for Registrar {
            fn inject(&self) -> cordis::InjectSpec {
                cordis::InjectSpec::new(["agents"])
            }

            async fn apply(
                &self,
                ctx: &Context,
                _config: cordis::ArcValue,
            ) -> Result<(), cordis::PluginError> {
                let registry: Arc<Arc<AgentRegistry>> = ctx.get_typed("agents", false).unwrap();
                let agent = test_agent(ctx, "fiber-agent");
                registry.register(ctx, agent);
                Ok(())
            }
        }

        let fiber = ctx.plugin(Arc::new(Registrar), cordis::arc(()));
        fiber.settle().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            registry
                .get(&dsh_session::session_id("fiber-agent"))
                .is_some()
        );

        fiber.dispose().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            registry
                .get(&dsh_session::session_id("fiber-agent"))
                .is_none(),
            "fiber disposal unregisters the agent"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_roots_and_ownership() {
        let ctx = Context::root();
        let registry = AgentRegistry::install(&ctx);
        let parent: Arc<dyn crate::Agent> = test_agent(&ctx, "parent");
        let child: Arc<dyn crate::Agent> = test_agent(&ctx, "child");
        let detach_parent = registry.enter(parent.clone(), None).unwrap();
        let detach_child = registry.enter(child.clone(), Some(parent.clone())).unwrap();
        registry.announce(&parent).await.unwrap();
        registry.announce(&child).await.unwrap();

        assert_eq!(registry.list().len(), 2);
        assert_eq!(registry.roots().len(), 1);
        assert_eq!(registry.roots()[0].id().as_str(), "parent");
        assert!(registry.is_owned_by(&dsh_session::session_id("child"), &parent));
        assert!(!registry.is_owned_by(&dsh_session::session_id("parent"), &child));

        detach_child().await;
        detach_parent().await;
        assert!(registry.list().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_and_resume_delegate_to_factory() {
        struct Factory {
            created: Arc<AtomicU32>,
            resumed: Arc<AtomicU32>,
        }

        #[async_trait::async_trait]
        impl crate::AgentFactory for Factory {
            async fn create_agent(
                &self,
                _owner_ctx: &Context,
                _options: crate::CreateAgentOptions,
            ) -> Result<crate::AgentHandle, String> {
                self.created.fetch_add(1, MemOrder::SeqCst);
                Err("not implemented in test".to_string())
            }

            async fn resume(
                &self,
                _owner_ctx: &Context,
                _options: crate::ResumeAgentOptions,
            ) -> Result<crate::AgentHandle, String> {
                self.resumed.fetch_add(1, MemOrder::SeqCst);
                Err("not implemented in test".to_string())
            }
        }

        let ctx = Context::root();
        let registry = AgentRegistry::install(&ctx);
        // Without a factory, both reject.
        let error = registry
            .create(crate::CreateAgentOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error, NO_FACTORY_MESSAGE);
        let error = registry
            .resume(crate::ResumeAgentOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error, NO_FACTORY_MESSAGE);

        let created = Arc::new(AtomicU32::new(0));
        let resumed = Arc::new(AtomicU32::new(0));
        let dispose = registry.set_factory(Arc::new(Factory {
            created: created.clone(),
            resumed: resumed.clone(),
        }));
        assert!(
            registry
                .create(crate::CreateAgentOptions::default())
                .await
                .unwrap_err()
                .contains("not implemented")
        );
        assert!(
            registry
                .resume(crate::ResumeAgentOptions::default())
                .await
                .unwrap_err()
                .contains("not implemented")
        );
        assert_eq!(created.load(MemOrder::SeqCst), 1);
        assert_eq!(resumed.load(MemOrder::SeqCst), 1);

        // A second factory registration rejects.
        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            registry.set_factory(Arc::new(Factory {
                created: Arc::new(AtomicU32::new(0)),
                resumed: Arc::new(AtomicU32::new(0)),
            }));
        }));
        assert!(duplicate.is_err());

        dispose().await;
        let error = registry
            .create(crate::CreateAgentOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error, NO_FACTORY_MESSAGE, "factory slot cleared on dispose");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn initiator_attribution_flows_and_clears() {
        let ctx = Context::root();
        let registry = AgentRegistry::install(&ctx);
        let agent = test_agent(&ctx, "a1");

        assert!(registry.current_initiator().unwrap().is_none());
        assert!(
            registry
                .require_initiator()
                .err()
                .expect("no initiator boundary")
                .contains("no initiating agent")
        );

        let seen = registry
            .with_initiator(agent.clone(), async {
                registry
                    .current_initiator()
                    .unwrap()
                    .map(|a| a.id().clone())
            })
            .await
            .unwrap();
        assert_eq!(seen.as_ref().map(|id| id.as_str()), Some("a1"));

        // without_initiator hides an inherited initiator.
        let hidden = registry
            .with_initiator(agent.clone(), async {
                registry
                    .without_initiator(async { registry.current_initiator().unwrap() })
                    .await
                    .unwrap()
            })
            .await
            .unwrap();
        assert!(hidden.is_none());

        assert!(
            registry.current_initiator().unwrap().is_none(),
            "boundary unwinds"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn agent_methods_track_calls() {
        let ctx = Context::root();
        let agent = test_agent(&ctx, "a1");

        agent.cancel(AgentCancelCause::User, None);
        let message = user_message("m1");
        agent.followup(message.clone());
        agent.steer(message.clone());
        agent.inject(message.clone());
        agent.send(message.clone(), crate::InboxTarget::NextTurn, true);

        assert_eq!(agent.cancels.lock().len(), 1);
        let sends = agent.sends.lock();
        assert_eq!(sends.len(), 4);
        assert_eq!(sends[0].0, "m1");
        assert_eq!(sends[1].0, "m1");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn agent_emit_contains_synchronous_callback_panics_and_continues() {
        use crate::dispatch::AgentEventDispatch;

        let ctx = Context::root();
        let agent = test_agent(&ctx, "sync-panic");
        let seen = Arc::new(AtomicU32::new(0));
        let seen_listener = seen.clone();
        ctx.on(
            "agent/sync-panic",
            Arc::new(move |_ctx, _args| {
                let seen = seen_listener.clone();
                Box::pin(async move {
                    seen.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        ctx.on(
            "agent/sync-panic",
            Arc::new(
                |_ctx, _args| -> cordis::BoxFuture<'static, Option<ArcValue>> {
                    panic!("sync agent listener panic")
                },
            ),
            EventOptions::default().prepend(true),
        )
        .await;

        AgentEventDispatch::new(&ctx, agent).emit("agent/sync-panic", |_| arc(()));
        assert_eq!(
            seen.load(MemOrder::SeqCst),
            1,
            "a synchronous callback panic must not skip later listeners"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agent_emit_finishes_all_listener_prefixes_before_pending_tails() {
        use crate::dispatch::AgentEventDispatch;

        let ctx = Context::root();
        let agent = test_agent(&ctx, "pending-order");
        let order = Arc::new(parking_lot::Mutex::new(Vec::<&'static str>::new()));
        let first_order = order.clone();
        ctx.on(
            "agent/pending-order",
            Arc::new(move |_ctx, _args| {
                let order = first_order.clone();
                Box::pin(async move {
                    order.lock().push("prefix-1");
                    tokio::task::yield_now().await;
                    order.lock().push("tail-1");
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        let second_order = order.clone();
        ctx.on(
            "agent/pending-order",
            Arc::new(move |_ctx, _args| {
                let order = second_order.clone();
                Box::pin(async move {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    order.lock().push("prefix-2");
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;

        AgentEventDispatch::new(&ctx, agent).emit("agent/pending-order", |_| arc(()));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while order.lock().len() < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending tail must settle");
        assert_eq!(
            &*order.lock(),
            &["prefix-1", "prefix-2", "tail-1"],
            "all synchronous listener prefixes must finish before any pending tail"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn agent_emit_runs_pending_listener_prefix_and_tail_exactly_once() {
        use crate::dispatch::AgentEventDispatch;

        let ctx = Context::root();
        let agent = test_agent(&ctx, "pending-tail");
        let prefix = Arc::new(AtomicU32::new(0));
        let suffix = Arc::new(AtomicU32::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let prefix_listener = prefix.clone();
        let suffix_listener = suffix.clone();
        let release_listener = release.clone();
        ctx.on(
            "agent/pending-tail",
            Arc::new(move |_ctx, _args| {
                let prefix = prefix_listener.clone();
                let suffix = suffix_listener.clone();
                let release = release_listener.clone();
                Box::pin(async move {
                    prefix.fetch_add(1, MemOrder::SeqCst);
                    release.notified().await;
                    suffix.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;

        AgentEventDispatch::new(&ctx, agent).emit("agent/pending-tail", |_| arc(()));
        assert_eq!(prefix.load(MemOrder::SeqCst), 1);
        assert_eq!(suffix.load(MemOrder::SeqCst), 0);
        release.notify_waiters();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while suffix.load(MemOrder::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending listener tail must resume");
        assert_eq!(prefix.load(MemOrder::SeqCst), 1);
        assert_eq!(suffix.load(MemOrder::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn agent_emit_delivers_reentrant_synchronous_notifications_inline() {
        use crate::dispatch::AgentEventDispatch;

        let ctx = Context::root();
        let agent = test_agent(&ctx, "reentrant");
        let inner_seen = Arc::new(AtomicU32::new(0));
        let inner_seen_listener = inner_seen.clone();
        ctx.on(
            "agent/reentrant-inner",
            Arc::new(move |_ctx, _args| {
                let inner_seen = inner_seen_listener.clone();
                Box::pin(async move {
                    inner_seen.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;

        let nested = AgentEventDispatch::new(&ctx, agent.clone());
        ctx.on(
            "agent/reentrant-outer",
            Arc::new(move |_ctx, _args| {
                let nested = nested.clone();
                Box::pin(async move {
                    nested.emit("agent/reentrant-inner", |_| arc(()));
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;

        AgentEventDispatch::new(&ctx, agent).emit("agent/reentrant-outer", |_| arc(()));
        assert_eq!(
            inner_seen.load(MemOrder::SeqCst),
            1,
            "the nested notification's synchronous prefix must not be lost"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn status_events_reach_scoped_listeners() {
        use crate::dispatch::AgentEventDispatch;
        let ctx = Context::root();
        let agent = test_agent(&ctx, "a1");
        let dispatch = AgentEventDispatch::new(&ctx, agent.clone());
        let seen = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let seen_listener = seen.clone();
        ctx.on(
            "agent/status",
            Arc::new(move |_ctx, args| {
                let seen = seen_listener.clone();
                Box::pin(async move {
                    let payload = downcast::<AgentStatusPayload>(&args[0]).expect("payload");
                    seen.lock().push(format!(
                        "{}:{}",
                        payload.agent.id().as_str(),
                        payload.status.as_str()
                    ));
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;
        dispatch.emit("agent/status", |agent| {
            arc(AgentStatusPayload {
                agent: agent.clone(),
                status: crate::AgentStatus::Running,
            })
        });
        assert_eq!(&*seen.lock(), &["a1:running"]);
    }
}
