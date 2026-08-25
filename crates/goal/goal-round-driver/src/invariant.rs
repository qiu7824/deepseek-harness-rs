//! Package-owned durable goal-round prompt invariants.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_goal::{
    GoalActivation, GoalFoldState, GoalPhase, GoalView, apply_goal_event, empty_goal_fold_state,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_llm::{MessageSource, UserMessage};
use dsh_session::{Session, SessionEvent};
use parking_lot::Mutex;

pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-goal-round-driver";
pub const NAME: &str = "goal-round-driver-invariant";
pub const INJECT: [&str; 1] = ["invariants"];

fn session_key(session: &Session) -> usize {
    session.identity()
}

/// Validate one positive goal-round message against its exact durable prefix.
fn validate_goal_round_state(state: &GoalFoldState, event: &SessionEvent) -> Result<(), String> {
    if event.type_ != "user/message" {
        return Ok(());
    }
    let message: UserMessage = serde_json::from_value(event.data.clone())
        .map_err(|error| format!("goal round message is invalid: {error}"))?;
    let MessageSource::Goal {
        goal_id,
        revision,
        round,
    } = &message.source
    else {
        return Ok(());
    };
    if *round == 0 {
        return Ok(());
    }

    let goal = state.goal.clone().ok_or_else(|| {
        format!("goal round {round} cannot be reconstructed from the preceding durable goal state")
    })?;
    let reconstructable = state.created_at.is_some()
        && state.updated_at.is_some()
        && goal.phase == GoalPhase::Active
        && goal.id.as_str() == goal_id
        && goal.revision == *revision
        && *round == state.rounds_started + 1
        && *round <= goal.max_goal_rounds;
    if !reconstructable {
        return Err(format!(
            "goal round {round} cannot be reconstructed from the preceding durable goal state"
        ));
    }
    let view = GoalView {
        id: goal.id,
        revision: goal.revision,
        objective: goal.objective,
        phase: goal.phase,
        blocked_reason: goal.blocked_reason,
        max_goal_rounds: goal.max_goal_rounds,
        rounds_started: state.rounds_started,
        created_at: state.created_at.expect("checked"),
        updated_at: state.updated_at.expect("checked"),
        activation: GoalActivation::Armed,
    };
    let expected = crate::render_goal_round_prompt(&view, *round);
    if message.content != expected {
        return Err(format!(
            "goal round {round} content does not match the package-owned continuation prompt"
        ));
    }
    Ok(())
}

pub fn validate_goal_round_event(
    prior: &[SessionEvent],
    event: &SessionEvent,
) -> Result<(), String> {
    let mut state = empty_goal_fold_state();
    for prior_event in prior {
        apply_goal_event(&mut state, prior_event).map_err(|error| {
            format!("cannot reconstruct the goal before a continuation message: {error}")
        })?;
    }
    validate_goal_round_state(&state, event)
}

pub fn validate_session(session: &Session) -> Result<(), String> {
    let events = session.events();
    let mut state = empty_goal_fold_state();
    for event in events.iter() {
        validate_goal_round_state(&state, event)?;
        apply_goal_event(&mut state, event)?;
    }
    Ok(())
}

fn seed_session(
    session: &Session,
    states: &Mutex<HashMap<usize, GoalFoldState>>,
    fail: &Arc<dyn Fn(&str) + Send + Sync>,
) {
    let events = session.events();
    let mut state = empty_goal_fold_state();
    for event in events.iter() {
        if let Err(error) = validate_goal_round_state(&state, event) {
            fail(&error);
        }
        if let Err(error) = apply_goal_event(&mut state, event) {
            fail(&error);
        }
    }
    states.lock().insert(session_key(session), state);
}

async fn install_event_checks(
    ctx: &Context,
    states: Arc<Mutex<HashMap<usize, GoalFoldState>>>,
    fail: Arc<dyn Fn(&str) + Send + Sync>,
) {
    let staged: Arc<Mutex<HashMap<(usize, u64), GoalFoldState>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let states_for_dispatch = states.clone();
    let staged_for_dispatch = staged.clone();
    let fail_for_dispatch = fail.clone();
    let dispatch: Arc<Listener> = Arc::new(move |_ctx, args| {
        let event_name = args
            .get(1)
            .and_then(|value| downcast::<String>(value))
            .cloned()
            .unwrap_or_default();
        let event_args = args.get(2).and_then(downcast_arc::<Vec<ArcValue>>);
        let states = states_for_dispatch.clone();
        let staged = staged_for_dispatch.clone();
        let fail = fail_for_dispatch.clone();
        Box::pin(async move {
            if event_name != "session/event" {
                return None;
            }
            let event_args = event_args?;
            let session = event_args
                .first()
                .and_then(|value| downcast::<Session>(value))
                .cloned()?;
            let event = event_args
                .get(1)
                .and_then(|value| downcast::<SessionEvent>(value))
                .cloned()?;
            let key = session_key(&session);
            let Some(mut state) = states.lock().get(&key).cloned() else {
                return None;
            };
            if let Err(error) = validate_goal_round_state(&state, &event) {
                fail(&error);
            }
            if let Err(error) = apply_goal_event(&mut state, &event) {
                fail(&error);
            }
            staged.lock().insert((key, event.seq), state);
            None
        })
    });
    ctx.on(
        "internal/dispatch",
        dispatch,
        EventOptions::default().global(true),
    )
    .await;

    let states_for_commit = states.clone();
    let staged_for_commit = staged.clone();
    let fail_for_commit = fail.clone();
    let commit: Arc<Listener> = Arc::new(move |_ctx, args| {
        let states = states_for_commit.clone();
        let staged = staged_for_commit.clone();
        let fail = fail_for_commit.clone();
        Box::pin(async move {
            let session = args
                .first()
                .and_then(|value| downcast::<Session>(value))
                .cloned();
            let event = args
                .get(1)
                .and_then(|value| downcast::<SessionEvent>(value))
                .cloned();
            let (Some(session), Some(event)) = (session, event) else {
                return None;
            };
            let key = session_key(&session);
            if let Some(state) = staged.lock().remove(&(key, event.seq)) {
                states.lock().insert(key, state);
            } else {
                seed_session(&session, &states, &fail);
            }
            None
        })
    });
    ctx.on(
        "session/event",
        commit,
        EventOptions::default().global(true),
    )
    .await;
}

pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let states: Arc<Mutex<HashMap<usize, GoalFoldState>>> =
                    Arc::new(Mutex::new(HashMap::new()));
                // Close publication gaps before snapshotting any existing
                // session. The final reconciliation snapshot subsumes events
                // observed while installation is still in progress.
                install_event_checks(&ctx, states.clone(), fail.clone()).await;

                let states_for_created = states.clone();
                let fail_for_created = fail.clone();
                let created: Arc<Listener> = Arc::new(move |_ctx, args| {
                    let states = states_for_created.clone();
                    let fail = fail_for_created.clone();
                    Box::pin(async move {
                        if let Some(session) =
                            args.first().and_then(|value| downcast::<Session>(value))
                        {
                            seed_session(session, &states, &fail);
                        }
                        None
                    })
                });
                ctx.on(
                    "session/created",
                    created,
                    EventOptions::default().global(true),
                )
                .await;

                if let Some(store) = ctx
                    .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        seed_session(&session, &states, &fail);
                    }
                }
            })
        }),
    }
}

pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the goal-round-driver invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

pub struct GoalRoundDriverInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for GoalRoundDriverInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
