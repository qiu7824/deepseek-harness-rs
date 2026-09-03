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
use futures::StreamExt;
use parking_lot::Mutex;

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
        assembly: PromptAssembly,
    },
}

/// Drives one session through turn and step boundaries.
pub struct ReactLoopAgent {
    loop_ctx: Context,
    id: SessionId,
    options: AgentOptions,
    session: Session,
    inbox: Inbox,
    scope: Scope,
    scope_key: dsh_scope::ScopeKey,
    ctx: Context,
    dispatch: Mutex<Option<AgentEventDispatch>>,
    weak: Weak<Self>,
    phase: Mutex<Phase>,
    activity: Arc<Mutex<Activity>>,
    clear_inbox_when_idle: AtomicBool,
    request_header_logged: AtomicBool,
    runtime_context: RuntimeContextProjection,
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
        let scope_key = dsh_scope::ScopeKey::new();
        let scope = create_scope(
            loop_ctx,
            scope_key.clone(),
            &dsh_scope::CreateScopeOptions::default(),
        );
        let scope_ctx = scope.ctx.clone();
        let agent: Arc<Self> = Arc::new_cyclic(move |agent_ref: &Weak<Self>| {
            let inbox = Inbox::new(
                &session,
                InboxNotifications {
                    inserted: Some(inbox_notify_inserted(loop_ctx, agent_ref)),
                    discarded: Some(inbox_notify_discarded(loop_ctx, agent_ref)),
                    claimed: Some(inbox_notify_claimed(loop_ctx, agent_ref)),
                },
            )
            .expect("inbox");
            let last_turn = session
                .events()
                .iter()
                .rev()
                .find(|event| event.type_ == "turn/start")
                .and_then(|event| event.data["turn"].as_u64())
                .unwrap_or(0);
            let runtime_context = RuntimeContextProjection::new(&scope_ctx, &session);
            Self {
                loop_ctx: loop_ctx.clone(),
                id,
                options,
                session,
                inbox,
                scope,
                scope_key,
                ctx: scope_ctx,
                dispatch: Mutex::new(None),
                weak: agent_ref.clone(),
                phase: Mutex::new(Phase::Idle { last_turn }),
                activity: Arc::new(Mutex::new(Activity::resolved())),
                clear_inbox_when_idle: AtomicBool::new(false),
                request_header_logged: AtomicBool::new(false),
                runtime_context,
            }
        });
        Ok(agent)
    }

    /// The fused dispatcher, built on first need so the cyclic construction
    /// can stay weak-only (the TS constructor builds it eagerly with the
    /// `this` reference).
    fn dispatcher(&self) -> AgentEventDispatch {
        if let Some(dispatch) = self.dispatch.lock().as_ref() {
            return dispatch.clone();
        }
        let agent: Arc<dyn Agent> = self.weak.upgrade().expect("live agent");
        let dispatch = AgentEventDispatch::new(&self.loop_ctx, agent);
        *self.dispatch.lock() = Some(dispatch.clone());
        dispatch
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
            .splice(resolved_target, f64::INFINITY, 0.0, vec![message])
            .expect("inbox splice");
        if wakeup {
            self.wake_driver(waking_after_abort);
        }
    }

    fn wake_driver(&self, _wake_after_abort: bool) {
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
            if let Some(agent) = weak.upgrade() {
                let agent_dyn: Arc<dyn Agent> = agent.clone();
                if let Some(agents) = agent
                    .loop_ctx
                    .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    let _ = agents.with_initiator(agent_dyn, agent.kick()).await;
                }
                agent.finish_driver(activity_token);
            }
        });
    }

    fn clear_cancelled_inbox(&self) {
        if self.clear_inbox_when_idle.swap(false, Ordering::SeqCst) {
            self.inbox.clear().expect("inbox clear after cancellation");
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
                    })
                },
                fallback,
            )
            .await;
        throw_if_aborted(&signal)?;
        let decision = downcast_arc::<PreStepDecision>(&decision).expect("agent/pre-step decision");
        match decision.as_ref() {
            PreStepDecision::Reject => Ok(PreparedStep::Reject),
            PreStepDecision::Enter { .. } => Ok(PreparedStep::Enter {
                messages: match decision.as_ref() {
                    PreStepDecision::Enter { messages } => messages.clone(),
                    PreStepDecision::Reject => unreachable!(),
                },
                assembly,
            }),
        }
    }

    /// Open one turn before claiming its first proposed step. Returns
    /// whether another turn is pending.
    async fn turn(&self) -> Result<bool, LoopCancelled> {
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
        let mut turn_ends: Option<TurnEndReason> = None;
        let mut target = InboxTarget::NextTurn;
        let step_outcome: Result<(), LoopCancelled> = async {
            loop {
                throw_if_aborted(&signal)?;
                let step = match &*self.phase.lock() {
                    Phase::Running { step, .. } => *step + 1,
                    _ => unreachable!(),
                };
                let decision = self.pre_step(target, (turn, step)).await?;
                let PreparedStep::Enter { messages, assembly } = decision else {
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
                let step_end = async {
                    for message in &messages {
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
                    self.step(&assembly).await
                }
                .await;
                self.session
                    .append(
                        "step/end",
                        serde_json::json!({ "turn": turn, "step": step }),
                        None,
                    )
                    .expect("step/end");
                let step_end = step_end?;
                // max-tokens is sticky: a later completed step must not
                // downgrade the turn outcome.
                if let Some(step_end) = step_end {
                    if turn_ends.is_none()
                        || !matches!(turn_ends.as_ref(), Some(TurnEndReason::MaxTokens))
                    {
                        turn_ends = Some(step_end);
                    }
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
        if !self.inbox.has_pending() {
            return Ok(false);
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
    ) -> Result<Option<TurnEndReason>, LoopCancelled> {
        let (turn, step, signal) = match &*self.phase.lock() {
            Phase::Running {
                abort, turn, step, ..
            } => (*turn, *step, Arc::clone(abort)),
            _ => panic!("agent {:?}: step outside running phase", self.id.as_str()),
        };
        throw_if_aborted(&signal)?;
        let system = render_prompt(assembly).expect("renderPrompt");

        loop {
            let boundary_messages = self.session.derive_messages().expect("deriveMessages");
            let (request, prepared_call) = self
                .build_request(
                    turn,
                    step,
                    &assembly.tools,
                    &system,
                    boundary_messages.as_ref(),
                    &signal,
                )
                .await?;
            let mut assembler = BlockAssembler::new();
            let mut chunk_seqs = Vec::new();
            let stream = match &prepared_call {
                Some(prepared) => {
                    let request_for_stream = request.clone();
                    (prepared.stream)(request_for_stream)
                        .map_err(|error| LoopCancelled::failure(error.failure))?
                }
                None => self.llm().stream(request.clone()),
            };
            throw_if_aborted(&signal)?;
            let mut stream = stream;
            loop {
                let next = tokio::select! {
                    biased;
                    _ = signal.cancelled() => {
                        let content = assembler.interrupted_blocks();
                        if !content.is_empty() {
                            let message = create_assistant_message(
                                content,
                                ModelMessageSource {
                                    provider: request.provider.clone(),
                                    model: request.model.clone(),
                                    replay_state: None,
                                },
                            );
                            let mut data = serde_json::json!({
                                "turn": turn,
                                "step": step,
                                "message": message,
                                "interrupted": true,
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
                    next = stream.next() => next,
                };
                let Some(chunk) = next else {
                    break;
                };
                throw_if_aborted(&signal)?;
                let event = self
                    .session
                    .append(
                        "assistant/chunk",
                        serde_json::json!({ "turn": turn, "step": step, "chunk": chunk }),
                        None,
                    )
                    .expect("assistant/chunk");
                chunk_seqs.push(event.seq.get());
                assembler.push(&chunk);
            }
            if signal.aborted() {
                let content = assembler.interrupted_blocks();
                if !content.is_empty() {
                    let message = create_assistant_message(
                        content,
                        ModelMessageSource {
                            provider: request.provider.clone(),
                            model: request.model.clone(),
                            replay_state: None,
                        },
                    );
                    let mut data = serde_json::json!({
                        "turn": turn,
                        "step": step,
                        "message": message,
                        "interrupted": true,
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
                                provider: request.provider.clone(),
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
                throw_if_aborted(&signal)?;
                let action = downcast_arc::<Option<RequestErrorAction>>(&decision)
                    .expect("agent/request-error action");
                if action.as_ref().as_ref() != Some(&RequestErrorAction::Retry) {
                    return Err(LoopCancelled::failure(failure));
                }
                continue;
            }

            let message = create_assistant_message(
                assembler.blocks(),
                ModelMessageSource {
                    provider: request.provider.clone(),
                    model: request.model.clone(),
                    replay_state: assembler.replay_state().cloned(),
                },
            );
            let mut data = serde_json::json!({
                "turn": turn,
                "step": step,
                "message": message,
            });
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
                .expect("assistant/message");
            if finish == FinishReason::MaxTokens {
                return Ok(Some(TurnEndReason::MaxTokens));
            }

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
            if tool_calls.is_empty() {
                return Ok(Some(TurnEndReason::Completed));
            }
            let weak = self.weak.clone();
            let tools = self.tools();
            let agent = self.weak.upgrade().expect("live agent");
            let signal_for_flag = Arc::clone(&signal);
            let signal_flag: Arc<dyn Fn() -> bool + Send + Sync> =
                Arc::new(move || signal_for_flag.aborted());
            let concluded = execute_tool_calls(
                &tools,
                agent,
                10,
                turn,
                step,
                tool_calls,
                signal_flag,
                Arc::new(move |context| {
                    if let Some(agent) = weak.upgrade() {
                        let len = agent.inbox.next_step().len();
                        agent
                            .inbox
                            .splice(InboxTarget::NextStep, len as f64, 0.0, vec![context])
                            .expect("inbox context splice");
                    }
                }),
            )
            .await
            .expect("executeToolCalls");
            if concluded {
                return Ok(Some(TurnEndReason::Completed));
            }
            // Not concluded: keep stepping within the turn.
        }
    }

    /// Compose one frozen request and bind it to the adapter registration
    /// that resolved its exact-model defaults.
    async fn build_request(
        &self,
        turn: u64,
        step: u64,
        tools: &[dsh_llm::ToolSchema],
        system: &str,
        boundary_messages: &[dsh_llm::Message],
        signal: &Arc<CancellationSignal>,
    ) -> Result<(GenerateOptions, Option<dsh_llm::PreparedLlmCall>), LoopCancelled> {
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

        let header = canonical_header(&EpochHeader {
            config: config.clone(),
            adapter_defaults: prepared_call
                .as_ref()
                .map(|prepared| prepared.adapter_defaults.clone()),
            system: if system.is_empty() {
                None
            } else {
                Some(system.to_string())
            },
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
        });
        let baseline = self.session.request_header();
        if !self.request_header_logged.load(Ordering::SeqCst) {
            self.session
                .append(
                    "request/header",
                    serde_json::json!({
                        "header": header,
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
                    serde_json::json!({ "header": header, "reason": "change" }),
                    None,
                )
                .expect("request/header change");
        }

        let request_context = RequestContext {
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
            messages: boundary_messages.to_vec(),
            system: header.system.clone(),
            tools: header.tools.clone(),
            temperature: header.config.temperature,
            max_tokens: header.config.max_tokens,
            stop: header.config.stop.clone(),
            signal: None,
            session_id: Some(self.session.id().as_str().to_string()),
            purpose: None,
            agent_loop_request: false,
        };
        mark_agent_loop_request(&mut request);
        let signal_for_request = Arc::clone(signal);
        request.signal = Some(Arc::new(move || signal_for_request.aborted()));
        Ok((request, prepared_call))
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

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        &self.scope_key
    }

    fn cancel(&self, cause: AgentCancelCause, options: Option<&CancelOptions>) {
        let keep_inbox = options.map(|options| options.keep_inbox).unwrap_or(false);
        let clear_now = {
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
                    if !keep_inbox {
                        self.clear_inbox_when_idle.store(true, Ordering::SeqCst);
                        *wake_requested = false;
                    }
                    abort.abort_with(cause);
                    false
                }
                Phase::Idle { .. } => !keep_inbox,
            }
        };
        if clear_now {
            self.inbox.clear().expect("inbox clear");
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

    fn followup(&self, message: UserMessage) {
        self.send(message, InboxTarget::NextTurn, true)
    }

    fn steer(&self, message: UserMessage) {
        self.send(message, InboxTarget::NextStep, true)
    }

    fn inject(&self, message: UserMessage) {
        self.send(message, InboxTarget::NextStep, false)
    }
}
