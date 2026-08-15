//! Package-owned strict Schedule stream invariant. Rust port of
//! `packages/schedule/schedule/src/invariant.ts`.
//!
//! # Deviations
//!
//! - The `internal/dispatch` pre-hook runs inline while `Session::append`
//!   holds the session state lock, so listeners must not call
//!   `session.events()` (deadlock). Instead the companion keeps an
//!   incremental per-session fold trace and validates each candidate
//!   `schedule/change` against it, which is equivalent to validating the
//!   candidate-extended stream.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{Session, SessionEvent, SessionStore};

use crate::domain::{FoldedSchedules, apply_change, decode_schedule_change, fold_schedule_events};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-schedule";

/// Cordis invariant-companion plugin name (TS `name`).
pub const NAME: &str = "tool-schedule-invariant";

/// Service required before reserving this package's invariant ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Validate a complete exact-session stream under its fork suffix policy.
pub fn validate(
    events: &[SessionEvent],
    seed_length: usize,
) -> Result<(), String> {
    fold_schedule_events(events, seed_length).map(|_| ()).map_err(|error| error.message)
}

/// Build the installer registered under [`PACKAGE_NAME`] (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                // Incremental per-session fold traces keyed by identity.
                let traces: Arc<parking_lot::Mutex<HashMap<usize, FoldedSchedules>>> =
                    Arc::new(parking_lot::Mutex::new(HashMap::new()));
                if let Some(store) = ctx
                    .get_typed::<Arc<SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        let events = session.events();
                        let folded = fold_schedule_events(
                            &events,
                            session.header().seed_length.unwrap_or(0) as usize,
                        );
                        match folded {
                            Ok(folded) => {
                                traces.lock().insert(session.identity(), folded);
                            }
                            Err(error) => fail(&error.message),
                        }
                    }
                }

                // Validate each schedule change before publication.
                let traces_for_dispatch = traces.clone();
                let fail_for_dispatch = fail.clone();
                let dispatch_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
                    let event_name = args
                        .get(1)
                        .and_then(|value| downcast::<String>(value))
                        .cloned()
                        .unwrap_or_default();
                    let event_args = args
                        .get(2)
                        .and_then(|value| downcast_arc::<Vec<ArcValue>>(value));
                    let traces = traces_for_dispatch.clone();
                    let fail = fail_for_dispatch.clone();
                    Box::pin(async move {
                        if event_name != "session/event" {
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
                        let (Some(session), Some(event)) = (session, event) else {
                            return None;
                        };
                        if event.type_ != "schedule/change" {
                            return None;
                        }
                        let change = match decode_schedule_change(&event.data) {
                            Ok(change) => change,
                            Err(error) => {
                                fail(&error.message);
                                return None;
                            }
                        };
                        let seed_length = session.header().seed_length.unwrap_or(0) as usize;
                        let mut traces = traces.lock();
                        let folded = traces.entry(session.identity()).or_insert_with(|| {
                            // A session created without a prior `session/created`
                            // seed folds from its current log (candidate-excluded).
                            fold_schedule_events(&[], seed_length)
                                .unwrap_or_default()
                        });
                        // The trace reflects committed events only; events at
                        // or below the seed boundary stay outside ownership.
                        if let Err(error) = apply_change(folded, &change) {
                            fail(&error.message);
                        }
                        None
                    })
                });
                ctx.on(
                    "internal/dispatch",
                    dispatch_listener,
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
        .expect("the schedule invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct ScheduleInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for ScheduleInvariantPlugin {
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
