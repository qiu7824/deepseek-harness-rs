//! Package-owned durable goal-stream invariants. Rust port of
//! `packages/goal/goal/src/invariant.ts`.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, InjectSpec, Plugin, PluginError, downcast};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{Session, SessionEvent};

use crate::fold::{GoalFoldState, apply_goal_event, empty_goal_fold_state};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-goal";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "goal-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

fn session_key(session: &Session) -> usize {
    session.identity()
}

/// Apply one event through the strict goal decoder and attribute failures.
fn apply_checked(
    state: &mut GoalFoldState,
    event: &SessionEvent,
    fail: &Arc<dyn Fn(&str) + Send + Sync>,
) {
    if let Err(error) = apply_goal_event(state, event) {
        fail(&format!(
            "session event {} violates the durable goal stream: {error}",
            event.seq
        ));
    }
}

/// Install an independent incremental fold over every attached session (TS
/// `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let states: Arc<parking_lot::Mutex<HashMap<usize, GoalFoldState>>> =
                    Arc::new(parking_lot::Mutex::new(HashMap::new()));
                let staged: Arc<parking_lot::Mutex<HashMap<(usize, u64), GoalFoldState>>> =
                    Arc::new(parking_lot::Mutex::new(HashMap::new()));

                let seed = |session: &Session,
                            states: &parking_lot::Mutex<HashMap<usize, GoalFoldState>>,
                            fail: &Arc<dyn Fn(&str) + Send + Sync>| {
                    let mut state = empty_goal_fold_state();
                    for event in session.events().iter() {
                        apply_checked(&mut state, event, fail);
                    }
                    states.lock().insert(session_key(session), state);
                };
                // Seed every attached session.
                if let Some(store) = ctx
                    .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        seed(&session, &states, &fail);
                    }
                }

                // Seed sessions created later.
                let states_for_created = states.clone();
                let fail_for_created = fail.clone();
                let created_listener: Arc<cordis::Listener> =
                    Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
                        let states = states_for_created.clone();
                        let fail = fail_for_created.clone();
                        Box::pin(async move {
                            if let Some(session) =
                                args.first().and_then(|value| downcast::<Session>(value))
                            {
                                seed(session, &states, &fail);
                            }
                            None
                        })
                    });
                ctx.on(
                    "session/created",
                    created_listener,
                    EventOptions::default().global(true),
                )
                .await;

                // Validate each event before publication, staging the
                // candidate fold.
                let staged_for_dispatch = staged.clone();
                let fail_for_dispatch = fail.clone();
                let dispatch_listener: Arc<cordis::Listener> =
                    Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
                        let event_name = args
                            .get(1)
                            .and_then(|value| value.downcast_ref::<String>())
                            .cloned();
                        let event_args = args
                            .get(2)
                            .and_then(|value| downcast::<Vec<ArcValue>>(value))
                            .cloned();
                        let staged = staged_for_dispatch.clone();
                        let fail = fail_for_dispatch.clone();
                        Box::pin(async move {
                            if event_name.as_deref() != Some("session/event") {
                                return None;
                            }
                            let Some(event_args) = event_args else {
                                return None;
                            };
                            let session = event_args
                                .first()
                                .and_then(|value| downcast::<Session>(value))
                                .cloned();
                            let event = event_args
                                .get(1)
                                .and_then(|value| downcast::<SessionEvent>(value))
                                .cloned();
                            let prefix = event_args
                                .get(2)
                                .and_then(|value| downcast::<Arc<Vec<SessionEvent>>>(value))
                                .cloned();
                            let (Some(session), Some(event), Some(prefix)) =
                                (session, event, prefix)
                            else {
                                return None;
                            };
                            let key = session_key(&session);
                            // `internal/dispatch` runs inside the Session
                            // append lock. The third event argument is the
                            // exact immutable prefix before this candidate;
                            // replay it directly instead of relying on the
                            // async session/created cache.
                            let mut state = empty_goal_fold_state();
                            for prior in prefix.iter() {
                                apply_checked(&mut state, prior, &fail);
                            }
                            apply_checked(&mut state, &event, &fail);
                            staged.lock().insert((key, event.seq), state);
                            None
                        })
                    });
                ctx.on(
                    "internal/dispatch",
                    dispatch_listener,
                    EventOptions::default().global(true),
                )
                .await;

                // Commit the staged fold at publication.
                let states_for_commit = states.clone();
                let staged_for_commit = staged.clone();
                let fail_for_commit = fail.clone();
                let commit_listener: Arc<cordis::Listener> = Arc::new(
                    move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
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
                            let candidate = staged.lock().remove(&(key, event.seq));
                            let Some(state) = candidate else {
                                fail(
                                    "session/event reached publication without matching goal-fold validation",
                                );
                                return None;
                            };
                            states.lock().insert(key, state);
                            None
                        })
                    },
                );
                ctx.on(
                    "session/event",
                    commit_listener,
                    EventOptions::default().global(true),
                )
                .await;
            })
        }),
    }
}

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the goal invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct GoalInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for GoalInvariantPlugin {
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
