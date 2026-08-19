//! Package-owned durable goal-round prompt invariants.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_goal::{GoalActivation, GoalPhase, GoalView, fold_goal};
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
pub fn validate_goal_round_event(
    prior: &[SessionEvent],
    event: &SessionEvent,
) -> Result<(), String> {
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

    let folded = fold_goal(prior).map_err(|error| {
        format!("cannot reconstruct the goal before a continuation message: {error}")
    })?;
    let goal = folded.goal.ok_or_else(|| {
        format!("goal round {round} cannot be reconstructed from the preceding durable goal state")
    })?;
    let reconstructable = folded.created_at.is_some()
        && folded.updated_at.is_some()
        && goal.phase == GoalPhase::Active
        && goal.id.as_str() == goal_id
        && goal.revision == *revision
        && *round == folded.rounds_started + 1
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
        rounds_started: folded.rounds_started,
        created_at: folded.created_at.expect("checked"),
        updated_at: folded.updated_at.expect("checked"),
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

pub fn validate_session(session: &Session) -> Result<(), String> {
    let events = session.events();
    for (index, event) in events.iter().enumerate() {
        validate_goal_round_event(&events[..index], event)?;
    }
    Ok(())
}

fn seed_session(
    session: &Session,
    histories: &Mutex<HashMap<usize, Vec<SessionEvent>>>,
    fail: &Arc<dyn Fn(&str) + Send + Sync>,
) {
    if let Err(error) = validate_session(session) {
        fail(&error);
    }
    histories.lock().insert(
        session_key(session),
        session.events().iter().cloned().collect(),
    );
}

async fn install_event_checks(
    ctx: &Context,
    histories: Arc<Mutex<HashMap<usize, Vec<SessionEvent>>>>,
    fail: Arc<dyn Fn(&str) + Send + Sync>,
) {
    let fail_for_dispatch = fail.clone();
    let dispatch: Arc<Listener> = Arc::new(move |_ctx, args| {
        let event_name = args
            .get(1)
            .and_then(|value| downcast::<String>(value))
            .cloned()
            .unwrap_or_default();
        let event_args = args.get(2).and_then(downcast_arc::<Vec<ArcValue>>);
        let fail = fail_for_dispatch.clone();
        Box::pin(async move {
            if event_name != "session/event" {
                return None;
            }
            let event_args = event_args?;
            let session = event_args
                .first()
                .and_then(|value| downcast::<Session>(value));
            let event = event_args
                .get(1)
                .and_then(|value| downcast::<SessionEvent>(value));
            let prefix = event_args
                .get(2)
                .and_then(|value| downcast::<Arc<Vec<SessionEvent>>>(value))
                .cloned();
            let (Some(_session), Some(event), Some(prefix)) = (session, event, prefix) else {
                return None;
            };
            if let Err(error) = validate_goal_round_event(prefix.as_ref(), event) {
                fail(&error);
            }
            None
        })
    });
    ctx.on(
        "internal/dispatch",
        dispatch,
        EventOptions::default().global(true),
    )
    .await;

    let commit: Arc<Listener> = Arc::new(move |_ctx, args| {
        let histories = histories.clone();
        Box::pin(async move {
            let session = args.first().and_then(|value| downcast::<Session>(value));
            let event = args
                .get(1)
                .and_then(|value| downcast::<SessionEvent>(value));
            if let (Some(session), Some(event)) = (session, event) {
                histories
                    .lock()
                    .entry(session_key(session))
                    .or_default()
                    .push(event.clone());
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
                let histories: Arc<Mutex<HashMap<usize, Vec<SessionEvent>>>> =
                    Arc::new(Mutex::new(HashMap::new()));
                // Close publication gaps before snapshotting any existing
                // session. The final reconciliation snapshot subsumes events
                // observed while installation is still in progress.
                install_event_checks(&ctx, histories.clone(), fail.clone()).await;

                let histories_for_created = histories.clone();
                let fail_for_created = fail.clone();
                let created: Arc<Listener> = Arc::new(move |_ctx, args| {
                    let histories = histories_for_created.clone();
                    let fail = fail_for_created.clone();
                    Box::pin(async move {
                        if let Some(session) =
                            args.first().and_then(|value| downcast::<Session>(value))
                        {
                            seed_session(session, &histories, &fail);
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
                        seed_session(&session, &histories, &fail);
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
