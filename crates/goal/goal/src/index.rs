//! Same-session goal domain: event-sourced state, compare-and-set mutations,
//! and process-local continuation activation. Rust port of
//! `packages/goal/goal/src/index.ts`.
//!
//! # Deviations
//!
//! - The `@Remote` annotations collapse (the typert remote runtime is a
//!   later milestone); `remote_export_create` is kept as the plain method.
//! - The `goal` projection-unit registration (the `ctx.inject` child) is
//!   omitted; [`apply_goal_projection`] ships as the pure last-wins fold the
//!   unit runs.
//! - The config schema validation collapses into [`Config`] field checks.
//! - The session cache is a strong map keyed by session identity (the TS
//!   `WeakMap` weak-key semantics collapse; entries live with the service).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use cordis::{ArcValue, Context, EventOptions, Service, arc};
use dsh_agent::{Agent, AgentRegistry, emit_agent_event};
use dsh_session::{Session, SessionEvent};
use parking_lot::Mutex;
use serde_json::Value;

use crate::domain::{
    GoalChangeMeta, GoalChanged, GoalChangedPayload, GoalClearChangeMeta, GoalError, GoalErrorCode,
    GoalOperation, GoalSnapshotChangeMeta,
};
use crate::fold::{
    GoalFoldState, apply_goal_event, decode_goal_change, empty_goal_fold_state, goal_change_ref,
};
use crate::runtime::GOAL_CHANGE_VERSION;
use crate::types::{
    CreateGoalRequest, CreateGoalResult, EditGoalRequest, GoalActivation, GoalBlockReason,
    GoalPhase, GoalProjection, GoalRef, GoalSnapshot, GoalView, goal_id,
};

pub use crate::types::{
    CreateGoalRequest as CreateGoalRequestType, CreateGoalResult as CreateGoalResultType,
    EditGoalRequest as EditGoalRequestType, GoalActivation as GoalActivationType,
    GoalBlockReason as GoalBlockReasonType, GoalPhase as GoalPhaseType,
    GoalProjection as GoalProjectionType, GoalRef as GoalRefType, GoalSnapshot as GoalSnapshotType,
    GoalView as GoalViewType,
};

/// Deployment default for goal creation (TS `Config`).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Total rounds used when a create request omits its own cap.
    pub default_max_goal_rounds: Option<u64>,
}

/// Resolved defaults.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// Validated positive round cap.
    pub default_max_goal_rounds: u64,
}

/// The default round cap when the composition entry omits one.
pub const DEFAULT_MAX_GOAL_ROUNDS: u64 = 256;

/// Light last-wins fold of the `goal` projection unit (TS
/// `applyGoalProjection`). Any non-goal or malformed event returns the
/// previous state.
pub fn apply_goal_projection(
    state: &Option<GoalProjection>,
    event: &SessionEvent,
) -> Option<GoalProjection> {
    if event.type_ != "goal/change" {
        return state.clone();
    }
    let change = match decode_goal_change(&event.data) {
        Ok(Some(change)) => change,
        _ => return state.clone(),
    };
    match change {
        GoalChangeMeta::Clear(_) => None,
        GoalChangeMeta::Snapshot(change) => Some(GoalProjection {
            goal: change.goal,
            rounds_started: change.rounds_started,
            created_at: change.created_at,
            updated_at: change.updated_at,
        }),
    }
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Process-local cache plus activation intent crossing the synchronous
/// append boundary (TS `GoalCache`).
#[derive(Clone)]
struct GoalCache {
    state: GoalFoldState,
    activation: GoalActivation,
    observed_seq: usize,
    pending_activation: Option<(u64, GoalActivation)>,
}

/// Stable process-local state for one exact Session object.
struct SessionGoalState {
    session: Session,
    cache: Mutex<GoalCache>,
    mutating: AtomicBool,
    disarm_requested: AtomicBool,
}

/// RAII ownership of the single in-flight mutation for one Session.
struct MutationClaim {
    state: Arc<SessionGoalState>,
    active: bool,
}

impl MutationClaim {
    fn acquire(state: Arc<SessionGoalState>) -> Result<Self, GoalError> {
        state
            .mutating
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                GoalError::new(
                    format!(
                        "a goal mutation is already in progress for session \"{}\"",
                        state.session.id()
                    ),
                    GoalErrorCode::CommitFailed,
                )
            })?;
        Ok(Self {
            state,
            active: true,
        })
    }

    fn publish(mut self, mut cache: GoalCache) -> GoalCache {
        {
            let mut shared = self.state.cache.lock();
            if self.state.disarm_requested.swap(false, Ordering::AcqRel) {
                cache.activation = GoalActivation::Disarmed;
            }
            *shared = cache.clone();
            self.state.mutating.store(false, Ordering::Release);
            if self.state.disarm_requested.swap(false, Ordering::AcqRel) {
                shared.activation = GoalActivation::Disarmed;
                cache.activation = GoalActivation::Disarmed;
            }
        }
        self.active = false;
        cache
    }

    fn release(&mut self) {
        if !self.active {
            return;
        }
        {
            let mut cache = self.state.cache.lock();
            if self.state.disarm_requested.swap(false, Ordering::AcqRel) {
                cache.activation = GoalActivation::Disarmed;
            }
            self.state.mutating.store(false, Ordering::Release);
        }
        self.active = false;
    }
}

impl Drop for MutationClaim {
    fn drop(&mut self) {
        self.release();
    }
}

fn resolve_max_goal_rounds(value: u64) -> Result<u64, GoalError> {
    if value < 1 {
        return Err(GoalError::new(
            "maxGoalRounds must be a positive safe integer",
            GoalErrorCode::InvalidMaxRounds,
        ));
    }
    Ok(value)
}

fn resolve_objective(value: &str) -> Result<String, GoalError> {
    if value.trim().is_empty() {
        return Err(GoalError::new(
            "goal objective must be a non-empty string",
            GoalErrorCode::InvalidObjective,
        ));
    }
    Ok(value.trim().to_string())
}

fn resolve_block_reason(reason: &GoalBlockReason) -> Result<GoalBlockReason, GoalError> {
    let kebab = regex::Regex::new(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$").expect("valid regex");
    if !kebab.is_match(&reason.code) || reason.message.trim().is_empty() {
        return Err(GoalError::new(
            "goal block reason requires a lower-kebab-case code and a non-empty message",
            GoalErrorCode::InvalidBlockReason,
        ));
    }
    Ok(GoalBlockReason {
        code: reason.code.clone(),
        message: reason.message.trim().to_string(),
    })
}

/// Goal service (`ctx.goals`) backed exclusively by the owning session log
/// (TS `GoalService`).
pub struct GoalService {
    pub ctx: Context,
    resolved: ResolvedConfig,
    caches: Mutex<HashMap<usize, Arc<SessionGoalState>>>,
}

impl Service for GoalService {
    fn service_name(&self) -> &'static str {
        "goals"
    }
}

impl GoalService {
    /// Construct, validate, register as `ctx.goals`, and wire the
    /// session-start disarm edge (the TS constructor collapse).
    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let default_max_goal_rounds = config
            .default_max_goal_rounds
            .unwrap_or(DEFAULT_MAX_GOAL_ROUNDS);
        if default_max_goal_rounds == 0 {
            panic!("maxGoalRounds must be a positive safe integer");
        }
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            resolved: ResolvedConfig {
                default_max_goal_rounds,
            },
            caches: Mutex::new(HashMap::new()),
        });
        let service_for_listener = service.clone();
        let listener: Arc<cordis::Listener> =
            Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
                let service = service_for_listener.clone();
                Box::pin(async move {
                    let agent = args
                        .first()
                        .and_then(|value| {
                            value.downcast_ref::<dsh_agent::AgentSessionStartPayload>()
                        })
                        .map(|payload| payload.agent.clone());
                    if let Some(agent) = agent {
                        let _ = service.disarm(&agent);
                    }
                    None
                })
            });
        let _ = futures::executor::block_on(ctx.on(
            "agent/session-start",
            listener,
            EventOptions::default().global(true),
        ));
        ctx.register_service(service.clone());
        service
    }

    /// Read the current goal for one exact live agent.
    pub fn get(&self, agent: &Arc<dyn Agent>) -> Result<Option<GoalView>, GoalError> {
        self.assert_live(agent)?;
        let state = self.state_for(agent);
        let mut cache = state.cache.lock();
        self.sync(&state.session, &mut cache);
        Ok(self.view(&cache))
    }

    /// Remove process-local continuation authority without changing durable
    /// goal phase or revision.
    pub fn disarm(&self, agent: &Arc<dyn Agent>) -> Result<Option<GoalView>, GoalError> {
        self.assert_live(agent)?;
        let state = self.state_for(agent);
        state.disarm_requested.store(true, Ordering::Release);
        let mut cache = state.cache.lock();
        self.sync(&state.session, &mut cache);
        cache.activation = GoalActivation::Disarmed;
        let view = self.view(&cache);
        if !state.mutating.load(Ordering::Acquire) {
            state.disarm_requested.store(false, Ordering::Release);
        }
        Ok(view)
    }

    /// Create and arm a goal. A completed goal may be replaced; every other
    /// current phase must be cleared or resumed instead.
    pub fn create(
        &self,
        agent: &Arc<dyn Agent>,
        request: CreateGoalRequest,
    ) -> Result<GoalView, GoalError> {
        let objective = resolve_objective(&request.objective)?;
        let max_goal_rounds = resolve_max_goal_rounds(
            request
                .max_goal_rounds
                .unwrap_or(self.resolved.default_max_goal_rounds),
        )?;
        let (state, cache, claim) = self.prepare_mutation(agent)?;
        if let Some(current) = &cache.state.goal {
            if current.phase != GoalPhase::Complete {
                return Err(GoalError::new(
                    format!(
                        "goal \"{}\" already exists with phase \"{}\"",
                        current.id,
                        current.phase.as_str()
                    ),
                    GoalErrorCode::AlreadyExists,
                ));
            }
        }
        let now = epoch_ms();
        let goal = GoalSnapshot {
            id: goal_id(format!("goal-{}", uuid::Uuid::new_v4())),
            revision: 1,
            objective,
            phase: GoalPhase::Active,
            max_goal_rounds,
            blocked_reason: None,
        };
        self.commit_snapshot(
            agent,
            &state,
            claim,
            cache,
            GoalOperation::Create,
            goal,
            0,
            now,
            now,
            GoalActivation::Armed,
        )
    }

    /// Edit objective and/or round cap without changing phase.
    pub fn edit(
        &self,
        agent: &Arc<dyn Agent>,
        ref_: &GoalRef,
        request: &EditGoalRequest,
    ) -> Result<GoalView, GoalError> {
        let (state, cache, claim) = self.prepare_mutation(agent)?;
        let current = self.expect_current(&cache, ref_)?.clone();
        if request.objective.is_none() && request.max_goal_rounds.is_none() {
            return Err(GoalError::new(
                "goal edit requires objective and/or maxGoalRounds",
                GoalErrorCode::InvalidEdit,
            ));
        }
        let goal = GoalSnapshot {
            revision: current.revision + 1,
            objective: match &request.objective {
                Some(objective) => resolve_objective(objective)?,
                None => current.objective,
            },
            max_goal_rounds: match request.max_goal_rounds {
                Some(rounds) => resolve_max_goal_rounds(rounds)?,
                None => current.max_goal_rounds,
            },
            ..current
        };
        let activation = cache.activation;
        self.commit_current(
            agent,
            &state,
            claim,
            cache,
            GoalOperation::Edit,
            goal,
            activation,
        )
    }

    /// Pause an active goal and disarm automatic continuation.
    pub fn pause(&self, agent: &Arc<dyn Agent>, ref_: &GoalRef) -> Result<GoalView, GoalError> {
        self.transition(
            agent,
            ref_,
            GoalOperation::Pause,
            &[GoalPhase::Active],
            GoalPhase::Paused,
            GoalActivation::Disarmed,
        )
    }

    /// Resume and arm a stopped goal, or rearm an active goal after a
    /// session-start edge, while its round budget still has capacity.
    pub fn resume(&self, agent: &Arc<dyn Agent>, ref_: &GoalRef) -> Result<GoalView, GoalError> {
        let (state, cache, claim) = self.prepare_mutation(agent)?;
        let current = self.expect_current(&cache, ref_)?.clone();
        let resumable = matches!(
            current.phase,
            GoalPhase::Active | GoalPhase::Paused | GoalPhase::Blocked
        );
        if !resumable {
            return Err(self.transition_error(
                &current,
                GoalOperation::Resume,
                &["active", "paused", "blocked"],
            ));
        }
        if current.phase == GoalPhase::Active && cache.activation == GoalActivation::Armed {
            return Err(GoalError::new(
                format!("goal \"{}\" is already active and armed", current.id),
                GoalErrorCode::InvalidTransition,
            ));
        }
        if cache.state.rounds_started >= current.max_goal_rounds {
            return Err(GoalError::new(
                format!(
                    "goal \"{}\" exhausted {} goal rounds; increase maxGoalRounds before resuming",
                    current.id, current.max_goal_rounds
                ),
                GoalErrorCode::InvalidTransition,
            ));
        }
        let goal = self.with_phase(&current, GoalPhase::Active);
        self.commit_current(
            agent,
            &state,
            claim,
            cache,
            GoalOperation::Resume,
            goal,
            GoalActivation::Armed,
        )
    }

    /// Mark a current non-complete goal complete and disarm it.
    pub fn complete(&self, agent: &Arc<dyn Agent>, ref_: &GoalRef) -> Result<GoalView, GoalError> {
        self.transition(
            agent,
            ref_,
            GoalOperation::Complete,
            &[GoalPhase::Active, GoalPhase::Paused, GoalPhase::Blocked],
            GoalPhase::Complete,
            GoalActivation::Disarmed,
        )
    }

    /// Mark an active goal blocked and disarm it.
    pub fn block(
        &self,
        agent: &Arc<dyn Agent>,
        ref_: &GoalRef,
        reason: &GoalBlockReason,
    ) -> Result<GoalView, GoalError> {
        let (state, cache, claim) = self.prepare_mutation(agent)?;
        let current = self.expect_current(&cache, ref_)?.clone();
        if current.phase != GoalPhase::Active {
            return Err(self.transition_error(&current, GoalOperation::Block, &["active"]));
        }
        let mut goal = self.with_phase(&current, GoalPhase::Blocked);
        goal.blocked_reason = Some(resolve_block_reason(reason)?);
        self.commit_current(
            agent,
            &state,
            claim,
            cache,
            GoalOperation::Block,
            goal,
            GoalActivation::Disarmed,
        )
    }

    /// Clear the current goal while retaining a durable tombstone and
    /// history.
    pub fn clear(&self, agent: &Arc<dyn Agent>, ref_: &GoalRef) -> Result<GoalRef, GoalError> {
        let (state, mut cache, claim) = self.prepare_mutation(agent)?;
        let current = self.expect_current(&cache, ref_)?.clone();
        let tombstone = GoalRef {
            id: current.id,
            revision: current.revision + 1,
        };
        let change = GoalChangeMeta::Clear(GoalClearChangeMeta {
            cleared: tombstone.clone(),
            cleared_at: self.next_mutation_time(&cache)?,
        });
        self.commit(
            &state.session,
            &mut cache,
            &change,
            GoalActivation::Disarmed,
        )?;
        let cache = claim.publish(cache);
        self.emit_changed(agent, &change, &cache);
        Ok(tombstone)
    }

    /// Claim one session's mutation lane and clone its synchronized cache so
    /// synchronous append observers may read or disarm without deadlocking.
    fn prepare_mutation(
        &self,
        agent: &Arc<dyn Agent>,
    ) -> Result<(Arc<SessionGoalState>, GoalCache, MutationClaim), GoalError> {
        self.assert_live(agent)?;
        let state = self.state_for(agent);
        let claim = MutationClaim::acquire(state.clone())?;
        let mut cache = state.cache.lock().clone();
        self.sync(&state.session, &mut cache);
        Ok((state, cache, claim))
    }

    fn expect_current<'a>(
        &self,
        cache: &'a GoalCache,
        ref_: &GoalRef,
    ) -> Result<&'a GoalSnapshot, GoalError> {
        let current = cache
            .state
            .goal
            .as_ref()
            .ok_or_else(|| GoalError::new("no current goal", GoalErrorCode::NotFound))?;
        if ref_.id != current.id || ref_.revision != current.revision {
            return Err(GoalError::new(
                format!(
                    "stale goal ref \"{}\" revision {}; current is \"{}\" revision {}",
                    ref_.id, ref_.revision, current.id, current.revision
                ),
                GoalErrorCode::StaleRevision,
            ));
        }
        Ok(current)
    }

    /// Enforce exact live-agent identity rather than trusting a matching id.
    fn assert_live(&self, agent: &Arc<dyn Agent>) -> Result<(), GoalError> {
        let live = self
            .ctx
            .get_typed::<Arc<AgentRegistry>>("agents", false)
            .map(|slot| slot.as_ref().clone())
            .and_then(|registry| registry.get(agent.id()));
        if !live
            .as_ref()
            .is_some_and(|registered| Arc::ptr_eq(registered, agent))
        {
            return Err(GoalError::new(
                format!("agent \"{}\" is not live in this registry", agent.id()),
                GoalErrorCode::AgentNotLive,
            ));
        }
        Ok(())
    }

    /// Return the stable state owned by one exact Session object, folding a
    /// seed once with continuation authority disarmed.
    fn state_for(&self, agent: &Arc<dyn Agent>) -> Arc<SessionGoalState> {
        let session = agent.session().clone();
        let key = session.identity();
        let mut states = self.caches.lock();
        if let Some(state) = states.get(&key)
            && state.session.ptr_eq(&session)
        {
            return state.clone();
        }

        let mut fold = empty_goal_fold_state();
        for event in session.events().iter() {
            let _ = apply_goal_event(&mut fold, event);
        }
        let state = Arc::new(SessionGoalState {
            cache: Mutex::new(GoalCache {
                state: fold,
                activation: GoalActivation::Disarmed,
                observed_seq: session.seq(),
                pending_activation: None,
            }),
            session,
            mutating: AtomicBool::new(false),
            disarm_requested: AtomicBool::new(false),
        });
        states.insert(key, state.clone());
        state
    }

    /// Incrementally observe durable events and reconcile local activation
    /// intent.
    fn sync(&self, session: &Session, cache: &mut GoalCache) {
        let events = session.events();
        while cache.observed_seq < events.len() {
            let event = &events[cache.observed_seq];
            let _ = apply_goal_event(&mut cache.state, event);
            if event.type_ == "goal/change" {
                cache.activation = match &cache.pending_activation {
                    Some((seq, activation)) if *seq == event.seq => *activation,
                    _ => GoalActivation::Disarmed,
                };
            }
            cache.observed_seq += 1;
        }
    }

    /// Build a new revision with one replacement phase.
    fn with_phase(&self, current: &GoalSnapshot, phase: GoalPhase) -> GoalSnapshot {
        GoalSnapshot {
            id: current.id.clone(),
            revision: current.revision + 1,
            objective: current.objective.clone(),
            phase,
            max_goal_rounds: current.max_goal_rounds,
            blocked_reason: None,
        }
    }

    /// Shared validated phase transition.
    fn transition(
        &self,
        agent: &Arc<dyn Agent>,
        ref_: &GoalRef,
        operation: GoalOperation,
        allowed: &[GoalPhase],
        phase: GoalPhase,
        activation: GoalActivation,
    ) -> Result<GoalView, GoalError> {
        let (state, cache, claim) = self.prepare_mutation(agent)?;
        let current = self.expect_current(&cache, ref_)?.clone();
        if !allowed.contains(&current.phase) {
            return Err(self.transition_error(
                &current,
                operation,
                &["active", "paused", "blocked"],
            ));
        }
        let goal = self.with_phase(&current, phase);
        self.commit_current(agent, &state, claim, cache, operation, goal, activation)
    }

    fn transition_error(
        &self,
        current: &GoalSnapshot,
        operation: GoalOperation,
        allowed: &[&str],
    ) -> GoalError {
        GoalError::new(
            format!(
                "cannot {} goal \"{}\" from phase \"{}\"; expected {}",
                operation.as_str(),
                current.id,
                current.phase.as_str(),
                allowed.join(" or ")
            ),
            GoalErrorCode::InvalidTransition,
        )
    }

    /// Commit a mutation that retains the current goal's derived
    /// counters/times.
    fn commit_current(
        &self,
        agent: &Arc<dyn Agent>,
        state: &Arc<SessionGoalState>,
        claim: MutationClaim,
        cache: GoalCache,
        operation: GoalOperation,
        goal: GoalSnapshot,
        activation: GoalActivation,
    ) -> Result<GoalView, GoalError> {
        let created_at = cache.state.created_at.ok_or_else(|| {
            GoalError::new(
                "current goal cache lacks createdAt",
                GoalErrorCode::NotFound,
            )
        })?;
        let updated_at = self.next_mutation_time(&cache)?;
        let rounds_started = cache.state.rounds_started;
        self.commit_snapshot(
            agent,
            state,
            claim,
            cache,
            operation,
            goal,
            rounds_started,
            created_at,
            updated_at,
            activation,
        )
    }

    fn next_mutation_time(&self, cache: &GoalCache) -> Result<u64, GoalError> {
        let updated_at = cache.state.updated_at.ok_or_else(|| {
            GoalError::new(
                "current goal cache lacks updatedAt",
                GoalErrorCode::NotFound,
            )
        })?;
        Ok(epoch_ms().max(updated_at))
    }

    /// Build and commit one full-snapshot mutation.
    #[allow(clippy::too_many_arguments)]
    fn commit_snapshot(
        &self,
        agent: &Arc<dyn Agent>,
        state: &Arc<SessionGoalState>,
        claim: MutationClaim,
        mut cache: GoalCache,
        operation: GoalOperation,
        goal: GoalSnapshot,
        rounds_started: u64,
        created_at: u64,
        updated_at: u64,
        activation: GoalActivation,
    ) -> Result<GoalView, GoalError> {
        let change = GoalChangeMeta::Snapshot(GoalSnapshotChangeMeta {
            operation,
            goal,
            rounds_started,
            created_at,
            updated_at,
        });
        self.commit(&state.session, &mut cache, &change, activation)?;
        let cache = claim.publish(cache);
        let view = self.view(&cache).ok_or_else(|| {
            GoalError::new(
                "snapshot commit cleared the goal unexpectedly",
                GoalErrorCode::NotFound,
            )
        })?;
        self.emit_changed(agent, &change, &cache);
        Ok(view)
    }

    /// Commit one mutation into the durable log and local cache clone.
    fn commit(
        &self,
        session: &Session,
        cache: &mut GoalCache,
        change: &GoalChangeMeta,
        activation: GoalActivation,
    ) -> Result<(), GoalError> {
        cache.pending_activation = Some((session.seq() as u64, activation));
        let json = change_to_json(change);
        if let Err(error) = session.append("goal/change", json, None) {
            cache.pending_activation = None;
            return Err(GoalError::new(
                format!("failed to append durable goal change: {error}"),
                GoalErrorCode::CommitFailed,
            ));
        }
        self.sync(session, cache);
        cache.pending_activation = None;
        Ok(())
    }

    fn emit_changed(&self, agent: &Arc<dyn Agent>, change: &GoalChangeMeta, cache: &GoalCache) {
        let notification = GoalChanged {
            operation: change.operation(),
            ref_: goal_change_ref(change),
            goal: self.view(cache),
        };
        emit_agent_event(&self.ctx, agent, "goal/changed", move |agent| {
            arc(GoalChangedPayload {
                agent: agent.clone(),
                change: notification,
            })
        });
    }

    /// Build a detached current view.
    fn view(&self, cache: &GoalCache) -> Option<GoalView> {
        let goal = cache.state.goal.as_ref()?;
        let created_at = cache.state.created_at?;
        let updated_at = cache.state.updated_at?;
        Some(GoalView {
            id: goal.id.clone(),
            revision: goal.revision,
            objective: goal.objective.clone(),
            phase: goal.phase,
            blocked_reason: goal.blocked_reason.clone(),
            max_goal_rounds: goal.max_goal_rounds,
            rounds_started: cache.state.rounds_started,
            created_at,
            updated_at,
            activation: cache.activation,
        })
    }

    /// Create one Goal through the remote boundary (TS `remoteExportCreate`;
    /// the `@Remote` transport collapses).
    pub fn remote_export_create(
        &self,
        agent: &Arc<dyn Agent>,
        request: CreateGoalRequest,
    ) -> Result<CreateGoalResult, GoalError> {
        let view = self.create(agent, request)?;
        Ok(CreateGoalResult {
            ref_: GoalRef {
                id: view.id,
                revision: view.revision,
            },
        })
    }
}

/// Serialize one change meta into the durable `goal/change` payload.
fn change_to_json(change: &GoalChangeMeta) -> Value {
    match change {
        GoalChangeMeta::Snapshot(change) => {
            let mut goal = serde_json::Map::new();
            goal.insert(
                "id".to_string(),
                Value::String(change.goal.id.as_str().to_string()),
            );
            goal.insert("revision".to_string(), Value::from(change.goal.revision));
            goal.insert(
                "objective".to_string(),
                Value::String(change.goal.objective.clone()),
            );
            goal.insert(
                "phase".to_string(),
                Value::String(change.goal.phase.as_str().to_string()),
            );
            goal.insert(
                "maxGoalRounds".to_string(),
                Value::from(change.goal.max_goal_rounds),
            );
            if let Some(reason) = &change.goal.blocked_reason {
                let mut blocked = serde_json::Map::new();
                blocked.insert("code".to_string(), Value::String(reason.code.clone()));
                blocked.insert("message".to_string(), Value::String(reason.message.clone()));
                goal.insert("blockedReason".to_string(), Value::Object(blocked));
            }
            let mut object = serde_json::Map::new();
            object.insert("kind".to_string(), Value::String("goal/change".to_string()));
            object.insert("version".to_string(), Value::from(GOAL_CHANGE_VERSION));
            object.insert(
                "operation".to_string(),
                Value::String(change.operation.as_str().to_string()),
            );
            object.insert("goal".to_string(), Value::Object(goal));
            object.insert(
                "roundsStarted".to_string(),
                Value::from(change.rounds_started),
            );
            object.insert("createdAt".to_string(), Value::from(change.created_at));
            object.insert("updatedAt".to_string(), Value::from(change.updated_at));
            Value::Object(object)
        }
        GoalChangeMeta::Clear(clear) => {
            let mut cleared = serde_json::Map::new();
            cleared.insert(
                "id".to_string(),
                Value::String(clear.cleared.id.as_str().to_string()),
            );
            cleared.insert("revision".to_string(), Value::from(clear.cleared.revision));
            let mut object = serde_json::Map::new();
            object.insert("kind".to_string(), Value::String("goal/change".to_string()));
            object.insert("version".to_string(), Value::from(GOAL_CHANGE_VERSION));
            object.insert("operation".to_string(), Value::String("clear".to_string()));
            object.insert("cleared".to_string(), Value::Object(cleared));
            object.insert("clearedAt".to_string(), Value::from(clear.cleared_at));
            Value::Object(object)
        }
    }
}
