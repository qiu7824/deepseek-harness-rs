//! Package-owned invariant companion for `@deepseek-ai/dsh-commands`:
//! command lifecycle events pair by commandId within one session log. Rust
//! port of `packages/interaction/commands/src/invariant.ts`.
//!
//! # Deviations
//!
//! - The `internal/dispatch` pre-hook runs while `Session::append` holds the
//!   session state lock, so the companion keeps its own per-session event
//!   history (keyed by session id) instead of re-reading `session.events()`
//!   inside the pre-hook (the established session-invariant pattern).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{Session, SessionEvent};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-commands";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "commands-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

type Histories = parking_lot::Mutex<HashMap<String, Vec<SessionEvent>>>;
type RunIds = parking_lot::Mutex<HashMap<String, HashSet<String>>>;

#[derive(Default)]
struct CommandTrace {
    end: u64,
    command_seqs: HashSet<u64>,
}
impl CommandTrace {
    fn observe(&mut self, event: &SessionEvent) {
        self.end = event.seq.get() + 1;
        if matches!(event.type_.as_str(), "command/run" | "command/done") {
            self.command_seqs.insert(event.seq.get());
        }
    }
    fn valid_source(&self, seq: u64) -> bool {
        seq < self.end && !self.command_seqs.contains(&seq)
    }
}
type Traces = parking_lot::Mutex<HashMap<String, CommandTrace>>;

/// Validate one lifecycle event against the incremental session history (TS
/// `validateEvent`; failures carry the exact TS messages).
pub fn validate_event(
    histories: &Histories,
    run_ids: &RunIds,
    session_id: &str,
    event: &SessionEvent,
) -> Result<(), String> {
    validate_with_source(run_ids, session_id, event, |source| {
        histories.lock().get(session_id).is_some_and(|history| {
            history.get(source as usize).is_some_and(|source_event| {
                source_event.seq == source
                    && !matches!(source_event.type_.as_str(), "command/run" | "command/done")
            })
        })
    })
}

fn validate_with_source(
    run_ids: &RunIds,
    session_id: &str,
    event: &SessionEvent,
    valid_source: impl Fn(u64) -> bool,
) -> Result<(), String> {
    let command_id = event
        .data
        .get("commandId")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    match event.type_.as_str() {
        "command/run" => {
            let mut run_ids = run_ids.lock();
            let ids = run_ids.entry(session_id.to_string()).or_default();
            if !ids.insert(command_id.to_string()) {
                return Err(format!(
                    "command/run repeats commandId {}",
                    serde_json::to_string(command_id).expect("id")
                ));
            }
            Ok(())
        }
        "command/done" => {
            let paired = run_ids
                .lock()
                .get(session_id)
                .is_some_and(|ids| ids.contains(command_id));
            if !paired {
                return Err(format!(
                    "command/done {} pairs no prior command/run in this log",
                    serde_json::to_string(command_id).expect("id")
                ));
            }
            let source = event
                .data
                .get("sourceEventSeq")
                .and_then(|value| value.as_u64());
            if let Some(source) = source {
                let kind = event
                    .data
                    .get("kind")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let valid_source =
                    kind == "success" && source < event.seq.get() && valid_source(source);
                if !valid_source {
                    return Err(format!(
                        "command/done {} has invalid sourceEventSeq {}",
                        serde_json::to_string(command_id).expect("id"),
                        source
                    ));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Build the installer registered under [`PACKAGE_NAME`] (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let histories: Arc<Traces> = Arc::new(parking_lot::Mutex::new(HashMap::new()));
                let run_ids: Arc<RunIds> = Arc::new(parking_lot::Mutex::new(HashMap::new()));

                let seed = |session: &Session,
                            histories: &Traces,
                            run_ids: &RunIds,
                            fail: &Arc<dyn Fn(&str) + Send + Sync>| {
                    let mut trace = CommandTrace::default();
                    run_ids.lock().remove(session.id().as_str());
                    let replayed = session.visit_events(0, None, |event| {
                        if let Err(message) =
                            validate_with_source(run_ids, session.id().as_str(), event, |seq| {
                                trace.valid_source(seq)
                            })
                        {
                            fail(&message);
                        }
                        trace.observe(event);
                        Ok(true)
                    });
                    if let Err(error) = replayed {
                        fail(&error);
                        return;
                    }
                    histories
                        .lock()
                        .insert(session.id().as_str().to_string(), trace);
                };

                // Seed every attached session.
                if let Some(store) = ctx
                    .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        seed(&session, &histories, &run_ids, &fail);
                    }
                }

                let created_histories = histories.clone();
                let created_ids = run_ids.clone();
                let created_fail = fail.clone();
                ctx.on(
                    "session/created",
                    Arc::new(move |_, args| {
                        let histories = created_histories.clone();
                        let run_ids = created_ids.clone();
                        let fail = created_fail.clone();
                        Box::pin(async move {
                            if let Some(session) = args.first().and_then(downcast::<Session>) {
                                seed(session, &histories, &run_ids, &fail);
                            }
                            None
                        })
                    }),
                    EventOptions::default().global(true),
                )
                .await;

                let committed = histories.clone();
                ctx.on(
                    "session/event",
                    Arc::new(move |_, args| {
                        let histories = committed.clone();
                        Box::pin(async move {
                            if let (Some(session), Some(event)) = (
                                args.first().and_then(downcast::<Session>),
                                args.get(1).and_then(downcast::<SessionEvent>),
                            ) {
                                histories
                                    .lock()
                                    .entry(session.id().to_string())
                                    .or_default()
                                    .observe(event);
                            }
                            None
                        })
                    }),
                    EventOptions::default().global(true),
                )
                .await;

                // Validate each event before publication from the
                // incremental history (no session-state read inside the
                // pre-hook).
                let histories_for_dispatch = histories.clone();
                let run_ids_for_dispatch = run_ids.clone();
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
                    let histories = histories_for_dispatch.clone();
                    let run_ids = run_ids_for_dispatch.clone();
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
                        if event.type_ != "command/run" && event.type_ != "command/done" {
                            return None;
                        }
                        if let Err(message) =
                            validate_with_source(&run_ids, session.id().as_str(), &event, |seq| {
                                histories
                                    .lock()
                                    .get(session.id().as_str())
                                    .is_some_and(|trace| trace.valid_source(seq))
                            })
                        {
                            fail(&message);
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
        .expect("the commands invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct CommandsInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for CommandsInvariantPlugin {
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
