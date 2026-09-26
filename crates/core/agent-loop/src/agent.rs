//! Default Agent driver over queued turns and step-boundary input. Every
//! request is derived from the session log. Rust port of
//! `packages/core/agent-loop/src/agent.ts`.
//!
//! # Deviations
//!
//! - `AbortSignal`/`AbortController` collapse to [`CancellationSignal`]
//!   (`AtomicBool` + reason cell); `signal.throwIfAborted()` becomes the
//!   [`LoopCancelled`] throw carrying the durable
//!   [`dsh_agent::AgentCancelCause`].
//! - The activity barrier (`activityDone`) is an epoch + shared future:
//!   `whenIdle` awaits the current shared driver, then re-reads the epoch
//!   until it stops advancing.
//! - The driver runs on a spawned task (the TS
//!   `loopCtx.agents.withInitiator` wrapper lands with the AgentLoop
//!   service).
//! - `createScope` keys by a freshly minted scope key instead of the TS
//!   agent-object identity.
//! - `runMaintenance` erases its generic result (Rust
//!   `BoxFuture<'static, ()>`).

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use cordis::{ArcValue, BoxFuture, Context, arc, downcast_arc};
use dsh_agent::{
    Agent, AgentCancelCause, AgentErrorPayload, AgentEventDispatch, AgentInboxClaimedPayload,
    AgentInboxMessagePayload, AgentOptions, AgentPreStepPayload, AgentRequestErrorPayload,
    AgentRequestPayload, AgentStatus, AgentStatusPayload, AgentTurnStoppingPayload, CancelOptions,
    CancellationSignal, Inbox, InboxNotifications, InboxTarget, PreStepDecision,
    RequestErrorAction, assemble_context_for,
};
use dsh_llm::{
    BlockAssembler, ContentBlock, FinishReason, GenerateOptions, LlmCallConfig, LlmFailure,
    ModelMessageSource, ToolCallBlock, create_assistant_message, mark_agent_loop_request,
};
use dsh_scope::{Scope, create_scope};
use dsh_session::{
    EpochHeader, RequestContext, Session, SessionId, SurfaceIntent, SurfaceOp, TurnEndCancelCause,
    TurnEndReason, UserMessage, canonical_header, header_equals,
};
use dsh_system_prompt::{
    PromptAssembly, join_context_sections, render_context_sections, render_prompt,
};
use futures::{FutureExt, StreamExt};
use parking_lot::{Mutex, ReentrantMutex, ReentrantMutexGuard};

use crate::runtime_context::RuntimeContextProjection;
use crate::tool_calls::execute_tool_calls;

/// The abort reason thrown wherever the TS code calls
/// `signal.throwIfAborted()`.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopCancelled {
    pub reason: AgentCancelCause,
    pub failure: Option<LlmFailure>,
}

impl LoopCancelled {
    fn hook(reason: impl Into<String>) -> Self {
        Self {
            reason: AgentCancelCause::Hook {
                reason: reason.into(),
            },
            failure: None,
        }
    }

    fn failure(failure: LlmFailure) -> Self {
        Self {
            reason: AgentCancelCause::Hook {
                reason: failure.message.clone(),
            },
            failure: Some(failure),
        }
    }
}

impl std::fmt::Display for LoopCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "agent loop cancelled: {:?}", self.reason)
    }
}

impl std::error::Error for LoopCancelled {}

fn throw_if_aborted(signal: &Arc<CancellationSignal>) -> Result<(), LoopCancelled> {
    if signal.aborted() {
        Err(LoopCancelled {
            reason: signal.reason().unwrap_or(AgentCancelCause::User),
            failure: None,
        })
    } else {
        Ok(())
    }
}

#[derive(Clone)]
enum Phase {
    Idle {
        last_turn: u64,
    },
    Maintenance {
        abort: Arc<CancellationSignal>,
        last_turn: u64,
        wake_requested: bool,
    },
    Running {
        abort: Arc<CancellationSignal>,
        turn: u64,
        step: u64,
        wake_requested: bool,
    },
}

struct Activity {
    epoch: u64,
    next_token: u64,
    active: HashSet<u64>,
    sender: Option<tokio::sync::watch::Sender<()>>,
    receiver: Option<tokio::sync::watch::Receiver<()>>,
}

impl Activity {
    fn resolved() -> Self {
        Self {
            epoch: 0,
            next_token: 0,
            active: HashSet::new(),
            sender: None,
            receiver: None,
        }
    }

    /// Join the current pending boundary or open a new one. Overlapping
    /// handoffs share one waiter; only the last activity may settle it.
    fn begin(&mut self) -> u64 {
        self.next_token += 1;
        let token = self.next_token;
        if self.active.is_empty() {
            self.epoch += 1;
            let (sender, receiver) = tokio::sync::watch::channel(());
            self.sender = Some(sender);
            self.receiver = Some(receiver);
        }
        self.active.insert(token);
        token
    }

    fn finish(&mut self, token: u64) -> bool {
        if !self.active.remove(&token) {
            return false;
        }
        if self.active.is_empty() {
            self.epoch += 1;
            if let Some(sender) = self.sender.take() {
                let _ = sender.send(());
            }
            self.receiver = None;
        }
        true
    }
}

/// An active driver latches every explicit wake unless teardown has made
/// that lifecycle terminal. Work consumed by the current kick resets the
/// latch before continuing; work arriving at its tail starts the next kick.
fn should_latch_active_wake(disposed: bool) -> bool {
    !disposed
}

/// Remove adapter-derived values before plugins propose the next request
/// config.

fn request_proposal(header: &EpochHeader) -> LlmCallConfig {
    let mut proposal = header.config.clone();
    if header
        .adapter_defaults
        .as_ref()
        .is_some_and(|defaults| defaults.reasoning_effort == Some(true))
    {
        proposal.reasoning_effort = None;
    }
    if header
        .adapter_defaults
        .as_ref()
        .is_some_and(|defaults| defaults.max_tokens == Some(true))
    {
        proposal.max_tokens = None;
    }
    proposal
}

enum PreparedStep {
    Reject,
    Enter {
        messages: Vec<UserMessage>,
        starts_request_series: bool,
        assembly: PromptAssembly,
    },
}

struct TurnDeadline(Option<tokio::task::JoinHandle<()>>);
impl Drop for TurnDeadline {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

/// Drives one session through turn and step boundaries.
pub struct ReactLoopAgent {
    published: AtomicBool,
    loop_ctx: Context,
    id: SessionId,
    options: AgentOptions,
    session: Session,
    inbox: Inbox,
    scope: Scope,
    scope_key: dsh_scope::ScopeKey,
    ctx: Context,
    weak: Weak<Self>,
    phase: Mutex<Phase>,
    activity: Arc<Mutex<Activity>>,
    cancelled_inbox: Mutex<Vec<dsh_llm::MessageId>>,
    control_boundary: ReentrantMutex<AgentControlState>,
    request_header_logged: AtomicBool,
    request_surface_generation: Mutex<Option<u64>>,
    runtime_context: RuntimeContextProjection,
}

#[derive(Default)]
struct AgentControlState {
    generation: Cell<u64>,
    mutation_depth: Cell<usize>,
}

struct AgentMutationGuard<'a> {
    guard: ReentrantMutexGuard<'a, AgentControlState>,
}

impl<'a> AgentMutationGuard<'a> {
    fn enter(boundary: &'a ReentrantMutex<AgentControlState>) -> Self {
        Self::from_locked(boundary.lock())
    }

    fn from_locked(guard: ReentrantMutexGuard<'a, AgentControlState>) -> Self {
        guard.mutation_depth.set(guard.mutation_depth.get() + 1);
        Self { guard }
    }
}

impl Drop for AgentMutationGuard<'_> {
    fn drop(&mut self) {
        self.guard
            .mutation_depth
            .set(self.guard.mutation_depth.get() - 1);
    }
}

impl dsh_agent::AgentControlGuard for AgentMutationGuard<'_> {}

struct DriverGuard {
    agent: Arc<ReactLoopAgent>,
    token: u64,
}

impl Drop for DriverGuard {
    fn drop(&mut self) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.agent.finish_driver(self.token);
        }));
        if outcome.is_err() {
            let mut phase = self.agent.phase.lock();
            if let Phase::Running { turn, .. } = &*phase {
                *phase = Phase::Idle { last_turn: *turn };
            }
            tracing::error!("agent driver cleanup panicked");
        }
        self.agent.activity.lock().finish(self.token);
    }
}

struct MaintenanceGuard {
    agent: Weak<ReactLoopAgent>,
    activity_token: u64,
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        let Some(agent) = self.agent.upgrade() else {
            return;
        };
        let wake_requested = {
            let mut phase = agent.phase.lock();
            let Phase::Maintenance {
                last_turn,
                wake_requested,
                ..
            } = &*phase
            else {
                drop(phase);
                agent.activity.lock().finish(self.activity_token);
                return;
            };
            let last_turn = *last_turn;
            let wake_requested = *wake_requested;
            *phase = Phase::Idle { last_turn };
            wake_requested
        };
        agent.clear_cancelled_inbox();
        if wake_requested && agent.inbox.has_pending() {
            agent.wake_driver(false);
        }
        agent.activity.lock().finish(self.activity_token);
    }
}

impl ReactLoopAgent {
    pub fn new(
        loop_ctx: &Context,
        id: SessionId,
        options: AgentOptions,
        session: Session,
    ) -> Result<Arc<Self>, String> {
        let inbox = Inbox::new(&session, InboxNotifications::default())?;
        let mut last_turn = 0;
        session.visit_events(0, None, |event| {
            if event.type_ == "turn/start" {
                last_turn = event.data["turn"].as_u64().unwrap_or(0);
            }
            Ok(true)
        })?;
        let runtime_context = RuntimeContextProjection::restore(&session)?;
        let scope_key = dsh_scope::ScopeKey::new();
        let scope = create_scope(
            loop_ctx,
            scope_key.clone(),
            &dsh_scope::CreateScopeOptions::default(),
        );
        let scope_ctx = scope.ctx.clone();
        runtime_context.attach(&scope_ctx, &session);
        let agent: Arc<Self> = Arc::new_cyclic(move |agent_ref: &Weak<Self>| {
            let inbox = inbox.with_notifications(InboxNotifications {
                inserted: Some(inbox_notify_inserted(loop_ctx, agent_ref)),
                discarded: Some(inbox_notify_discarded(loop_ctx, agent_ref)),
                claimed: Some(inbox_notify_claimed(loop_ctx, agent_ref)),
            });
            Self {
                published: AtomicBool::new(true),
                loop_ctx: loop_ctx.clone(),
                id,
                options,
                session,
                inbox,
                scope,
                scope_key,
                ctx: scope_ctx,
                weak: agent_ref.clone(),
                phase: Mutex::new(Phase::Idle { last_turn }),
                activity: Arc::new(Mutex::new(Activity::resolved())),
                cancelled_inbox: Mutex::new(Vec::new()),
                control_boundary: ReentrantMutex::new(AgentControlState::default()),
                request_header_logged: AtomicBool::new(false),
                request_surface_generation: Mutex::new(None),
                runtime_context,
            }
        });
        Ok(agent)
    }

    /// Dispatch holds the agent only for the lifetime of the current event.
    /// Caching it here would create an Agent -> dispatch -> Agent strong cycle.
    fn dispatcher(&self) -> AgentEventDispatch {
        let agent: Arc<dyn Agent> = self.weak.upgrade().expect("live agent");
        AgentEventDispatch::new(&self.loop_ctx, agent)
    }

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    fn llm(&self) -> Arc<dsh_llm::LlmRuntime> {
        self.loop_ctx
            .get_typed::<Arc<dsh_llm::LlmRuntime>>("llm", false)
            .map(|arc| arc.as_ref().clone())
            .expect("agent loop requires the llm service")
    }

    fn system_prompt(&self) -> Arc<dsh_system_prompt::SystemPrompt> {
        self.loop_ctx
            .get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
            .map(|arc| arc.as_ref().clone())
            .expect("agent loop requires the systemPrompt service")
    }

    fn tools(&self) -> Arc<dsh_tools::ToolRuntime> {
        self.loop_ctx
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .map(|arc| arc.as_ref().clone())
            .expect("agent loop requires the tools service")
    }

    /// The deployment-wide scheduler cap owned by the `agentLoop` service.
    /// Falls back to the documented default when the loop drives an agent
    /// outside the installed service (tests, embedded hosts).
    fn max_parallel_tool_calls(&self) -> usize {
        let cap = self
            .loop_ctx
            .get_typed::<Arc<crate::index::AgentLoop>>("agentLoop", false)
            .map(|service| service.max_parallel_tool_calls())
            .unwrap_or(crate::constants::DEFAULT_MAX_PARALLEL_TOOL_CALLS);
        usize::try_from(cap.max(1)).unwrap_or(usize::MAX)
    }

    fn status(&self) -> AgentStatus {
        match &*self.phase.lock() {
            Phase::Idle { .. } | Phase::Maintenance { .. } => AgentStatus::Idle,
            Phase::Running { .. } => AgentStatus::Running,
        }
    }

    fn emit_status(&self, status: AgentStatus) {
        if let Some(agent) = self.weak.upgrade() {
            let agent_dyn: Arc<dyn Agent> = agent;
            self.dispatcher().emit("agent/status", |_| {
                arc(AgentStatusPayload {
                    agent: Arc::clone(&agent_dyn),
                    status,
                })
            });
        }
    }

    fn send(&self, message: UserMessage, target: InboxTarget, wakeup: bool) {
        self.send_with_context(message, target, wakeup, None);
    }

    fn send_with_context(
        &self,
        message: UserMessage,
        target: InboxTarget,
        wakeup: bool,
        context: Option<UserMessage>,
    ) {
        let _control = AgentMutationGuard::enter(&self.control_boundary);
        let waking_after_abort = wakeup
            && matches!(
                &*self.phase.lock(),
                Phase::Maintenance { abort, .. } | Phase::Running { abort, .. }
                    if abort.aborted()
            );
        let resolved_target = if waking_after_abort {
            InboxTarget::NextTurn
        } else {
            target
        };
        self.inbox
            .append_with_context(resolved_target, message, context)
            .expect("inbox splice");
        if wakeup {
            self.wake_driver(waking_after_abort);
        }
    }

    pub(crate) fn hold_publication(&self) {
        let _control = AgentMutationGuard::enter(&self.control_boundary);
        self.published.store(false, Ordering::Release);
    }

    pub(crate) fn release_publication(&self) {
        let _control = AgentMutationGuard::enter(&self.control_boundary);
        self.published.store(true, Ordering::Release);
        if self.inbox.has_pending() {
            self.wake_driver(false);
        }
    }

    fn wake_driver(&self, _wake_after_abort: bool) {
        let _control = AgentMutationGuard::enter(&self.control_boundary);
        if !self.published.load(Ordering::Acquire) {
            return;
        }
        // Claim Idle and open its activity in one critical section. Two
        // concurrent wakeups can no longer both observe Idle and spawn
        // competing drivers for the same Session.
        let activity_token = {
            let mut activity = self.activity.lock();
            let mut phase = self.phase.lock();
            match &mut *phase {
                Phase::Maintenance {
                    abort,
                    wake_requested,
                    ..
                }
                | Phase::Running {
                    abort,
                    wake_requested,
                    ..
                } => {
                    let disposed = abort
                        .reason()
                        .is_some_and(|reason| reason == AgentCancelCause::Disposed);
                    if should_latch_active_wake(disposed) {
                        *wake_requested = true;
                    }
                    return;
                }
                Phase::Idle { last_turn } => {
                    let last_turn = *last_turn;
                    *phase = Phase::Running {
                        abort: CancellationSignal::new(),
                        turn: last_turn,
                        step: 0,
                        wake_requested: false,
                    };
                    activity.begin()
                }
            }
        };
        self.emit_status(AgentStatus::Running);
        let weak = self.weak.clone();
        tokio::spawn(async move {
            let Some(agent) = weak.upgrade() else {
                return;
            };
            let _guard = DriverGuard {
                agent: agent.clone(),
                token: activity_token,
            };
            // The driver owns the Running phase. A panic anywhere inside the
            // turn/step machinery must still release that phase and the
            // activity barrier; otherwise the agent is wedged in Running
            // forever (wakes only latch, `when_idle` never resolves, and
            // `run_maintenance` panics on "already has active work").
            let agent_dyn: Arc<dyn Agent> = agent.clone();
            let agents = agent
                .loop_ctx
                .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
                .map(|slot| slot.as_ref().clone());
            match agents {
                Some(agents) => {
                    let outcome = std::panic::AssertUnwindSafe(
                        agents.with_initiator(agent_dyn, agent.kick()),
                    )
                    .catch_unwind()
                    .await;
                    match outcome {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            tracing::warn!(
                                agent = agent.id.as_str(),
                                error = %error,
                                "agent driver could not enter the initiator boundary; queued input stays pending"
                            );
                        }
                        Err(payload) => {
                            let message = payload
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                                .unwrap_or_else(|| "non-string panic payload".to_string());
                            tracing::error!(
                                agent = agent.id.as_str(),
                                panic = %message,
                                "agent driver panicked; releasing the Running phase"
                            );
                            agent.record_driver_panic(&message);
                        }
                    }
                }
                None => {
                    tracing::error!(
                        agent = agent.id.as_str(),
                        "agent driver has no AgentRegistry (\"agents\" service); queued input stays pending"
                    );
                }
            }
        });
    }

    /// Best-effort durable record of a driver panic so the session log does
    /// not end on an open turn that only crash repair can close. Every step is
    /// fallible here; a second failure is logged and swallowed.
    fn record_driver_panic(&self, message: &str) {
        let (turn, step) = match &*self.phase.lock() {
            Phase::Running { turn, step, .. } => (*turn, *step),
            _ => return,
        };
        let mut open_turn = None;
        let mut open_step = None;
        if let Err(error) = self.session.find_event_rev(|event| {
            match event.type_.as_str() {
                "turn/start" | "turn/end" if open_turn.is_none() => {
                    open_turn = Some(event.type_ == "turn/start");
                }
                "step/start" | "step/end" if open_step.is_none() => {
                    open_step = Some(event.type_ == "step/start");
                }
                _ => {}
            }
            open_turn.is_some() && open_step.is_some()
        }) {
            tracing::warn!(error = %error, "could not inspect lifecycle after a driver panic");
            return;
        }
        if open_turn != Some(true) {
            return;
        }
        if open_step == Some(true)
            && let Err(error) = self.session.append(
                "step/end",
                serde_json::json!({ "turn": turn, "step": step }),
                None,
            )
        {
            tracing::warn!(error = %error, "could not close the step after a driver panic");
        }
        let reason = TurnEndReason::Error {
            error: LlmFailure {
                offload_images: None,
                message: format!("agent driver panicked: {message}"),
                code: "DRIVER_PANIC".to_string(),
                status: None,
                provider_retry_after_ms: None,
                request_id: None,
            },
        };
        if let Err(error) = self.session.append(
            "turn/end",
            serde_json::json!({ "turn": turn, "reason": reason }),
            None,
        ) {
            tracing::warn!(error = %error, "could not close the turn after a driver panic");
        }
    }

    fn clear_cancelled_inbox(&self) {
        let captured = std::mem::take(&mut *self.cancelled_inbox.lock());
        for id in captured {
            if let Err(error) = self.inbox.remove(&id) {
                tracing::warn!(error = %error, "could not remove cancelled inbox item");
            }
        }
    }

    fn finish_driver(&self, activity_token: u64) {
        // The driver owns the publication boundary until this point. Apply a
        // deferred cancellation clear before publishing Idle so observers
        // never see quiescence with stale queued authority.
        self.clear_cancelled_inbox();
        let wake_requested = {
            let mut phase = self.phase.lock();
            let Phase::Running {
                turn,
                wake_requested,
                ..
            } = &*phase
            else {
                return;
            };
            let turn = *turn;
            let wake_requested = *wake_requested;
            *phase = Phase::Idle { last_turn: turn };
            wake_requested
        };
        self.emit_status(AgentStatus::Idle);
        self.clear_cancelled_inbox();
        if wake_requested && self.inbox.has_pending() {
            self.wake_driver(false);
        }
        self.activity.lock().finish(activity_token);
    }

    async fn kick(&self) {
        loop {
            match self.turn().await {
                Ok(true) => continue,
                Ok(false) | Err(_) => break,
            }
        }
    }

    async fn pre_step(
        &self,
        target: InboxTarget,
        position: (u64, u64),
    ) -> Result<PreparedStep, LoopCancelled> {
        let (turn, step) = position;
        let signal = match &*self.phase.lock() {
            Phase::Running { abort, .. } => Arc::clone(abort),
            _ => panic!(
                "agent {:?}: pre-step outside running phase",
                self.id.as_str()
            ),
        };
        let claimed = self.inbox.claim(target, turn).expect("inbox claim");
        let agent: Arc<dyn Agent> = self.weak.upgrade().expect("live agent");
        let assembly = self
            .system_prompt()
            .assemble(&self.ctx, &assemble_context_for(&agent))
            .await
            .expect("prompt assembly");
        throw_if_aborted(&signal)?;
        let sections = render_context_sections(&assembly).expect("context sections");
        let context = self
            .runtime_context
            .project(&join_context_sections(&sections), &sections);
        let claimed_for_payload = claimed.clone();
        let claimed_for_fallback = claimed.clone();
        let fallback: BoxFuture<'static, ArcValue> = Box::pin(async move {
            arc(PreStepDecision::Enter {
                starts_request_series: false,
                messages: match context {
                    Some(context) => {
                        let mut messages = claimed_for_fallback;
                        messages.push(context);
                        messages
                    }
                    None => claimed_for_fallback,
                },
            })
        });
        let decision = self
            .dispatcher()
            .waterfall(
                "agent/pre-step",
                |agent| {
                    arc(AgentPreStepPayload {
                        agent: Arc::clone(agent),
                        messages: claimed_for_payload.clone(),
                        turn,
                        step,
                        signal: Arc::clone(&signal),
                    })
                },
                fallback,
            )
            .await;
        throw_if_aborted(&signal)?;
        let decision = downcast_arc::<PreStepDecision>(&decision).expect("agent/pre-step decision");
        match decision.as_ref() {
            PreStepDecision::Reject => Ok(PreparedStep::Reject),
            PreStepDecision::Enter {
                messages,
                starts_request_series,
            } => Ok(PreparedStep::Enter {
                messages: messages.clone(),
                starts_request_series: *starts_request_series,
                assembly,
            }),
        }
    }

    /// Open one turn before claiming its first proposed step. Returns
    /// whether another turn is pending.
    async fn turn(&self) -> Result<bool, LoopCancelled> {
        self.clear_cancelled_inbox();
        let signal = match &*self.phase.lock() {
            Phase::Running { abort, .. } => Arc::clone(abort),
            _ => panic!(
                "agent {:?}: turn without driver reservation",
                self.id.as_str()
            ),
        };
        throw_if_aborted(&signal)?;
        let turn = match &*self.phase.lock() {
            Phase::Running { turn, .. } => *turn + 1,
            _ => unreachable!(),
        };
        self.session
            .append("turn/start", serde_json::json!({ "turn": turn }), None)
            .expect("turn/start");
        if let Phase::Running {
            turn: phase_turn, ..
        } = &mut *self.phase.lock()
        {
            *phase_turn = turn;
        }
        let deadline = TurnDeadline(self.options.timeout_seconds.map(|seconds| {
            let weak = self.weak.clone();
            let expected_signal = signal.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
                if let Some(agent) = weak.upgrade() {
                    let _boundary = agent.control_boundary.lock();
                    let owns_turn = matches!(&*agent.phase.lock(), Phase::Running { abort, .. }
                        if Arc::ptr_eq(abort, &expected_signal) && !abort.aborted());
                    if owns_turn {
                        agent.cancel(
                            AgentCancelCause::Hook {
                                reason: "subagent timeoutSeconds exceeded".into(),
                            },
                            None,
                        );
                    }
                }
            })
        }));
        let mut turn_ends: Option<TurnEndReason> = None;
        let mut continuation = crate::response_continuation::ResponseContinuation::default();
        let mut target = InboxTarget::NextTurn;
        let step_outcome: Result<(), LoopCancelled> = async {
            let _run_permit = if let Some(gate) = self
                .ctx
                .get_typed::<Arc<dsh_agent::AgentRunAdmission>>("agentRunAdmission", false)
            {
                match (gate.admit)(self) {
                    Ok(permit) => Some(permit),
                    Err(reason) => {
                        self.cancel(AgentCancelCause::Hook { reason }, None);
                        throw_if_aborted(&signal)?;
                        unreachable!()
                    }
                }
            } else {
                None
            };
            loop {
                throw_if_aborted(&signal)?;
                let step = match &*self.phase.lock() {
                    Phase::Running { step, .. } => *step + 1,
                    _ => unreachable!(),
                };
                if self.options.max_steps.is_some_and(|limit| step > limit) {
                    self.cancel(
                        AgentCancelCause::Hook {
                            reason: "subagent maxTurns exceeded".into(),
                        },
                        None,
                    );
                    throw_if_aborted(&signal)?;
                }
                let decision = tokio::select! {
                    biased;
                    _ = signal.cancelled() => { throw_if_aborted(&signal)?; unreachable!() },
                    result = self.pre_step(target, (turn, step)) => result?,
                };
                let PreparedStep::Enter {
                    messages,
                    assembly,
                    starts_request_series,
                } = decision
                else {
                    turn_ends = Some(TurnEndReason::Blocked);
                    return Ok(());
                };
                if turn_ends.is_some() && messages.is_empty() {
                    return Ok(());
                }
                // A removed waking message or an enter decision rewritten to
                // empty still owns the initial turn boundary, but it spends
                // no model call.
                let is_first_step = match &*self.phase.lock() {
                    Phase::Running { step, .. } => *step == 0,
                    _ => unreachable!(),
                };
                if is_first_step && messages.is_empty() {
                    turn_ends = Some(TurnEndReason::Completed);
                    return Ok(());
                }
                throw_if_aborted(&signal)?;
                self.session
                    .append(
                        "step/start",
                        serde_json::json!({ "turn": turn, "step": step }),
                        None,
                    )
                    .expect("step/start");
                if let Phase::Running {
                    step: phase_step, ..
                } = &mut *self.phase.lock()
                {
                    *phase_step = step;
                }
                let step_end = self
                    .step(
                        &assembly,
                        &messages,
                        starts_request_series,
                        &mut continuation,
                    )
                    .await;
                self.session
                    .append(
                        "step/end",
                        serde_json::json!({ "turn": turn, "step": step }),
                        None,
                    )
                    .expect("step/end");
                let step_end = step_end?;
                if let Some(step_end) = step_end {
                    turn_ends = Some(step_end);
                }
                throw_if_aborted(&signal)?;
                if turn_ends.is_some() && self.inbox.next_step().is_empty() {
                    let _ = self
                        .dispatcher()
                        .serial("agent/turn-stopping", |agent| {
                            arc(AgentTurnStoppingPayload {
                                agent: Arc::clone(agent),
                                turn,
                            })
                        })
                        .await;
                    throw_if_aborted(&signal)?;
                }
                if turn_ends.is_some() && self.inbox.next_step().is_empty() {
                    return Ok(());
                }
                target = InboxTarget::NextStep;
            }
        }
        .await;
        drop(deadline);
        if let Err(error) = step_outcome {
            if signal.aborted() {
                turn_ends = Some(TurnEndReason::Aborted {
                    reason: signal
                        .reason()
                        .map(Into::<TurnEndCancelCause>::into)
                        .unwrap_or(TurnEndCancelCause::User),
                });
            } else {
                // Every failure is structured: an `LlmError` keeps its facts,
                // anything else flattens to `errorChain` text under the
                // `UNKNOWN` code.
                turn_ends = Some(TurnEndReason::Error {
                    error: error.failure.clone().unwrap_or_else(|| LlmFailure {
                        offload_images: None,
                        message: error.to_string(),
                        code: "UNKNOWN".to_string(),
                        status: None,
                        provider_retry_after_ms: None,
                        request_id: None,
                    }),
                });
                let current_step = match &*self.phase.lock() {
                    Phase::Running { step, .. } => *step,
                    _ => 0,
                };
                self.dispatcher().emit("agent/error", |agent| {
                    arc(AgentErrorPayload {
                        agent: Arc::clone(agent),
                        turn,
                        step: current_step,
                        error: serde_json::json!(error.to_string()),
                    })
                });
            }
        }
        self.session
            .append(
                "turn/end",
                serde_json::json!({ "turn": turn, "reason": turn_ends.expect("turn ending") }),
                None,
            )
            .expect("turn/end");
        self.dispatcher()
            .serial("agent/turn-finished", |agent| {
                arc(AgentTurnStoppingPayload {
                    agent: Arc::clone(agent),
                    turn,
                })
            })
            .await;
        // Remove only input captured by cancel, before deciding whether to
        // start another turn. Fresh input admitted after cancel must survive.
        self.clear_cancelled_inbox();
        if !self.inbox.has_pending() {
            return Ok(false);
        }
        // Keeping queued input is not permission to resume after the user's
        // stop request. Only a new wake admitted after cancellation resumes it.
        if let Phase::Running {
            abort,
            wake_requested,
            ..
        } = &*self.phase.lock()
        {
            if abort.reason() == Some(AgentCancelCause::User) && !*wake_requested {
                return Ok(false);
            }
        }
        // A fresh controller makes a latch set on the old one stale: the live
        // driver claims the queue itself.
        if let Phase::Running {
            abort,
            wake_requested,
            step,
            ..
        } = &mut *self.phase.lock()
        {
            *abort = CancellationSignal::new();
            *wake_requested = false;
            *step = 0;
        }
        Ok(true)
    }

    /// Execute one model step; `None` means the turn keeps stepping.
    async fn step(
        &self,
        assembly: &PromptAssembly,
        incoming: &[UserMessage],
        starts_request_series: bool,
        continuation: &mut crate::response_continuation::ResponseContinuation,
    ) -> Result<Option<TurnEndReason>, LoopCancelled> {
        let (turn, step, signal) = match &*self.phase.lock() {
            Phase::Running {
                abort, turn, step, ..
            } => (*turn, *step, Arc::clone(abort)),
            _ => panic!("agent {:?}: step outside running phase", self.id.as_str()),
        };
        throw_if_aborted(&signal)?;
        let system = render_prompt(assembly).expect("renderPrompt");

        let mut first_attempt = true;
        loop {
            let prepared = tokio::select! {
                biased;
                _ = signal.cancelled() => { throw_if_aborted(&signal)?; unreachable!() },
                result = self.prepare_request(turn, step, &signal) => result,
            };
            let (config, prepared_call) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    // Failed route preparation must not lose the admitted user input.
                    if first_attempt {
                        self.commit_system_prompt(turn, step, &system, false, true)?;
                        self.commit_incoming(incoming);
                    }
                    return Err(error);
                }
            };
            let before = self
                .session
                .surface()
                .map_err(LoopCancelled::hook)?
                .replace_generation;
            let previous_generation = *self.request_surface_generation.lock();
            let baseline = self.session.request_header();
            let tools_changed = baseline.as_ref().is_none_or(|header| {
                header.tools.as_deref().unwrap_or(&[]) != assembly.tools.as_slice()
            });
            let route_changed = baseline.as_ref().is_some_and(|header| {
                header.config.provider != config.provider || header.config.model != config.model
            });
            let explicit_series = first_attempt && starts_request_series;
            let reset_series = explicit_series
                || previous_generation != Some(before)
                || tools_changed
                || route_changed;
            let in_history = prepared_call
                .as_ref()
                .and_then(|call| call.system_prompt_update)
                == Some(dsh_llm::SystemPromptUpdate::InHistory);
            self.commit_system_prompt(turn, step, &system, in_history, reset_series)?;
            if first_attempt {
                self.commit_incoming(incoming);
            }
            first_attempt = false;
            let generation = self
                .session
                .surface()
                .map_err(LoopCancelled::hook)?
                .replace_generation;
            let starts_series =
                explicit_series || previous_generation != Some(generation) || route_changed;
            let has_boundary_messages = !self
                .session
                .derive_messages()
                .map_err(LoopCancelled::hook)?
                .is_empty();
            let mut request = self.build_request(
                &assembly.tools,
                config,
                prepared_call.as_ref(),
                starts_series,
                has_boundary_messages,
                &signal,
            )?;
            *self.request_surface_generation.lock() = Some(generation);

            let mut assembler = BlockAssembler::new();
            let mut saw_tool_call = false;
            let mut chunk_seqs = Vec::new();
            let mut request_metrics = crate::request_metrics::RequestMetrics::new();
            let (phase_sender, mut phase_receiver) =
                tokio::sync::mpsc::unbounded_channel::<dsh_llm::RequestPhase>();
            let measurement = request_metrics.provider_measurement.clone();
            request.telemetry = Some(dsh_llm::RequestTelemetry::new(Arc::new(move |phase| {
                *measurement.lock() = Some(phase.clone());
                let _ = phase_sender.send(phase);
            })));
            // The loop needs response attribution and telemetry after
            // dispatch, not another complete copy of the request context.
            let request_provider = request.provider.clone();
            let request_model = request.model.clone();
            let request_telemetry = request.telemetry.clone();
            let stream = match &prepared_call {
                Some(prepared) => (prepared.stream)(request)
                    .map_err(|error| LoopCancelled::failure(error.failure))?,
                None => self.llm().stream(request),
            };
            throw_if_aborted(&signal)?;
            let mut stream = stream;
            loop {
                let next_wait_started = std::time::Instant::now();
                let next = tokio::select! {
                    biased;
                    _ = signal.cancelled() => {
                        request_metrics.waited(next_wait_started.elapsed());
                        if let Some(telemetry) = &request_telemetry {
                            telemetry.cancel_pending();
                        }
                        while let Ok(phase) = phase_receiver.try_recv() {
                            let mut data = serde_json::to_value(phase).expect("request phase JSON");
                            data["turn"] = serde_json::json!(turn);
                            data["step"] = serde_json::json!(step);
                            self.session.append("request/phase", data, None).expect("request phase");
                        }
                        let content = assembler.interrupted_blocks();
                        if !content.is_empty() {
                            let message = create_assistant_message(
                                content,
                                ModelMessageSource {
                                    provider: request_provider.clone(),
                                    model: request_model.clone(),
                                    replay_state: None,
                                },
                            );
                            let mut data = serde_json::json!({
                                "turn": turn,
                                "step": step,
                                "message": message,
                                "interrupted": true,
                                "requestMetrics": request_metrics.value(),
                            });
                            if let Some(usage) = assembler.usage() {
                                data.as_object_mut().expect("assistant message data")
                                    .insert("usage".to_string(), serde_json::to_value(usage).expect("usage"));
                            }
                            self.session.append(
                                "assistant/message",
                                data,
                                Some(SurfaceIntent {
                                    surface_op: SurfaceOp::Append,
                                    source_event_seqs: Some(chunk_seqs),
                                }),
                            ).expect("interrupted assistant/message");
                        }
                        return Err(LoopCancelled {
                            reason: signal.reason().unwrap_or(AgentCancelCause::User),
                            failure: None,
                        });
                    },
                    phase = phase_receiver.recv() => {
                        request_metrics.waited(next_wait_started.elapsed());
                        if let Some(phase)=phase {
                            let mut data=serde_json::to_value(phase).expect("request phase JSON");
                            data["turn"]=serde_json::json!(turn);data["step"]=serde_json::json!(step);
                            self.session.append("request/phase",data,None).expect("request phase");
                        }
                        continue;
                    },
                    next = stream.next() => next,
                };
                request_metrics.waited(next_wait_started.elapsed());
                let Some(chunk) = next else {
                    break;
                };
                throw_if_aborted(&signal)?;
                let chunk_arrived = std::time::Instant::now();
                let event = self
                    .session
                    .append(
                        "assistant/chunk",
                        serde_json::json!({ "turn": turn, "step": step, "chunk": chunk }),
                        None,
                    )
                    .expect("assistant/chunk");
                chunk_seqs.push(event.seq.get());
                saw_tool_call |= matches!(&chunk,
                    dsh_llm::StreamChunk::BlockStart { block_type, .. } if block_type == "tool-call")
                    || matches!(
                        &chunk,
                        dsh_llm::StreamChunk::ToolCallDelta { .. }
                            | dsh_llm::StreamChunk::BlockEnd {
                                block: ContentBlock::ToolCall { .. },
                                ..
                            }
                    );
                assembler.push(&chunk);
                request_metrics.processed(&chunk, chunk_arrived);
            }
            if signal.aborted() {
                let content = assembler.interrupted_blocks();
                if !content.is_empty() {
                    let message = create_assistant_message(
                        content,
                        ModelMessageSource {
                            provider: request_provider.clone(),
                            model: request_model.clone(),
                            replay_state: None,
                        },
                    );
                    let mut data = serde_json::json!({
                        "turn": turn,
                        "step": step,
                        "message": message,
                        "interrupted": true,
                        "requestMetrics": request_metrics.value(),
                    });
                    if let Some(usage) = assembler.usage() {
                        data.as_object_mut()
                            .expect("assistant message data")
                            .insert(
                                "usage".to_string(),
                                serde_json::to_value(usage).expect("usage"),
                            );
                    }
                    self.session
                        .append(
                            "assistant/message",
                            data,
                            Some(SurfaceIntent {
                                surface_op: SurfaceOp::Append,
                                source_event_seqs: Some(chunk_seqs),
                            }),
                        )
                        .expect("interrupted assistant/message");
                }
                return Err(LoopCancelled {
                    reason: signal.reason().unwrap_or(AgentCancelCause::User),
                    failure: None,
                });
            }
            let finish = assembler.finish();
            if matches!(
                finish,
                FinishReason::Error { .. } | FinishReason::Aborted { .. }
            ) {
                let failure = match &finish {
                    FinishReason::Error { failure } | FinishReason::Aborted { failure } => {
                        failure.clone()
                    }
                    _ => unreachable!(),
                };
                // Provider cancellation and an explicit safety refusal are
                // terminal even when a provider's retry mode is "always".
                // They still follow the normal prefix persistence and
                // agent/error notification path below.
                let terminal_failure = matches!(finish, FinishReason::Aborted { .. })
                    || failure.code == "CONTENT_FILTER";
                let retry = if terminal_failure {
                    false
                } else if failure.code == "IMAGE_OFFLOAD_REQUIRED"
                    && failure.offload_images.is_some()
                {
                    self.session
                        .offload_oldest_images(failure.offload_images.unwrap_or(0))
                        .unwrap_or(false)
                } else {
                    let fallback: BoxFuture<'static, ArcValue> =
                        Box::pin(async { arc(None::<RequestErrorAction>) });
                    let decision = self
                        .dispatcher()
                        .waterfall(
                            "agent/request-error",
                            |agent| {
                                arc(AgentRequestErrorPayload {
                                    agent: Arc::clone(agent),
                                    turn,
                                    step,
                                    provider: request_provider.clone(),
                                    failure: failure.clone(),
                                    retry_policy: prepared_call
                                        .as_ref()
                                        .map(|prepared| prepared.retry_policy.clone()),
                                    signal: Arc::clone(&signal),
                                })
                            },
                            fallback,
                        )
                        .await;
                    let action = downcast_arc::<Option<RequestErrorAction>>(&decision)
                        .expect("agent/request-error action");
                    action.as_ref().as_ref() == Some(&RequestErrorAction::Retry)
                };
                if signal.aborted() || !retry {
                    let content = assembler.interrupted_blocks();
                    if !content.is_empty() {
                        let message = create_assistant_message(
                            content,
                            ModelMessageSource {
                                provider: request_provider.clone(),
                                model: request_model.clone(),
                                replay_state: None,
                            },
                        );
                        let mut data = serde_json::json!({"turn":turn,"step":step,"message":message,"interrupted":true,"requestMetrics":request_metrics.value()});
                        if let Some(usage) = assembler.usage() {
                            data["usage"] = serde_json::to_value(usage).expect("usage");
                        }
                        self.session
                            .append(
                                "assistant/message",
                                data,
                                Some(SurfaceIntent {
                                    surface_op: SurfaceOp::Append,
                                    source_event_seqs: Some(chunk_seqs),
                                }),
                            )
                            .expect("failed assistant/message");
                    }
                    throw_if_aborted(&signal)?;
                    return Err(LoopCancelled::failure(failure));
                }
                throw_if_aborted(&signal)?;
                continue;
            }

            let response_model = assembler
                .replay_state()
                .and_then(|state| state.get("responseModel"))
                .and_then(serde_json::Value::as_str)
                .filter(|model| !model.is_empty() && model.len() <= 1024)
                .unwrap_or(&request_model);
            let content = assembler.blocks();
            let unsafe_replay = finish == FinishReason::MaxTokens && (
                saw_tool_call || assembler.replay_state().is_some_and(|state| state["truncatedToolCalls"] == true)
                || !content.iter().any(|block| matches!(block, ContentBlock::Text { text } if !text.trim().is_empty()))
            );
            let message = create_assistant_message(
                content,
                ModelMessageSource {
                    provider: request_provider.clone(),
                    model: response_model.to_string(),
                    replay_state: (!unsafe_replay)
                        .then(|| assembler.replay_state().cloned())
                        .flatten(),
                },
            );
            let mut data = serde_json::json!({
                "turn": turn,
                "step": step,
                "message": message,
                "requestMetrics": request_metrics.value(),
            });
            if let Some(usage) = assembler.usage() {
                data["usage"] = serde_json::to_value(usage).expect("usage");
            }
            if finish == FinishReason::MaxTokens {
                data["truncated"] = serde_json::json!(true);
            }
            self.session
                .append(
                    "assistant/message",
                    data,
                    Some(SurfaceIntent {
                        surface_op: SurfaceOp::Append,
                        source_event_seqs: Some(chunk_seqs),
                    }),
                )
                .expect("assistant/message");
            let tool_calls: Vec<ToolCallBlock> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolCall {
                        id,
                        name,
                        arguments,
                    } => Some(ToolCallBlock {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    }),
                    _ => None,
                })
                .collect();
            if tool_calls.is_empty() || finish == FinishReason::MaxTokens {
                throw_if_aborted(&signal)?;
                if let Some(notice) = continuation
                    .observe(
                        &finish,
                        assembler.replay_state(),
                        &message.content,
                        saw_tool_call,
                    )
                    .map_err(LoopCancelled::failure)?
                {
                    if !notice.is_empty() {
                        self.inbox
                            .splice(
                                InboxTarget::NextStep,
                                self.inbox.next_step().len() as f64,
                                0.0,
                                vec![dsh_llm::create_user_message(
                                    vec![ContentBlock::Text {
                                        text: notice.into(),
                                    }],
                                    dsh_llm::MessageSource::Plugin {
                                        plugin: "agent-loop:response-recovery".into(),
                                        form: Some(dsh_llm::ContextForm::Notice),
                                        sections: None,
                                        summary: Some("继续完成未结束的模型答复".into()),
                                        compaction_id: None,
                                        source_command_id: None,
                                    },
                                )],
                            )
                            .expect("response continuation notice");
                    }
                    return Ok(None);
                }
                return Ok(Some(TurnEndReason::Completed));
            }
            continuation.reset();
            let weak = self.weak.clone();
            let tools = self.tools();
            let agent = self.weak.upgrade().expect("live agent");
            let signal_for_flag = Arc::clone(&signal);
            let signal_flag: Arc<dyn Fn() -> bool + Send + Sync> =
                Arc::new(move || signal_for_flag.aborted());
            let concluded = execute_tool_calls(
                &tools,
                agent,
                self.max_parallel_tool_calls(),
                turn,
                step,
                tool_calls,
                signal_flag,
                Arc::new(move |context| {
                    if let Some(agent) = weak.upgrade() {
                        let len = agent.inbox.next_step().len();
                        if let Err(error) = agent.inbox.splice(
                            InboxTarget::NextStep,
                            len as f64,
                            0.0,
                            vec![context],
                        ) {
                            tracing::warn!(
                                agent = agent.id.as_str(),
                                error = %error,
                                "dropping deferred tool context: inbox splice failed"
                            );
                        }
                    }
                }),
            )
            .await
            // A scheduler failure is a structured turn error, not a driver
            // panic: the turn still closes with `turn/end {reason: error}`.
            .map_err(|error| {
                LoopCancelled::failure(LlmFailure {
                    message: format!("tool-call scheduler: {error}"),
                    code: "TOOL_SCHEDULER_FAILED".into(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                    offload_images: None,
                })
            })?;
            if concluded {
                return Ok(Some(TurnEndReason::Completed));
            }
            // Re-enter pre_step before the next model request so steering,
            // deferred tool context, and runtime changes are admitted.
            return Ok(None);
        }
    }

    fn commit_incoming(&self, messages: &[UserMessage]) {
        for message in messages {
            self.session
                .append(
                    "user/message",
                    serde_json::to_value(message).expect("message"),
                    Some(SurfaceIntent {
                        surface_op: SurfaceOp::Append,
                        source_event_seqs: None,
                    }),
                )
                .expect("user/message");
        }
    }

    fn commit_system_prompt(
        &self,
        turn: u64,
        step: u64,
        rendered: &str,
        in_history: bool,
        starts_series: bool,
    ) -> Result<(), LoopCancelled> {
        for commit in crate::system_prompt_projection::project(
            &self.session,
            rendered,
            in_history,
            starts_series,
        )
        .map_err(LoopCancelled::hook)?
        {
            self.session.append("system/message",serde_json::json!({"turn":turn,"step":step,"prefix":commit.prefix,"message":commit.message}),Some(commit.intent)).map_err(LoopCancelled::hook)?;
        }
        Ok(())
    }

    /// Capture the route and its capabilities before changing the model surface.
    async fn prepare_request(
        &self,
        turn: u64,
        step: u64,
        signal: &Arc<CancellationSignal>,
    ) -> Result<(LlmCallConfig, Option<dsh_llm::PreparedLlmCall>), LoopCancelled> {
        let persisted_header = self.session.request_header();
        let persisted_config = persisted_header
            .as_ref()
            .map(|header| header.config.clone());
        let provider = self.options.provider.clone().unwrap_or_default();
        let model = self.options.model.clone().unwrap_or_default();
        let reasoning_effort = self.options.reasoning_effort.clone().or_else(|| {
            // Fork children carry a completed parent prefix, including its
            // request/header. A delegation that did not explicitly request an
            // effort must not inherit that parent's paid reasoning setting.
            if self.options.subagent_depth.is_some() {
                return None;
            }
            if persisted_config
                .as_ref()
                .is_some_and(|config| config.provider == provider && config.model == model)
                && persisted_header
                    .as_ref()
                    .and_then(|header| header.adapter_defaults.as_ref())
                    .is_none_or(|defaults| defaults.reasoning_effort != Some(true))
            {
                persisted_config
                    .as_ref()
                    .and_then(|config| config.reasoning_effort.clone())
            } else {
                None
            }
        });
        let max_tokens = self.options.max_tokens;
        let seed_config = if self.request_header_logged.load(Ordering::SeqCst) {
            request_proposal(persisted_header.as_ref().expect("logged header"))
        } else {
            LlmCallConfig {
                execution_mode: self.options.execution_mode,
                provider,
                model,
                reasoning_effort,
                temperature: None,
                max_tokens,
                stop: None,
            }
        };
        let fallback: BoxFuture<'static, ArcValue> =
            Box::pin(async move { arc(seed_config.clone()) });
        let proposed = self
            .dispatcher()
            .waterfall(
                "agent/request",
                |agent| {
                    arc(AgentRequestPayload {
                        agent: Arc::clone(agent),
                        turn,
                        step,
                    })
                },
                fallback,
            )
            .await;
        throw_if_aborted(signal)?;
        let proposed_config =
            downcast_arc::<LlmCallConfig>(&proposed).expect("agent/request config");
        if proposed_config.provider.is_empty() || proposed_config.model.is_empty() {
            return Err(LoopCancelled::hook(format!(
                "agent \"{}\" has no provider/model: set AgentOptions.provider and AgentOptions.model or supply both via the agent/request waterfall",
                self.id.as_str()
            )));
        }
        let (config, prepared_call) = match self.llm().prepare_call(&proposed_config, None).await {
            Ok(prepared) => (prepared.config.clone(), Some(prepared)),
            // Middleware may serve an unregistered route; terminal dispatch
            // still requires an adapter.
            Err(error) if error.code == "NO_ADAPTER" => (proposed_config.as_ref().clone(), None),
            Err(error) => return Err(LoopCancelled::failure(error.failure)),
        };
        throw_if_aborted(signal)?;

        Ok((config, prepared_call))
    }

    fn build_request(
        &self,
        tools: &[dsh_llm::ToolSchema],
        config: LlmCallConfig,
        prepared_call: Option<&dsh_llm::PreparedLlmCall>,
        starts_series: bool,
        has_boundary_messages: bool,
        signal: &Arc<CancellationSignal>,
    ) -> Result<GenerateOptions, LoopCancelled> {
        let header = canonical_header(&EpochHeader {
            config: config.clone(),
            adapter_defaults: prepared_call
                .as_ref()
                .map(|prepared| prepared.adapter_defaults.clone()),
            system: None,
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
        });
        let baseline = self.session.request_header();
        let mut persisted_header = header.clone();
        persisted_header.system = None;
        let notice = if !has_boundary_messages {
            None
        } else {
            dsh_agent::model_selection::model_switch_notice(
                baseline.as_ref().map(|header| &header.config),
                &config,
            )
        };
        if let Some(message) = &notice {
            self.session
                .append(
                    "user/message",
                    serde_json::to_value(message).expect("model switch notice"),
                    Some(SurfaceIntent {
                        surface_op: SurfaceOp::Append,
                        source_event_seqs: None,
                    }),
                )
                .expect("model switch notice");
        }
        if !self.request_header_logged.load(Ordering::SeqCst) {
            self.session
                .append(
                    "request/header",
                    serde_json::json!({
                        "header": persisted_header,
                        "reason": if baseline.is_none() { "initial" } else { "resume" },
                    }),
                    None,
                )
                .expect("request/header");
            self.request_header_logged.store(true, Ordering::SeqCst);
        } else if baseline
            .as_ref()
            .is_none_or(|baseline| !header_equals(baseline, &header))
        {
            self.session
                .append(
                    "request/header",
                    serde_json::json!({ "header": persisted_header, "reason": "change", "startsSeries": starts_series }),
                    None,
                )
                .expect("request/header change");
        } else if starts_series {
            self.session
                .append(
                    "request/header",
                    serde_json::json!({"header":persisted_header,"reason":"series"}),
                    None,
                )
                .expect("request/header series");
        }

        let request_context = RequestContext {
            system_prompt_update: prepared_call
                .as_ref()
                .and_then(|prepared| prepared.system_prompt_update),
            context_window_estimated: prepared_call
                .as_ref()
                .and_then(|prepared| prepared.context.as_ref())
                .and_then(|context| context.estimated.then_some(true)),
            provider: config.provider.clone(),
            model: config.model.clone(),
            context_window: prepared_call
                .as_ref()
                .and_then(|prepared| prepared.context.as_ref())
                .map(|context| context.context_window),
        };
        let previous_context = self.session.request_context();
        let changed = previous_context.as_ref().is_none_or(|previous| {
            previous.provider != request_context.provider
                || previous.model != request_context.model
                || previous.context_window != request_context.context_window
                || previous.context_window_estimated != request_context.context_window_estimated
                || previous.system_prompt_update != request_context.system_prompt_update
        });
        if changed {
            self.session
                .append(
                    "request/context",
                    serde_json::to_value(&request_context).expect("request context"),
                    None,
                )
                .expect("request/context");
        }
        throw_if_aborted(signal)?;

        let mut request = GenerateOptions {
            provider: header.config.provider.clone(),
            model: header.config.model.clone(),
            reasoning_effort: header.config.reasoning_effort.clone(),
            messages: self
                .session
                .derive_messages()
                .map_err(LoopCancelled::hook)?
                .as_ref()
                .clone(),
            // V3 carries system instructions exactly once in messages.
            system: None,
            tools: header.tools.clone(),
            temperature: header.config.temperature,
            max_tokens: header.config.max_tokens,
            stop: header.config.stop.clone(),
            signal: None,
            session_id: Some(self.session.id().as_str().to_string()),
            purpose: None,
            agent_loop_request: false,
            telemetry: None,
        };
        mark_agent_loop_request(&mut request);
        let signal_for_request = Arc::clone(signal);
        request.signal = Some(Arc::new(move || signal_for_request.aborted()));
        Ok(request)
    }
}

fn inbox_notify_inserted(
    loop_ctx: &Context,
    weak: &Weak<ReactLoopAgent>,
) -> Arc<dyn Fn(&UserMessage) + Send + Sync> {
    let loop_ctx = loop_ctx.clone();
    let weak = weak.clone();
    Arc::new(move |message: &UserMessage| {
        let Some(agent) = weak.upgrade() else {
            return;
        };
        let agent_dyn: Arc<dyn Agent> = agent;
        AgentEventDispatch::new(&loop_ctx, Arc::clone(&agent_dyn)).emit(
            "agent/inbox/inserted",
            |_| {
                arc(AgentInboxMessagePayload {
                    agent: Arc::clone(&agent_dyn),
                    message: message.clone(),
                })
            },
        );
    })
}

fn inbox_notify_discarded(
    loop_ctx: &Context,
    weak: &Weak<ReactLoopAgent>,
) -> Arc<dyn Fn(&UserMessage) + Send + Sync> {
    let loop_ctx = loop_ctx.clone();
    let weak = weak.clone();
    Arc::new(move |message: &UserMessage| {
        let Some(agent) = weak.upgrade() else {
            return;
        };
        let agent_dyn: Arc<dyn Agent> = agent;
        AgentEventDispatch::new(&loop_ctx, Arc::clone(&agent_dyn)).emit(
            "agent/inbox/discarded",
            |_| {
                arc(AgentInboxMessagePayload {
                    agent: Arc::clone(&agent_dyn),
                    message: message.clone(),
                })
            },
        );
    })
}

fn inbox_notify_claimed(
    loop_ctx: &Context,
    weak: &Weak<ReactLoopAgent>,
) -> Arc<dyn Fn(&UserMessage, u64) + Send + Sync> {
    let loop_ctx = loop_ctx.clone();
    let weak = weak.clone();
    Arc::new(move |message: &UserMessage, turn: u64| {
        let Some(agent) = weak.upgrade() else {
            return;
        };
        let agent_dyn: Arc<dyn Agent> = agent;
        AgentEventDispatch::new(&loop_ctx, Arc::clone(&agent_dyn)).emit(
            "agent/inbox/claimed",
            |_| {
                arc(AgentInboxClaimedPayload {
                    agent: Arc::clone(&agent_dyn),
                    message: message.clone(),
                    turn,
                })
            },
        );
    })
}

impl Agent for ReactLoopAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn options(&self) -> &AgentOptions {
        &self.options
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    fn status(&self) -> AgentStatus {
        self.status()
    }

    fn try_idle_control(
        &self,
    ) -> Result<Box<dyn dsh_agent::AgentControlGuard + '_>, dsh_agent::AgentControlBusy> {
        use dsh_agent::AgentControlBusy;
        let boundary = self
            .control_boundary
            .try_lock()
            .ok_or(AgentControlBusy::Contended)?;
        // Reentrant event dispatch must not observe the half-published window
        // between the durable inbox event and its in-memory projection.
        if boundary.mutation_depth.get() != 0 {
            return Err(AgentControlBusy::Contended);
        }
        if !self.published.load(Ordering::Acquire) {
            return Err(AgentControlBusy::Unavailable);
        }
        let activity = self.activity.lock();
        let phase = self.phase.lock();
        if !matches!(&*phase, Phase::Idle { .. }) || !activity.active.is_empty() {
            return Err(AgentControlBusy::Active);
        }
        if self.inbox.has_pending() {
            return Err(AgentControlBusy::PendingInput);
        }
        Ok(Box::new(AgentMutationGuard::from_locked(boundary)))
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        &self.scope_key
    }

    fn cancel(&self, cause: AgentCancelCause, options: Option<&CancelOptions>) {
        let control = AgentMutationGuard::enter(&self.control_boundary);
        let generation = &control.guard.generation;
        generation.set(generation.get().wrapping_add(1));
        let keep_inbox = options.map(|options| options.keep_inbox).unwrap_or(false);
        let clear_now = {
            let mut phase = self.phase.lock();
            if !keep_inbox {
                self.cancelled_inbox.lock().extend(self.inbox.pending_ids());
            }
            match &mut *phase {
                Phase::Maintenance {
                    abort,
                    wake_requested,
                    ..
                }
                | Phase::Running {
                    abort,
                    wake_requested,
                    ..
                } => {
                    if !keep_inbox || cause == AgentCancelCause::User {
                        *wake_requested = false;
                    }
                    abort.abort_with(cause);
                    false
                }
                Phase::Idle { .. } => !keep_inbox,
            }
        };
        if clear_now {
            self.clear_cancelled_inbox();
        }
    }

    fn try_generation_control(
        &self,
        expected: u64,
    ) -> Result<Box<dyn dsh_agent::AgentControlGuard + '_>, dsh_agent::AgentControlBusy> {
        use dsh_agent::AgentControlBusy;
        let boundary = self
            .control_boundary
            .try_lock()
            .ok_or(AgentControlBusy::Contended)?;
        if boundary.mutation_depth.get() != 0 || boundary.generation.get() != expected {
            return Err(AgentControlBusy::Contended);
        }
        if !self.published.load(Ordering::Acquire) {
            return Err(AgentControlBusy::Unavailable);
        }
        Ok(Box::new(AgentMutationGuard::from_locked(boundary)))
    }

    fn cancellation_generation(&self) -> Option<u64> {
        Some(self.control_boundary.lock().generation.get())
    }

    fn running_cancellation_generation(&self) -> Option<u64> {
        let boundary = self.control_boundary.lock();
        match &*self.phase.lock() {
            Phase::Running { abort, .. } if !abort.aborted() => Some(boundary.generation.get()),
            _ => None,
        }
    }

    fn send_from_generation(
        &self,
        message: UserMessage,
        target: InboxTarget,
        expected: Option<u64>,
    ) -> bool {
        // Inbox listeners execute synchronously and may call cancel again on
        // this thread. A reentrant guard keeps that legal while excluding a
        // concurrent Stop from crossing the enqueue/wake boundary.
        let control = AgentMutationGuard::enter(&self.control_boundary);
        let current = || expected == Some(control.guard.generation.get());
        let target = if !current()
            || matches!(
                &*self.phase.lock(),
                Phase::Maintenance { abort, .. } | Phase::Running { abort, .. }
                    if abort.aborted()
            ) {
            InboxTarget::NextTurn
        } else {
            target
        };
        self.inbox.append(target, message).expect("inbox splice");
        // An inbox notification may have synchronously stopped this agent.
        if current() {
            self.wake_driver(false);
            true
        } else {
            false
        }
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        let activity = Arc::clone(&self.activity);
        Box::pin(async move {
            loop {
                let (epoch, mut receiver) = {
                    let activity = activity.lock();
                    (activity.epoch, activity.receiver.clone())
                };
                if let Some(receiver) = &mut receiver {
                    let _ = receiver.changed().await;
                }
                if activity.lock().epoch == epoch {
                    return;
                }
            }
        })
    }

    fn run_maintenance(
        &self,
        task: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()> {
        let _control = AgentMutationGuard::enter(&self.control_boundary);
        let activity_token = {
            let mut activity = self.activity.lock();
            let mut phase = self.phase.lock();
            let Phase::Idle { last_turn } = &*phase else {
                panic!("agent \"{}\" already has active work", self.id.as_str());
            };
            let last_turn = *last_turn;
            *phase = Phase::Maintenance {
                abort: CancellationSignal::new(),
                last_turn,
                wake_requested: false,
            };
            activity.begin()
        };
        let guard = MaintenanceGuard {
            agent: self.weak.clone(),
            activity_token,
        };
        Box::pin(async move {
            let _guard = guard;
            let _ = task().await;
        })
    }

    fn send(&self, message: UserMessage, target: InboxTarget, wakeup: bool) {
        self.send(message, target, wakeup)
    }

    fn send_with_context(
        &self,
        message: UserMessage,
        target: InboxTarget,
        context: Option<UserMessage>,
    ) {
        self.send_with_context(message, target, true, context);
    }

    fn followup(&self, message: UserMessage) {
        self.send(message, InboxTarget::NextTurn, true)
    }

    fn steer(&self, message: UserMessage) {
        self.send(message, InboxTarget::NextStep, true)
    }

    fn steer_queued(&self, message_id: &dsh_llm::MessageId) -> Result<bool, String> {
        let _control = AgentMutationGuard::enter(&self.control_boundary);
        let moved = self.inbox.move_to_next_step(message_id)?;
        if moved {
            self.wake_driver(true);
        }
        Ok(moved)
    }

    fn inject(&self, message: UserMessage) {
        self.send(message, InboxTarget::NextStep, false)
    }
}
