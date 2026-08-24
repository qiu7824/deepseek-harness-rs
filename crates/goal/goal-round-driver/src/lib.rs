//! Same-session automatic goal-round driver.

pub mod invariant;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{
    ArcValue, Context, Disposer, EventOptions, Listener, NextFn, arc, downcast_arc, make_disposer,
};
use dsh_agent::{
    Agent, AgentErrorPayload, AgentInboxClaimedPayload, AgentInboxMessagePayload,
    AgentLifecyclePayload, AgentPreStepPayload, AgentRegistry, AgentSessionStartPayload,
    AgentStatus, AgentStatusPayload, CancelOptions, InboxTarget, PreStepDecision,
};
use dsh_goal::{
    GoalActivation, GoalBlockReason, GoalChangedPayload, GoalPhase, GoalRef, GoalService, GoalView,
};
use dsh_llm::{ContentBlock, MessageSource, create_user_message};
use dsh_session::{Session, SessionEvent, SessionStore};
use parking_lot::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttemptPhase {
    Queued,
    Claimed,
    Admitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReservationVerdict {
    Valid,
    Invalid,
    InfrastructureFailure,
}

fn reservation_verdict(result: Result<bool, String>) -> ReservationVerdict {
    match result {
        Ok(true) => ReservationVerdict::Valid,
        Ok(false) => ReservationVerdict::Invalid,
        Err(_) => ReservationVerdict::InfrastructureFailure,
    }
}

#[derive(Clone)]
struct RoundAttempt {
    goal_id: String,
    revision: u64,
    round: u64,
    message_id: dsh_llm::MessageId,
    content: Vec<ContentBlock>,
    phase: AttemptPhase,
    stale: bool,
    cancelled: bool,
}

struct AgentDriverState {
    agent: Arc<dyn Agent>,
    requested: AtomicBool,
    running: AtomicBool,
    attempt: Mutex<Option<RoundAttempt>>,
}

struct Driver {
    agents: Arc<AgentRegistry>,
    goals: Arc<GoalService>,
    sessions: Arc<SessionStore>,
    states: Mutex<HashMap<usize, Arc<AgentDriverState>>>,
    stopping: AtomicBool,
    tasks: Arc<DriverTaskTracker>,
}

#[derive(Default)]
struct DriverTaskTrackerState {
    closed: bool,
    active: usize,
}

struct DriverTaskTracker {
    state: Mutex<DriverTaskTrackerState>,
    changed: tokio::sync::Notify,
}

impl DriverTaskTracker {
    fn new() -> Self {
        Self {
            state: Mutex::new(DriverTaskTrackerState::default()),
            changed: tokio::sync::Notify::new(),
        }
    }

    fn try_register(self: &Arc<Self>) -> Option<DriverTaskGuard> {
        let mut state = self.state.lock();
        if state.closed {
            return None;
        }
        state.active += 1;
        Some(DriverTaskGuard {
            tracker: self.clone(),
        })
    }

    async fn stop_and_drain(&self) {
        {
            let mut state = self.state.lock();
            state.closed = true;
        }
        loop {
            let changed = self.changed.notified();
            if self.state.lock().active == 0 {
                return;
            }
            changed.await;
        }
    }
}

struct DriverTaskGuard {
    tracker: Arc<DriverTaskTracker>,
}

impl Drop for DriverTaskGuard {
    fn drop(&mut self) {
        let mut state = self.tracker.state.lock();
        state.active -= 1;
        let drained = state.active == 0;
        drop(state);
        if drained {
            self.tracker.changed.notify_waiters();
        }
    }
}

fn agent_key(agent: &Arc<dyn Agent>) -> usize {
    Arc::as_ptr(agent) as *const () as usize
}

fn positive_goal_source(message: &dsh_llm::UserMessage) -> Option<(&str, u64, u64)> {
    match &message.source {
        MessageSource::Goal {
            goal_id,
            revision,
            round,
        } if *round > 0 => Some((goal_id.as_str(), *revision, *round)),
        _ => None,
    }
}

fn same_reserved_message(message: &dsh_llm::UserMessage, attempt: &RoundAttempt) -> bool {
    positive_goal_source(message).is_some_and(|(goal_id, revision, round)| {
        message.id == attempt.message_id
            && message.content == attempt.content
            && goal_id == attempt.goal_id
            && revision == attempt.revision
            && round == attempt.round
    })
}

fn exact_positive_batch(messages: &[dsh_llm::UserMessage], attempt: &RoundAttempt) -> bool {
    let mut positive = messages
        .iter()
        .filter(|message| positive_goal_source(message).is_some());
    positive
        .next()
        .is_some_and(|message| same_reserved_message(message, attempt))
        && positive.next().is_none()
}

fn restore_other_claimed(
    agent: &Arc<dyn Agent>,
    messages: &[dsh_llm::UserMessage],
    submitted_id: &dsh_llm::MessageId,
) -> Result<(), String> {
    for message in messages
        .iter()
        .filter(|message| {
            message.id != *submitted_id && !matches!(message.source, MessageSource::Goal { .. })
        })
        .rev()
    {
        let pending = agent
            .inbox()
            .next_step()
            .iter()
            .chain(agent.inbox().next_turn().iter())
            .any(|candidate| candidate.id == message.id);
        if !pending {
            agent
                .inbox()
                .prepend(InboxTarget::NextStep, message.clone())?;
        }
    }
    Ok(())
}

fn reserve_attempt(slot: &Mutex<Option<RoundAttempt>>, attempt: RoundAttempt) -> bool {
    let mut slot = slot.lock();
    if slot.is_some() {
        return false;
    }
    *slot = Some(attempt);
    true
}

fn same_attempt(left: &RoundAttempt, right: &RoundAttempt) -> bool {
    left.goal_id == right.goal_id
        && left.revision == right.revision
        && left.round == right.round
        && left.message_id == right.message_id
        && left.content == right.content
}

fn clear_matching_attempt(state: &AgentDriverState, message: &dsh_llm::UserMessage) -> bool {
    let mut attempt = state.attempt.lock();
    if attempt
        .as_ref()
        .is_some_and(|attempt| same_reserved_message(message, attempt))
    {
        *attempt = None;
        true
    } else {
        false
    }
}

fn clear_exact_attempt(state: &AgentDriverState, expected: &RoundAttempt) -> bool {
    let mut attempt = state.attempt.lock();
    if attempt
        .as_ref()
        .is_some_and(|current| same_attempt(current, expected))
    {
        *attempt = None;
        true
    } else {
        false
    }
}

async fn retire_unowned_attempt(state: &Arc<AgentDriverState>, expected: RoundAttempt) {
    loop {
        let current = state.attempt.lock().clone();
        let Some(current) = current else {
            return;
        };
        if !same_attempt(&current, &expected) {
            return;
        }
        match current.phase {
            AttemptPhase::Queued if state.agent.status() == AgentStatus::Idle => {
                if let Ok(true) = state.agent.inbox().remove(&current.message_id) {
                    clear_exact_attempt(state, &current);
                    return;
                }
            }
            AttemptPhase::Queued => {}
            AttemptPhase::Claimed | AttemptPhase::Admitted => {
                if state.agent.status() == AgentStatus::Idle {
                    clear_exact_attempt(state, &current);
                    return;
                }
            }
        }
        tokio::task::yield_now().await;
    }
}

impl Driver {
    fn state_for(&self, agent: Arc<dyn Agent>) -> Arc<AgentDriverState> {
        self.states
            .lock()
            .entry(agent_key(&agent))
            .or_insert_with(|| {
                Arc::new(AgentDriverState {
                    agent,
                    requested: AtomicBool::new(false),
                    running: AtomicBool::new(false),
                    attempt: Mutex::new(None),
                })
            })
            .clone()
    }

    fn is_live(&self, agent: &Arc<dyn Agent>) -> bool {
        self.agents
            .get(agent.id())
            .is_some_and(|current| Arc::ptr_eq(&current, agent))
    }

    fn disarm(&self, agent: &Arc<dyn Agent>) {
        let _ = self.goals.disarm(agent);
    }

    fn valid_reservation(
        &self,
        state: &Arc<AgentDriverState>,
        message: &dsh_llm::UserMessage,
    ) -> Result<bool, String> {
        if self.stopping.load(Ordering::SeqCst) || !self.is_live(&state.agent) {
            return Ok(false);
        }
        let attempt = state.attempt.lock().clone();
        let Some(attempt) = attempt else {
            return Ok(false);
        };
        if attempt.phase != AttemptPhase::Claimed
            || attempt.stale
            || attempt.cancelled
            || !same_reserved_message(message, &attempt)
        {
            return Ok(false);
        }
        let Some(goal) = self
            .goals
            .get(&state.agent)
            .map_err(|error| error.to_string())?
        else {
            return Ok(false);
        };
        Ok(goal.id.as_str() == attempt.goal_id
            && goal.revision == attempt.revision
            && goal.phase == GoalPhase::Active
            && goal.activation == GoalActivation::Armed
            && attempt.round == goal.rounds_started + 1)
    }

    async fn drive_once(&self, state: &Arc<AgentDriverState>) -> Result<(), String> {
        let agent = &state.agent;
        if self.stopping.load(Ordering::SeqCst)
            || !self.is_live(agent)
            || agent.status() != AgentStatus::Idle
        {
            return Ok(());
        }
        self.sessions.flush(agent.session()).await?;
        if self.stopping.load(Ordering::SeqCst)
            || !self.is_live(agent)
            || agent.status() != AgentStatus::Idle
        {
            return Ok(());
        }
        let Some(goal) = self.goals.get(agent).map_err(|error| error.to_string())? else {
            return Ok(());
        };
        if goal.phase != GoalPhase::Active || goal.activation != GoalActivation::Armed {
            return Ok(());
        }
        if goal.rounds_started >= goal.max_goal_rounds {
            self.goals
                .block(
                    agent,
                    &GoalRef {
                        id: goal.id,
                        revision: goal.revision,
                    },
                    &GoalBlockReason {
                        code: "round-limit".to_string(),
                        message: format!(
                            "Goal reached its configured limit of {} rounds.",
                            goal.max_goal_rounds
                        ),
                    },
                )
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        let round = goal.rounds_started + 1;
        let content = render_goal_round_prompt(&goal, round);
        let message = create_user_message(
            content.clone(),
            MessageSource::Goal {
                goal_id: goal.id.as_str().to_string(),
                revision: goal.revision,
                round,
            },
        );
        let attempt = RoundAttempt {
            goal_id: goal.id.as_str().to_string(),
            revision: goal.revision,
            round,
            message_id: message.id.clone(),
            content,
            phase: AttemptPhase::Queued,
            stale: false,
            cancelled: false,
        };
        if !reserve_attempt(&state.attempt, attempt) {
            return Ok(());
        }
        agent.followup(message);
        Ok(())
    }

    async fn drive_task(self: Arc<Self>, state: Arc<AgentDriverState>) {
        let run = self
            .agents
            .without_initiator(async {
                while state.requested.swap(false, Ordering::SeqCst)
                    && !self.stopping.load(Ordering::SeqCst)
                {
                    if let Err(_error) = self.drive_once(&state).await {
                        self.disarm(&state.agent);
                        break;
                    }
                }
            })
            .await;
        if run.is_err() {
            self.disarm(&state.agent);
        }
        state.running.store(false, Ordering::SeqCst);
        if state.requested.load(Ordering::SeqCst) && !self.stopping.load(Ordering::SeqCst) {
            self.start_state(state);
        }
    }

    fn start_state(self: &Arc<Self>, state: Arc<AgentDriverState>) {
        if state.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(task_guard) = self.tasks.try_register() else {
            state.running.store(false, Ordering::SeqCst);
            return;
        };
        let driver = self.clone();
        tokio::spawn(async move {
            let _guard = task_guard;
            tokio::task::yield_now().await;
            if state.agent.status() != AgentStatus::Idle {
                state.running.store(false, Ordering::SeqCst);
                return;
            }
            let task_driver = driver.clone();
            let task_state = state.clone();
            let maintenance = state.agent.run_maintenance(Arc::new(move || {
                let driver = task_driver.clone();
                let state = task_state.clone();
                Box::pin(async move { driver.drive_task(state).await })
            }));
            maintenance.await;
        });
    }

    fn request(self: &Arc<Self>, agent: Arc<dyn Agent>) {
        if self.stopping.load(Ordering::SeqCst) || !self.is_live(&agent) {
            return;
        }
        let state = self.state_for(agent);
        state.requested.store(true, Ordering::SeqCst);
        self.start_state(state);
    }
}

fn session_start_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            if let Some(payload) = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentSessionStartPayload>())
                && driver.is_live(&payload.agent)
            {
                let state = driver.state_for(payload.agent.clone());
                *state.attempt.lock() = None;
                state.requested.store(false, Ordering::SeqCst);
            }
            None
        })
    })
}

fn error_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            if let Some(payload) = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentErrorPayload>())
                && driver.is_live(&payload.agent)
            {
                driver.disarm(&payload.agent);
            }
            None
        })
    })
}

fn goal_changed_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            if let Some(payload) = args
                .first()
                .and_then(|value| value.downcast_ref::<GoalChangedPayload>())
            {
                driver.request(payload.agent.clone());
            }
            None
        })
    })
}

fn inserted_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            let payload = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentInboxMessagePayload>())?;
            if !payload
                .agent
                .inbox()
                .next_turn()
                .iter()
                .any(|message| message.id == payload.message.id)
            {
                return None;
            }
            let state = driver.state_for(payload.agent.clone());
            let mut attempt = state.attempt.lock();
            let is_reserved = attempt
                .as_ref()
                .is_some_and(|attempt| same_reserved_message(&payload.message, attempt));
            if !is_reserved
                && let Some(attempt) = attempt.as_mut()
                && attempt.phase == AttemptPhase::Queued
            {
                attempt.stale = true;
            }
            None
        })
    })
}

fn discarded_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            let payload = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentInboxMessagePayload>())?;
            let state = driver.state_for(payload.agent.clone());
            let mut attempt = state.attempt.lock();
            if attempt
                .as_ref()
                .is_some_and(|attempt| same_reserved_message(&payload.message, attempt))
                && let Some(attempt) = attempt.as_mut()
            {
                attempt.cancelled = true;
            }
            None
        })
    })
}

fn claimed_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            let payload = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentInboxClaimedPayload>())?;
            let state = driver.state_for(payload.agent.clone());
            let mut attempt = state.attempt.lock();
            if attempt
                .as_ref()
                .is_some_and(|attempt| same_reserved_message(&payload.message, attempt))
                && let Some(attempt) = attempt.as_mut()
            {
                attempt.phase = AttemptPhase::Claimed;
            }
            None
        })
    })
}

fn session_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            let session = args
                .first()
                .and_then(|value| value.downcast_ref::<Session>())?;
            let event = args
                .get(1)
                .and_then(|value| value.downcast_ref::<SessionEvent>())?;
            let agent = driver.agents.get(session.id())?;
            if !agent.session().ptr_eq(session) {
                return None;
            }
            let state = driver.state_for(agent);
            let mut attempt = state.attempt.lock();
            match event.type_.as_str() {
                "user/message" => {
                    if attempt
                        .as_ref()
                        .is_some_and(|attempt| event.data["id"] == attempt.message_id.as_str())
                        && let Some(attempt) = attempt.as_mut()
                    {
                        attempt.phase = AttemptPhase::Admitted;
                    }
                }
                "turn/end" if event.data["reason"]["kind"] == "max-tokens" => {
                    drop(attempt);
                    driver.disarm(&state.agent);
                    return None;
                }
                "turn/end" if event.data["reason"]["kind"] == "aborted" => {
                    if attempt.as_ref().is_some_and(|attempt| {
                        matches!(
                            attempt.phase,
                            AttemptPhase::Claimed | AttemptPhase::Admitted
                        )
                    }) && let Some(attempt) = attempt.as_mut()
                    {
                        attempt.cancelled = true;
                    } else {
                        drop(attempt);
                        driver.disarm(&state.agent);
                        return None;
                    }
                }
                _ => {}
            }
            None
        })
    })
}

fn pre_step_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            let payload = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentPreStepPayload>())
                .cloned()
                .expect("agent/pre-step payload");
            let next = downcast_arc::<NextFn>(args.last().expect("agent/pre-step next"))
                .expect("agent/pre-step next");
            let Some(submitted) = payload
                .messages
                .iter()
                .find(|message| positive_goal_source(message).is_some())
            else {
                return Some(next.call().await);
            };
            let state = driver.state_for(payload.agent.clone());
            let verdict = reservation_verdict(driver.valid_reservation(&state, submitted));
            if verdict != ReservationVerdict::Valid {
                let cancelled = state.attempt.lock().as_ref().is_some_and(|attempt| {
                    attempt.cancelled && same_reserved_message(submitted, attempt)
                });
                if !cancelled {
                    clear_matching_attempt(&state, submitted);
                }
                let restored =
                    restore_other_claimed(&payload.agent, &payload.messages, &submitted.id);
                if restored.is_err() || verdict == ReservationVerdict::InfrastructureFailure {
                    driver.disarm(&payload.agent);
                } else if !cancelled {
                    state.requested.store(true, Ordering::SeqCst);
                }
                return Some(arc(PreStepDecision::Reject));
            }
            let decision_value = next.call().await;
            let decision = downcast_arc::<PreStepDecision>(&decision_value)
                .expect("agent/pre-step decision")
                .as_ref()
                .clone();
            if matches!(decision, PreStepDecision::Reject) {
                clear_matching_attempt(&state, submitted);
                let submitted_source = positive_goal_source(submitted);
                match driver.goals.get(&payload.agent) {
                    Ok(Some(goal))
                        if submitted_source.is_some_and(|(goal_id, revision, _)| {
                            goal.id.as_str() == goal_id && goal.revision == revision
                        }) && goal.phase == GoalPhase::Active
                            && goal.activation == GoalActivation::Armed =>
                    {
                        if driver
                            .goals
                            .block(
                                &payload.agent,
                                &GoalRef {
                                    id: goal.id,
                                    revision: goal.revision,
                                },
                                &GoalBlockReason {
                                    code: "prompt-rejected".to_string(),
                                    message: "Goal round was rejected before entering its step."
                                        .to_string(),
                                },
                            )
                            .is_err()
                        {
                            driver.disarm(&payload.agent);
                        }
                    }
                    Err(_) => driver.disarm(&payload.agent),
                    _ => {}
                }
                return Some(decision_value);
            }
            let exact_batch = match &decision {
                PreStepDecision::Enter { messages } => state
                    .attempt
                    .lock()
                    .as_ref()
                    .is_some_and(|attempt| exact_positive_batch(messages, attempt)),
                PreStepDecision::Reject => false,
            };
            let post_verdict = if exact_batch {
                reservation_verdict(driver.valid_reservation(&state, submitted))
            } else {
                ReservationVerdict::Invalid
            };
            if post_verdict != ReservationVerdict::Valid {
                clear_matching_attempt(&state, submitted);
                let restored = match &decision {
                    PreStepDecision::Enter { messages } => {
                        restore_other_claimed(&payload.agent, messages, &submitted.id)
                    }
                    PreStepDecision::Reject => Ok(()),
                };
                if restored.is_err() || post_verdict == ReservationVerdict::InfrastructureFailure {
                    driver.disarm(&payload.agent);
                } else {
                    state.requested.store(true, Ordering::SeqCst);
                }
                return Some(arc(PreStepDecision::Reject));
            }
            Some(decision_value)
        })
    })
}

fn status_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            if let Some(payload) = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentStatusPayload>())
                && payload.status == AgentStatus::Idle
            {
                let state = driver.state_for(payload.agent.clone());
                let (should_pause, should_clear) = state
                    .attempt
                    .lock()
                    .as_ref()
                    .map(|attempt| {
                        (
                            attempt.cancelled || attempt.phase == AttemptPhase::Claimed,
                            attempt.phase == AttemptPhase::Admitted && !attempt.cancelled,
                        )
                    })
                    .unwrap_or((false, false));
                if should_pause || should_clear {
                    *state.attempt.lock() = None;
                }
                if should_pause {
                    match driver.goals.get(&payload.agent) {
                        Ok(Some(goal))
                            if goal.phase == GoalPhase::Active
                                && goal.activation == GoalActivation::Armed =>
                        {
                            if driver
                                .goals
                                .pause(
                                    &payload.agent,
                                    &GoalRef {
                                        id: goal.id,
                                        revision: goal.revision,
                                    },
                                )
                                .is_err()
                            {
                                driver.disarm(&payload.agent);
                            }
                        }
                        Err(_) => driver.disarm(&payload.agent),
                        _ => {}
                    }
                }
                driver.request(payload.agent.clone());
            }
            None
        })
    })
}

fn disposed_listener(driver: Arc<Driver>) -> Arc<Listener> {
    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let driver = driver.clone();
        Box::pin(async move {
            if let Some(payload) = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentLifecyclePayload>())
            {
                driver.states.lock().remove(&agent_key(&payload.agent));
            }
            None
        })
    })
}

pub const NAME: &str = "goal-round-driver";
pub const INJECT: [&str; 3] = ["agents", "goals", "sessions"];

/// Canonical model-visible continuation prompt for one exact goal round.
pub fn render_goal_round_prompt(goal: &GoalView, round: u64) -> Vec<ContentBlock> {
    vec![ContentBlock::Text {
        text: format!(
            "<goal_round>\nObjective: {}\nRound: {}/{}\n\nContinue working toward the objective in this same session. Treat the current workspace, tool results, and durable session state as authoritative; inspect them instead of assuming earlier narration is still current. Make concrete progress and verify the result. Before claiming completion, gather evidence that the whole objective is achieved, read the current goal, and mark it complete. If work remains, leave the goal active for the next round. Follow the configured goal-tool policy before reporting a blocker.\n</goal_round>",
            serde_json::to_string(&goal.objective).expect("objective JSON"),
            round,
            goal.max_goal_rounds,
        ),
    }]
}

/// Install automatic same-session continuation for exact live agents.
pub fn apply(ctx: &Context) -> Result<Disposer, String> {
    let agents = ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "goal-round-driver requires the agents service".to_string())?;
    let goals = ctx
        .get_typed::<Arc<GoalService>>("goals", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "goal-round-driver requires the goals service".to_string())?;
    let sessions = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "goal-round-driver requires the sessions service".to_string())?;
    let driver = Arc::new(Driver {
        agents,
        goals,
        sessions,
        states: Mutex::new(HashMap::new()),
        stopping: AtomicBool::new(false),
        tasks: Arc::new(DriverTaskTracker::new()),
    });
    let options = EventOptions::default().global(true);
    let session_start = futures::executor::block_on(ctx.on(
        "agent/session-start",
        session_start_listener(driver.clone()),
        options.clone(),
    ));
    let error = futures::executor::block_on(ctx.on(
        "agent/error",
        error_listener(driver.clone()),
        options.clone(),
    ));
    let goal_changed = futures::executor::block_on(ctx.on(
        "goal/changed",
        goal_changed_listener(driver.clone()),
        options.clone(),
    ));
    let inserted = futures::executor::block_on(ctx.on(
        "agent/inbox/inserted",
        inserted_listener(driver.clone()),
        options.clone(),
    ));
    let discarded = futures::executor::block_on(ctx.on(
        "agent/inbox/discarded",
        discarded_listener(driver.clone()),
        options.clone(),
    ));
    let claimed = futures::executor::block_on(ctx.on(
        "agent/inbox/claimed",
        claimed_listener(driver.clone()),
        options.clone(),
    ));
    let session = futures::executor::block_on(ctx.on(
        "session/event",
        session_listener(driver.clone()),
        options.clone(),
    ));
    let pre_step = futures::executor::block_on(ctx.on(
        "agent/pre-step",
        pre_step_listener(driver.clone()),
        EventOptions::default().global(true).prepend(true),
    ));
    let status = futures::executor::block_on(ctx.on(
        "agent/status",
        status_listener(driver.clone()),
        options.clone(),
    ));
    let disposed = futures::executor::block_on(ctx.on(
        "agent/disposed",
        disposed_listener(driver.clone()),
        options,
    ));
    for agent in driver.agents.list() {
        driver.state_for(agent.clone());
        driver.disarm(&agent);
    }
    let closed = Arc::new(AtomicBool::new(false));
    let cleanup = make_disposer(move || {
        let driver = driver.clone();
        let session_start = session_start.clone();
        let error = error.clone();
        let goal_changed = goal_changed.clone();
        let inserted = inserted.clone();
        let discarded = discarded.clone();
        let claimed = claimed.clone();
        let session = session.clone();
        let pre_step = pre_step.clone();
        let status = status.clone();
        let disposed = disposed.clone();
        let closed = closed.clone();
        Box::pin(async move {
            if closed.swap(true, Ordering::SeqCst) {
                return;
            }
            driver.stopping.store(true, Ordering::SeqCst);
            let states = driver.states.lock().values().cloned().collect::<Vec<_>>();
            let mut waits = Vec::new();
            let mut retire = Vec::new();
            for state in &states {
                let attempt = {
                    let mut slot = state.attempt.lock();
                    let attempt = slot.clone();
                    if let Some(current) = slot.as_mut() {
                        current.stale = true;
                    }
                    attempt
                };
                driver.disarm(&state.agent);
                if let Some(attempt) = attempt {
                    let owns_running_turn = matches!(
                        attempt.phase,
                        AttemptPhase::Claimed | AttemptPhase::Admitted
                    ) && !attempt.stale
                        && !attempt.cancelled
                        && state.agent.status() == AgentStatus::Running;
                    if owns_running_turn {
                        state.agent.cancel(
                            dsh_agent::AgentCancelCause::Disposed,
                            Some(&CancelOptions { keep_inbox: true }),
                        );
                        waits.push(state.agent.clone());
                    } else {
                        retire.push((state.clone(), attempt));
                    }
                }
            }
            for (state, attempt) in retire {
                retire_unowned_attempt(&state, attempt).await;
            }
            for agent in waits {
                agent.when_idle().await;
            }
            driver.tasks.stop_and_drain().await;
            disposed().await;
            status().await;
            pre_step().await;
            session().await;
            claimed().await;
            discarded().await;
            inserted().await;
            goal_changed().await;
            error().await;
            session_start().await;
        })
    });
    let effect_cleanup = cleanup.clone();
    let _ = ctx.effect(
        "goal-round-driver lifecycle",
        Box::pin(async move { Some(effect_cleanup) }),
    );
    Ok(cleanup)
}
