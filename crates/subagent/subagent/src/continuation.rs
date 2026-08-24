//! Internal continuable-subagent manager: stable child ids, descriptor
//! persistence, activation admission, the live ownership graph, and
//! settlement delivery to the parent, behind `ctx.subagents`. Rust port of
//! `packages/subagent/subagent/src/continuation.ts`.
//!
//! # Deviations
//!
//! - Cold resume rejects with `NOT_RESUMABLE`: the Rust agent loop's resume
//!   path is not wired to a persistence backend yet.
//! - The activation setup registry is not ported: materialization composes
//!   policy + persona/restriction without deployment contributions.
//! - `agent/inbox/claimed`/`discarded` accounting is not wired (the Rust
//!   inbox publishes those payloads through the agent scope; the accepted
//!   set drains when the watcher observes quiescence instead).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::child_agent::{
    ChildComposition, DelegatedPolicyOverrides, append_delegated_policy_overrides,
    apply_child_composition, capture_delegated_policy_overrides, child_session_meta,
    resolve_child_agent_options, resolve_child_depth,
};
use crate::descriptor::{SubagentDescriptorData, snapshot_subagent_descriptor};
use crate::descriptor_seed::seed_descriptor_turn;
use crate::error::SubagentError;
use crate::lifecycle::ActivationObserver;
use crate::types::{
    ContinuableCreateRequest, ContinuableCreateSpec, SubagentStartRequest, SubagentStopReason,
};
use cordis::Context;
use dsh_agent::{Agent, AgentHandle, AgentRegistry};
use dsh_llm::{ContentBlock, MessageId, MessageSource, bound_context_summary, create_user_message};
use dsh_session::{SessionEvent, SessionId, session_id};
use dsh_session_persistence::SessionPersistenceApi;

/// What a caller asks for when starting a continuable background child.
#[derive(Clone)]
pub struct ContinuableStartSpec {
    pub provider: String,
    pub label: String,
    pub request: SubagentStartRequest,
    pub signal: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Identities returned once a continuable child accepted its initial prompt.
#[derive(Debug, Clone)]
pub struct ContinuableStart {
    pub child_id: SessionId,
    pub message_id: MessageId,
}

/// Options for following up with one continuable child.
#[derive(Clone)]
pub struct SubagentFollowupOptions {
    pub source: MessageSource,
    pub signal: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Options for one continuable child's report to its direct parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentReportDelivery {
    Quiet,
    Wakeup,
}

#[derive(Clone)]
pub struct SubagentReportOptions {
    pub delivery: SubagentReportDelivery,
    pub signal: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Authority under which one interrupt request is admitted.
#[derive(Clone)]
pub enum SubagentInterruptAuthority {
    User { parent_session_id: SessionId },
    Ancestor { agent: Arc<dyn Agent> },
}

/// Hooks the manager needs from the owning service.
pub trait ContinuationHost: Send + Sync + 'static {
    fn prepare_continuable(
        &self,
        name: &str,
        request: ContinuableCreateRequest,
    ) -> cordis::BoxFuture<'static, Result<ContinuableCreateSpec, SubagentError>>;
    fn observe_activation(
        &self,
        provider: &str,
        child_id: &SessionId,
        parent: &Arc<dyn Agent>,
    ) -> ActivationObserver;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivationState {
    Running,
    Waiting,
    Settled,
}

type ActivationDisposal = Arc<tokio::sync::Mutex<Option<Result<(), String>>>>;

struct MaterializeRequest<'a> {
    child_id: &'a SessionId,
    provider: &'a str,
    parent: Arc<dyn Agent>,
    seed: &'a [SessionEvent],
    child_depth: u64,
    lineage_seed_length: usize,
    request: &'a SubagentStartRequest,
    delegated_policies: &'a DelegatedPolicyOverrides,
    signal: &'a Arc<dyn Fn() -> bool + Send + Sync>,
}

/// One residency epoch for a reconstructed continuable child Agent.
struct Activation {
    child_id: SessionId,
    parent_session: SessionId,
    handle: AgentHandle,
    ancestry: HashSet<usize>,
    owned_children: HashSet<String>,
    observer: ActivationObserver,
    disposal: Option<ActivationDisposal>,
    accepted: HashSet<String>,
    announced: bool,
    poke: Arc<tokio::sync::Notify>,
}

impl Activation {
    fn handle(&self) -> &AgentHandle {
        &self.handle
    }
}

/// Serialize each durable child's delivery, release, and disposal.
#[derive(Default)]
struct ChildLock {
    tails: parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ChildLock {
    async fn run<T, F, Fut>(&self, child_id: &SessionId, operation: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let gate = self
            .tails
            .lock()
            .entry(child_id.as_str().to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = gate.lock_owned().await;
        operation().await
    }
}

/// The continuable-subagent orchestration service behind `ctx.subagents`.
pub struct SubagentContinuationManager {
    pub ctx: Context,
    self_arc: std::sync::OnceLock<std::sync::Weak<Self>>,
    activations: parking_lot::Mutex<HashMap<String, Arc<parking_lot::Mutex<Activation>>>>,
    materializations: parking_lot::Mutex<HashMap<u64, HashSet<usize>>>,
    next_materialization: std::sync::atomic::AtomicU64,
    materialization_changed: tokio::sync::Notify,
    locks: ChildLock,
    host: Arc<dyn ContinuationHost>,
    closing_scopes: parking_lot::Mutex<HashMap<usize, HashSet<usize>>>,
    draining: std::sync::atomic::AtomicBool,
}

struct MaterializationGuard {
    manager: std::sync::Weak<SubagentContinuationManager>,
    token: u64,
}

impl Drop for MaterializationGuard {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.upgrade() {
            manager.materializations.lock().remove(&self.token);
            manager.materialization_changed.notify_waiters();
        }
    }
}

impl SubagentContinuationManager {
    fn begin_materialization(&self, parent: &Arc<dyn Agent>) -> MaterializationGuard {
        let token = self
            .next_materialization
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let lineage = self
            .live_lineage(parent)
            .into_iter()
            .map(|agent| Arc::as_ptr(&agent).cast::<()>() as usize)
            .collect();
        self.materializations.lock().insert(token, lineage);
        MaterializationGuard {
            manager: self.self_arc.get().expect("manager weak").clone(),
            token,
        }
    }

    async fn wait_materializations(&self, roots: Option<&HashSet<usize>>) {
        loop {
            let notified = self.materialization_changed.notified();
            let pending = {
                let materializations = self.materializations.lock();
                match roots {
                    None => !materializations.is_empty(),
                    Some(roots) => materializations
                        .values()
                        .any(|lineage| !lineage.is_disjoint(roots)),
                }
            };
            if !pending {
                return;
            }
            notified.await;
        }
    }
    /// Build the manager (TS constructor).
    pub fn new(ctx: &Context, host: Arc<dyn ContinuationHost>) -> Arc<Self> {
        let manager = Arc::new(Self {
            ctx: ctx.clone(),
            self_arc: std::sync::OnceLock::new(),
            activations: parking_lot::Mutex::new(HashMap::new()),
            materializations: parking_lot::Mutex::new(HashMap::new()),
            next_materialization: std::sync::atomic::AtomicU64::new(1),
            materialization_changed: tokio::sync::Notify::new(),
            locks: ChildLock::default(),
            host,
            closing_scopes: parking_lot::Mutex::new(HashMap::new()),
            draining: std::sync::atomic::AtomicBool::new(false),
        });
        // Remove closing scopes when an agent leaves the registry.
        let manager_for_disposed = manager.clone();
        let disposed_listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
            let manager = manager_for_disposed.clone();
            Box::pin(async move {
                if let Some(payload) = args
                    .first()
                    .and_then(|value| cordis::downcast::<dsh_agent::AgentLifecyclePayload>(value))
                {
                    let key = Arc::as_ptr(&payload.agent).cast::<()>() as usize;
                    manager.closing_scopes.lock().remove(&key);
                }
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "agent/disposed",
            disposed_listener,
            Default::default(),
        ));
        for event in ["agent/inbox/claimed", "agent/inbox/discarded"] {
            let manager_for_inbox = manager.clone();
            let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
                let manager = manager_for_inbox.clone();
                let payload = args.first().cloned();
                Box::pin(async move {
                    if let Some(payload) = payload {
                        if let Some(claimed) =
                            payload.downcast_ref::<dsh_agent::AgentInboxClaimedPayload>()
                        {
                            manager.settle_accepted(&claimed.agent, &claimed.message.id);
                        } else if let Some(discarded) =
                            payload.downcast_ref::<dsh_agent::AgentInboxMessagePayload>()
                        {
                            manager.settle_accepted(&discarded.agent, &discarded.message.id);
                        }
                    }
                    None
                })
            });
            let _ = futures::executor::block_on(ctx.on(event, listener, Default::default()));
        }
        manager
            .self_arc
            .set(Arc::downgrade(&manager))
            .expect("once");
        let weak = Arc::downgrade(&manager);
        let _ = ctx.effect(
            "subagents.continuations()",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(manager) = weak.upgrade() {
                            let _ = manager.drain().await;
                        }
                    })
                }))
            }),
        );
        manager
    }

    fn settle_accepted(&self, agent: &Arc<dyn Agent>, message_id: &MessageId) {
        let activation = self.activations.lock().get(agent.id().as_str()).cloned();
        if let Some(activation) = activation {
            let mut activation = activation.lock();
            if Arc::ptr_eq(&activation.handle().agent, agent)
                && activation.accepted.remove(message_id.as_str())
            {
                activation.poke.notify_waiters();
            }
        }
    }

    fn agents(&self) -> Arc<AgentRegistry> {
        self.ctx
            .get_typed::<Arc<AgentRegistry>>("agents", false)
            .map(|slot| slot.as_ref().clone())
            .expect("agents service")
    }

    fn persistence(&self) -> Option<Arc<dyn SessionPersistenceApi>> {
        self.ctx
            .get_typed::<Arc<dyn SessionPersistenceApi>>("sessionPersistence", false)
            .map(|slot| slot.as_ref().clone())
    }

    fn require_persistence(&self) -> Result<Arc<dyn SessionPersistenceApi>, SubagentError> {
        self.persistence().ok_or_else(|| {
            SubagentError::new(
                "PERSISTENCE_UNAVAILABLE",
                "continuable subagents require session persistence (load a dsh-session-persistence backend)",
            )
        })
    }

    /// Start one continuable background child.
    pub async fn start_continuable(
        &self,
        spec: ContinuableStartSpec,
    ) -> Result<ContinuableStart, SubagentError> {
        let request = &spec.request;
        let parent = request.parent.clone();
        self.assert_admitting(parent.as_ref())?;
        self.require_persistence()?;
        crate::depth::assert_subagent_max_depth(request.max_depth)
            .map_err(|message| SubagentError::new("INVALID_MAX_DEPTH", message))?;
        let child_id = session_id(uuid::Uuid::new_v4().to_string());
        let child_depth = resolve_child_depth(parent.as_ref(), request.max_depth)
            .map_err(|error| SubagentError::new("DEPTH_EXCEEDED", error.message))?;
        let agent_provider = request
            .agent_options
            .as_ref()
            .and_then(|options| options.provider.clone())
            .or_else(|| parent.options().provider.clone());
        let agent_model = request
            .agent_options
            .as_ref()
            .and_then(|options| options.model.clone())
            .or_else(|| parent.options().model.clone());
        let descriptor = snapshot_subagent_descriptor(&SubagentDescriptorData::Continuable {
            version: crate::descriptor::SUBAGENT_DESCRIPTOR_VERSION,
            provider: spec.provider.clone(),
            label: spec.label.clone(),
            agent_provider,
            agent_model,
            persona: request.persona.clone(),
            tool_filter: request.tool_filter.clone(),
        })
        .map_err(|message| SubagentError::new("INVALID_DESCRIPTOR", message))?;
        let delegated_policies = capture_delegated_policy_overrides(parent.as_ref());

        let prepared = self
            .host
            .prepare_continuable(
                &spec.provider,
                ContinuableCreateRequest {
                    session_id: child_id.clone(),
                    parent: parent.clone(),
                    signal: spec.signal.clone(),
                },
            )
            .await?;
        if (spec.signal)() {
            return Err(SubagentError::new(
                "CANCELLED",
                "subagent request was aborted",
            ));
        }
        self.assert_admitting(parent.as_ref())?;

        let lineage_seed_length = prepared.seed.as_ref().map(|seed| seed.len()).unwrap_or(0);
        let seed = seed_descriptor_turn(&child_id, prepared.seed.as_deref(), &descriptor)
            .map_err(|error| SubagentError::new("CHILD_SEED_FAILED", error))?;
        let child_id_for_child = child_id.clone();
        let message_id = self
            .locks
            .run(&child_id, || {
                let manager = self;
                let parent = parent.clone();
                let spec = spec.clone();
                async move {
                    let activation = manager
                        .materialize(MaterializeRequest {
                            child_id: &child_id_for_child,
                            provider: &spec.provider,
                            parent: parent.clone(),
                            seed: &seed,
                            child_depth,
                            lineage_seed_length,
                            request,
                            delegated_policies: &delegated_policies,
                            signal: &spec.signal,
                        })
                        .await?;
                    manager
                        .submit_materialized(
                            activation,
                            &spec.request.prompt,
                            MessageSource::User {
                                rpc_id: None,
                                client_time_zone: None,
                            },
                            parent,
                            &spec.signal,
                        )
                        .await
                }
            })
            .await?;
        Ok(ContinuableStart {
            child_id,
            message_id,
        })
    }

    /// Deliver one later message to a known continuable child.
    pub async fn followup(
        &self,
        parent: Arc<dyn Agent>,
        child_id: &SessionId,
        content: &[ContentBlock],
        options: &SubagentFollowupOptions,
    ) -> Result<MessageId, SubagentError> {
        self.assert_admitting(parent.as_ref())?;
        loop {
            let live = self
                .locks
                .run(child_id, || {
                    let manager = self;
                    let parent = parent.clone();
                    let content = content.to_vec();
                    let options = options.clone();
                    async move {
                        let activation = {
                            let activations = manager.activations.lock();
                            activations.get(child_id.as_str()).cloned()
                        };
                        match activation {
                            None => Err(SubagentError::new(
                                "NOT_RESUMABLE",
                                format!("subagent \"{child_id}\" is not resident and cold resume is not ported"),
                            )),
                            Some(activation) => {
                                manager
                                    .submit_admitted(&activation, &content, &options.source, parent, &options.signal)
                                    .map(Some)
                            }
                        }
                    }
                })
                .await;
            match live {
                Ok(Some(message_id)) => return Ok(message_id),
                Ok(None) => {
                    // The disposal cutoff race: retry.
                    self.assert_admitting(parent.as_ref())?;
                    if (options.signal)() {
                        return Err(SubagentError::new(
                            "CANCELLED",
                            "subagent request was aborted",
                        ));
                    }
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Interrupt one live continuable child's current turn.
    pub fn interrupt(
        &self,
        target_session_id: &SessionId,
        authority: &SubagentInterruptAuthority,
    ) -> Result<(), SubagentError> {
        let activations = self.activations.lock();
        let Some(activation) = activations.get(target_session_id.as_str()).cloned() else {
            return Ok(());
        };
        let agent = activation.lock().handle().agent.clone();
        let activation = activation.lock();
        match authority {
            SubagentInterruptAuthority::User { parent_session_id } => {
                if agent.session().header().parent_session.as_ref() != Some(parent_session_id) {
                    return Err(SubagentError::new(
                        "UNAUTHORIZED",
                        format!(
                            "subagent \"{target_session_id}\" belongs to another parent session"
                        ),
                    ));
                }
                agent.cancel(
                    dsh_session::AgentCancelCause::User,
                    Some(&dsh_agent::CancelOptions { keep_inbox: true }),
                );
            }
            SubagentInterruptAuthority::Ancestor { agent: caller } => {
                let caller_key = Arc::as_ptr(caller).cast::<()>() as usize;
                if !activation.ancestry.contains(&caller_key) {
                    return Err(SubagentError::new(
                        "UNAUTHORIZED",
                        format!(
                            "subagent \"{target_session_id}\" is not a live descendant of agent \"{}\"",
                            caller.id().as_str()
                        ),
                    ));
                }
                agent.cancel(
                    dsh_session::AgentCancelCause::Parent,
                    Some(&dsh_agent::CancelOptions { keep_inbox: true }),
                );
            }
        }
        Ok(())
    }

    /// Deliver explicitly selected content from one resident continuable
    /// child to its durable direct parent.
    pub async fn report_from(
        &self,
        child: &Arc<dyn Agent>,
        content: &[ContentBlock],
        options: &SubagentReportOptions,
    ) -> Result<MessageId, SubagentError> {
        if (options.signal)() {
            return Err(SubagentError::new(
                "CANCELLED",
                "subagent request was aborted",
            ));
        }
        self.assert_admitting(child.as_ref())?;
        let activation = {
            let activations = self.activations.lock();
            activations.get(child.id().as_str()).cloned()
        }
        .ok_or_else(|| {
            SubagentError::new(
                "UNAUTHORIZED",
                format!(
                    "agent \"{}\" is not a live continuable subagent and cannot report",
                    child.id().as_str()
                ),
            )
        })?;
        let parent_id = child
            .session()
            .header()
            .parent_session
            .clone()
            .ok_or_else(|| {
                SubagentError::new(
                    "PARENT_UNAVAILABLE",
                    "direct parent is not live; report was not delivered",
                )
            })?;
        let parent = self.agents().get(&parent_id).ok_or_else(|| {
            SubagentError::new(
                "PARENT_UNAVAILABLE",
                "direct parent is not live; report was not delivered",
            )
        })?;
        let mut content_blocks = vec![ContentBlock::Text {
            text: format!(
                "Background subagent {} reported:",
                activation.lock().child_id.as_str()
            ),
        }];
        content_blocks.extend(content.iter().cloned());
        let message = create_user_message(
            content_blocks,
            MessageSource::SubagentReport {
                form: dsh_llm::ContextForm::Relay,
                sender_session_id: activation.lock().child_id.as_str().to_string(),
            },
        );
        let message_id = message.id.clone();
        match options.delivery {
            SubagentReportDelivery::Wakeup => parent.followup(message),
            SubagentReportDelivery::Quiet => parent.inject(message),
        }
        Ok(message_id)
    }

    /// Close admission, await materializations, then dispose the stable live
    /// Activation forest child-first.
    pub async fn drain(&self) -> Result<(), SubagentError> {
        self.draining
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.wait_materializations(None).await;
        let roots = {
            let activations = self.activations.lock();
            let mut owned: HashSet<String> = HashSet::new();
            for activation in activations.values() {
                let activation = activation.lock();
                for child in &activation.owned_children {
                    owned.insert(child.clone());
                }
            }
            activations
                .values()
                .filter(|activation| !owned.contains(activation.lock().child_id.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut failures: Vec<String> = Vec::new();
        for root in roots {
            if let Err(error) = self.dispose(&root).await {
                failures.push(error);
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(SubagentError::new(
                "ACTIVATION_TEARDOWN_FAILED",
                format!(
                    "continuable subagent teardown failed: {}",
                    failures.join("; ")
                ),
            ))
        }
    }

    /// Stop only the continuable descendants of exact live host-owned
    /// parents.
    pub async fn drain_descendants(&self, parents: &[Arc<dyn Agent>]) -> Result<(), SubagentError> {
        let mut roots: HashSet<usize> = HashSet::new();
        for parent in parents {
            if self
                .agents()
                .get(parent.id())
                .is_some_and(|live| Arc::ptr_eq(&live, parent))
            {
                roots.insert(Arc::as_ptr(parent).cast::<()>() as usize);
            }
        }
        if roots.is_empty() {
            return Ok(());
        }
        {
            let mut closing = self.closing_scopes.lock();
            for root in &roots {
                closing.entry(*root).or_default().insert(*root);
            }
        }
        self.wait_materializations(Some(&roots)).await;
        let targets = {
            let activations = self.activations.lock();
            activations
                .values()
                .filter(|activation| {
                    let activation = activation.lock();
                    let agent = activation.handle().agent.clone();
                    let agent_key = Arc::as_ptr(&agent).cast::<()>() as usize;
                    roots
                        .iter()
                        .any(|root| *root != agent_key && activation.ancestry.contains(root))
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut failures: Vec<String> = Vec::new();
        for target in targets {
            if let Err(error) = self.dispose(&target).await {
                failures.push(error);
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(SubagentError::new(
                "ACTIVATION_TEARDOWN_FAILED",
                format!(
                    "continuable subagent teardown failed: {}",
                    failures.join("; ")
                ),
            ))
        }
    }

    /// One admitted materialization: create the child Agent, install the
    /// Activation, and register ownership.
    async fn materialize(
        &self,
        input: MaterializeRequest<'_>,
    ) -> Result<Arc<parking_lot::Mutex<Activation>>, SubagentError> {
        let MaterializeRequest {
            child_id,
            provider,
            parent,
            seed,
            child_depth,
            lineage_seed_length,
            request,
            delegated_policies,
            signal,
        } = input;
        self.assert_admitting(parent.as_ref())?;
        let _materialization = self.begin_materialization(&parent);
        if (signal)() {
            return Err(SubagentError::new(
                "CANCELLED",
                "subagent request was aborted",
            ));
        }
        // Compose the child after publication, before its first turn (the
        // TS creation-window setup deviation, shared with the one-shot
        // driver).
        let handle = {
            let registry = self.agents();
            let parent_for_setup = parent.clone();
            let delegated = delegated_policies.clone();
            let composition = ChildComposition {
                persona: request.persona.clone(),
                tool_filter: request.tool_filter.clone(),
            };
            let registry_for_setup = registry.clone();
            let child_id_for_setup = child_id.clone();
            let handle = registry
                .create(dsh_agent::CreateAgentOptions {
                    session_id: Some(child_id_for_setup),
                    meta: Some(child_session_meta(
                        parent.as_ref(),
                        child_depth,
                        lineage_seed_length as u64,
                    )),
                    seed: Some(seed.to_vec()),
                    agent_options: Some(resolve_child_agent_options(
                        parent.as_ref(),
                        request.agent_options.as_ref(),
                        child_depth,
                    )),
                    setup: None,
                })
                .await
                .map_err(|error| SubagentError::new("CHILD_CREATE_FAILED", error))?;
            let _ = registry_for_setup;
            if let Err(error) =
                append_delegated_policy_overrides(handle.agent.session(), &delegated)
            {
                handle.dispose.await;
                return Err(SubagentError::new("CHILD_COMPOSE_FAILED", error));
            }
            apply_child_composition(handle.agent.ctx(), parent_for_setup.as_ref(), &composition);
            handle
        };
        let lineage = self.live_lineage(&parent);
        let mut ancestry: HashSet<usize> = lineage
            .iter()
            .map(|agent| Arc::as_ptr(agent).cast::<()>() as usize)
            .collect();
        ancestry.insert(Arc::as_ptr(&handle.agent).cast::<()>() as usize);
        let observer = self.host.observe_activation(provider, child_id, &parent);
        let activation = Arc::new(parking_lot::Mutex::new(Activation {
            child_id: child_id.clone(),
            parent_session: parent.id().clone(),
            handle,
            ancestry,
            owned_children: HashSet::new(),
            observer,
            disposal: None,
            accepted: HashSet::new(),
            announced: false,
            poke: Arc::new(tokio::sync::Notify::new()),
        }));
        self.activations
            .lock()
            .insert(child_id.as_str().to_string(), activation.clone());
        self.acquire_ownership(&parent, child_id, &activation)?;
        let child_agent = activation.lock().handle().agent.clone();
        activation.lock().observer.start(&child_agent);
        self.watch_settlement(&activation);
        Ok(activation)
    }

    /// Submit to a freshly materialized Activation or roll it back.
    async fn submit_materialized(
        &self,
        activation: Arc<parking_lot::Mutex<Activation>>,
        content: &[ContentBlock],
        source: MessageSource,
        parent: Arc<dyn Agent>,
        signal: &Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<MessageId, SubagentError> {
        match self.submit_admitted(&activation, content, &source, parent, signal) {
            Ok(message_id) => Ok(message_id),
            Err(error) => {
                let _ = self.dispose(&activation).await;
                Err(error)
            }
        }
    }

    /// Cross the final admission cutoff and submit without yielding.
    fn submit_admitted(
        &self,
        activation: &Arc<parking_lot::Mutex<Activation>>,
        content: &[ContentBlock],
        source: &MessageSource,
        parent: Arc<dyn Agent>,
        signal: &Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<MessageId, SubagentError> {
        if (signal)() {
            return Err(SubagentError::new(
                "CANCELLED",
                "subagent request was aborted",
            ));
        }
        self.assert_admitting(parent.as_ref())?;
        let child_id = activation.lock().child_id.clone();
        {
            let activation_guard = activation.lock();
            if activation_guard.disposal.is_some() {
                return Err(SubagentError::new(
                    "ACTIVATION_CLOSING",
                    format!(
                        "subagent \"{child_id}\" activation is being disposed; the message was not accepted"
                    ),
                ));
            }
        }
        self.authorize_lineage(&parent, &child_id, activation)?;
        self.acquire_ownership(&parent, &child_id, activation)?;
        let message = create_user_message(content.to_vec(), source.clone());
        let message_id = message.id.clone();
        let child_agent = activation.lock().handle().agent.clone();
        child_agent.followup(message);
        activation
            .lock()
            .accepted
            .insert(message_id.as_str().to_string());
        activation.lock().announced = true;
        activation.lock().poke.notify_waiters();
        Ok(message_id)
    }

    /// Authorize one operation against the durable direct-parent lineage.
    fn authorize_lineage(
        &self,
        parent: &Arc<dyn Agent>,
        child_id: &SessionId,
        activation: &Arc<parking_lot::Mutex<Activation>>,
    ) -> Result<(), SubagentError> {
        let is_live = self
            .agents()
            .get(parent.id())
            .is_some_and(|live| Arc::ptr_eq(&live, parent));
        if !is_live {
            return Err(SubagentError::new(
                "UNAUTHORIZED",
                format!("subagent \"{child_id}\" delivery requires the exact live parent agent"),
            ));
        }
        let header_parent = activation
            .lock()
            .handle()
            .agent
            .session()
            .header()
            .parent_session
            .clone();
        if header_parent.as_ref() != Some(parent.id()) {
            return Err(SubagentError::new(
                "UNAUTHORIZED",
                format!("subagent \"{child_id}\" belongs to another parent session"),
            ));
        }
        Ok(())
    }

    /// Register the child in a continuation-managed parent's owned set.
    fn acquire_ownership(
        &self,
        parent: &Arc<dyn Agent>,
        child_id: &SessionId,
        _child_activation: &Arc<parking_lot::Mutex<Activation>>,
    ) -> Result<(), SubagentError> {
        let activations = self.activations.lock();
        let Some(parent_activation) = activations.get(parent.id().as_str()).cloned() else {
            return Ok(());
        };
        let mut parent_activation = parent_activation.lock();
        if parent_activation.disposal.is_some() {
            return Err(SubagentError::new(
                "ACTIVATION_CLOSING",
                format!(
                    "subagent parent \"{}\" is being disposed; the child was not established",
                    parent.id().as_str()
                ),
            ));
        }
        parent_activation
            .owned_children
            .insert(child_id.as_str().to_string());
        Ok(())
    }

    /// Remove one child from its live owner's set.
    fn release_ownership(&self, child_id: &SessionId) {
        let activations = self.activations.lock();
        for activation in activations.values() {
            let mut activation = activation.lock();
            if activation.owned_children.remove(child_id.as_str()) {
                activation.poke.notify_waiters();
            }
        }
    }

    /// Derive residency from Agent quiescence and the owned-child set.
    fn state_of(activation: &Activation) -> ActivationState {
        if activation.handle().agent.status() == dsh_agent::AgentStatus::Running
            || !activation.accepted.is_empty()
        {
            return ActivationState::Running;
        }
        if !activation.owned_children.is_empty() {
            return ActivationState::Waiting;
        }
        ActivationState::Settled
    }

    /// Follow one Activation to settlement.
    fn watch_settlement(&self, activation: &Arc<parking_lot::Mutex<Activation>>) {
        let manager = self.clone_manager();
        let activation = activation.clone();
        tokio::spawn(async move {
            loop {
                if activation.lock().disposal.is_some() {
                    return;
                }
                let poke = activation.lock().poke.clone();
                let child_agent = activation.lock().handle().agent.clone();
                child_agent.when_idle().await;
                let _ = poke.notified().await;
                if activation.lock().disposal.is_some() {
                    return;
                }
                let child_id = activation.lock().child_id.clone();
                let settling = manager
                    .locks
                    .run(&child_id, || {
                        let manager = manager.clone();
                        let activation = activation.clone();
                        async move {
                            if activation.lock().disposal.is_some()
                                || Self::state_of(&activation.lock()) != ActivationState::Settled
                            {
                                return None;
                            }
                            Some(Box::pin(async move { manager.dispose(&activation).await }))
                        }
                    })
                    .await;
                match settling {
                    None => continue,
                    Some(disposal) => {
                        if let Err(error) = disposal.await {
                            manager.ctx.logger.warn(
                                &manager.ctx,
                                vec![cordis::arc(format!(
                                    "subagent \"{}\" activation teardown failed: {error}",
                                    activation.lock().child_id.as_str()
                                ))],
                            );
                        }
                        return;
                    }
                }
            }
        });
    }

    /// Stop one Activation immediately, then release it child-first.
    async fn dispose(
        &self,
        activation: &Arc<parking_lot::Mutex<Activation>>,
    ) -> Result<(), String> {
        let existing = { activation.lock().disposal.clone() };
        let slot = match existing {
            Some(slot) => slot,
            None => {
                let slot = Arc::new(tokio::sync::Mutex::new(None));
                activation.lock().disposal = Some(slot.clone());
                slot
            }
        };
        let mut guard = slot.clone().lock_owned().await;
        if let Some(result) = &*guard {
            return result.clone();
        }
        let outcome = self.finish_disposal(activation).await;
        *guard = Some(outcome.clone());
        outcome
    }

    /// Propagate stop synchronously, then finish the child-first release.
    async fn finish_disposal(
        &self,
        activation: &Arc<parking_lot::Mutex<Activation>>,
    ) -> Result<(), String> {
        let child_id = activation.lock().child_id.clone();
        activation.lock().poke.notify_waiters();
        let child_agent = activation.lock().handle().agent.clone();
        child_agent.cancel(dsh_session::AgentCancelCause::Parent, None);
        let children = {
            let activation = activation.lock();
            activation
                .owned_children
                .iter()
                .filter_map(|child| self.activations.lock().get(child).cloned())
                .collect::<Vec<_>>()
        };
        let mut failures: Vec<String> = Vec::new();
        for child in children {
            if let Err(error) = Box::pin(self.dispose(&child)).await {
                failures.push(error);
            }
        }
        child_agent.when_idle().await;
        activation.lock().observer.capture(&child_agent);
        // Await the real handle teardown: the replace swaps in a no-op
        // stand-in only so the original dispose future can be awaited
        // outside the lock.
        let handle = {
            let mut activation = activation.lock();
            let agent = activation.handle.agent.clone();
            std::mem::replace(
                &mut activation.handle,
                AgentHandle {
                    agent,
                    dispose: Box::pin(async {}),
                },
            )
        };
        handle.dispose.await;
        let terminal_failure = if failures.is_empty() {
            None
        } else {
            Some(failures.join("; "))
        };
        self.activations.lock().remove(child_id.as_str());
        self.notify_settlement(activation, terminal_failure.as_deref());
        self.release_ownership(&child_id);
        activation
            .lock()
            .observer
            .settle(terminal_failure.as_deref());
        match terminal_failure {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }

    /// Tell the durable direct parent that this child settled.
    fn notify_settlement(
        &self,
        activation: &Arc<parking_lot::Mutex<Activation>>,
        failure: Option<&str>,
    ) {
        let (announced, child_id, parent_session, terminal) = {
            let activation = activation.lock();
            (
                activation.announced,
                activation.child_id.clone(),
                activation.parent_session.clone(),
                activation.observer.terminal(failure),
            )
        };
        if !announced {
            return;
        }
        let Some(parent) = self.agents().get(&parent_session) else {
            return;
        };
        let summary = settlement_summary(&child_id, terminal.stop_reason);
        let mut content = vec![ContentBlock::Text {
            text: summary.clone(),
        }];
        match &terminal.output {
            None => content.push(ContentBlock::Text {
                text: "It left no closing message.".to_string(),
            }),
            Some(output) => {
                content.push(ContentBlock::Text {
                    text: "Its closing message:".to_string(),
                });
                content.extend(output.iter().cloned());
            }
        }
        let message = create_user_message(
            content,
            MessageSource::SubagentSettled {
                form: dsh_llm::ContextForm::Notice,
                summary: bound_context_summary(&summary),
                sender_session_id: child_id.as_str().to_string(),
            },
        );
        if parent.status() == dsh_agent::AgentStatus::Idle {
            parent.followup(message);
        } else {
            parent.steer(message);
        }
    }

    fn assert_admitting(&self, agent: &dyn Agent) -> Result<(), SubagentError> {
        if self.draining.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SubagentError::new(
                "DRAINING",
                "continuable subagents are draining; the operation was not admitted",
            ));
        }
        let mut lineage = HashSet::new();
        lineage.insert(agent as *const dyn Agent as *const () as usize);
        let mut parent_session = agent.session().header().parent_session.clone();
        while let Some(parent_id) = parent_session {
            let Some(parent) = self.agents().get(&parent_id) else {
                break;
            };
            lineage.insert(Arc::as_ptr(&parent).cast::<()>() as usize);
            parent_session = parent.session().header().parent_session.clone();
        }
        if self
            .closing_scopes
            .lock()
            .iter()
            .any(|(root, members)| lineage.contains(root) || !lineage.is_disjoint(members))
        {
            return Err(SubagentError::new(
                "DRAINING",
                "continuable subagents below this parent are draining; the operation was not admitted",
            ));
        }
        Ok(())
    }

    fn live_lineage(&self, agent: &Arc<dyn Agent>) -> Vec<Arc<dyn Agent>> {
        let mut lineage = vec![agent.clone()];
        let mut seen: HashSet<String> = HashSet::from([agent.id().as_str().to_string()]);
        let mut parent_session = agent.session().header().parent_session.clone();
        while let Some(parent_id) = parent_session {
            let Some(parent) = self.agents().get(&parent_id) else {
                break;
            };
            if seen.contains(parent.id().as_str()) {
                break;
            }
            seen.insert(parent.id().as_str().to_string());
            parent_session = parent.session().header().parent_session.clone();
            lineage.push(parent);
        }
        lineage
    }

    fn clone_manager(&self) -> Arc<Self> {
        self.self_arc
            .get()
            .and_then(std::sync::Weak::upgrade)
            .expect("the continuation manager must be held by an Arc")
    }
}

/// One line telling a parent that a background child is finished and why.
fn settlement_summary(child_id: &SessionId, stop_reason: SubagentStopReason) -> String {
    let subject = format!("Background subagent {child_id}");
    match stop_reason {
        SubagentStopReason::Completed => {
            format!("{subject} finished and will do no further work unless you send it more.")
        }
        SubagentStopReason::Aborted => format!("{subject} was stopped before it finished."),
        SubagentStopReason::MaxTokens => format!("{subject} ran out of room before it finished."),
        SubagentStopReason::Refusal => format!("{subject} declined the task."),
        SubagentStopReason::Error => format!("{subject} failed before it finished."),
    }
}

// Re-exported for the runtime's public continuable surface.
pub use crate::types::SubagentResult as _SubagentResultAnchor;
