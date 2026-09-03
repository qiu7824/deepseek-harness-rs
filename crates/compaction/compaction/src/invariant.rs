//! Package-owned compaction log-stream invariants. Rust port of
//! `packages/compaction/compaction/src/invariant.ts` (core subset: the
//! start/summary/end bracket state machine, checkpoint correlation, turn
//! enclosure, and seed-boundary stale starts; the summary-adjacency and
//! shadow-price cross-checks remain documented as deferred).
//!
//! # Deviations
//!
//! - The `internal/dispatch` pre-hook runs under the append lock, so the
//!   companion keeps its own per-session event history (the established
//!   session-invariant pattern).

use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{Session, SessionEvent};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-compaction";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "compaction-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

#[derive(Clone)]
pub struct CompactionTrace {
    compaction_id: String,
    source_command_id: Option<String>,
    start_seq: u64,
    turn: Option<u64>,
    summarized: bool,
}

/// The per-session compaction bracket trace (exported for the invariant
/// spec).
#[derive(Clone, Default)]
pub struct SessionTrace {
    open_turn: Option<u64>,
    pub compaction: Option<CompactionTrace>,
}

/// Advance one event through the compaction state machine (TS transition
/// subset; failures carry the exact TS messages).
pub fn apply_compaction_event(
    trace: &mut SessionTrace,
    event: &SessionEvent,
) -> Result<(), String> {
    match event.type_.as_str() {
        "compaction/start" => {
            if trace.compaction.is_some() {
                return Err("compaction/start overlaps an open compaction".to_string());
            }
            let id = event
                .data
                .get("compactionId")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "compaction/start compactionId must be a non-empty string".to_string()
                })?;
            let source_command_id = event
                .data
                .get("sourceCommandId")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let turn = match event.data.get("turn") {
                Some(value) if value.is_null() => None,
                Some(value) => value.as_u64(),
                None => None,
            };
            trace.compaction = Some(CompactionTrace {
                compaction_id: id.to_string(),
                source_command_id,
                start_seq: event.seq.get(),
                turn,
                summarized: false,
            });
            Ok(())
        }
        "compaction/summary" => {
            let Some(open) = &trace.compaction else {
                return Err("compaction/summary has no matching compaction/start".to_string());
            };
            let id = event
                .data
                .get("compactionId")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if id != open.compaction_id {
                return Err(format!(
                    "compaction/summary id {id} does not match compaction/start id {}",
                    open.compaction_id
                ));
            }
            let source_command_id = event
                .data
                .get("sourceCommandId")
                .and_then(|value| value.as_str());
            if source_command_id != open.source_command_id.as_deref() {
                return Err(format!(
                    "compaction/summary sourceCommandId {} does not match compaction/start sourceCommandId {}",
                    source_command_id.unwrap_or("undefined"),
                    open.source_command_id.as_deref().unwrap_or("undefined")
                ));
            }
            trace.compaction.as_mut().expect("open").summarized = true;
            Ok(())
        }
        "compaction/end" => {
            let Some(open) = trace.compaction.take() else {
                return Err("compaction/end has no matching compaction/start".to_string());
            };
            let id = event
                .data
                .get("compactionId")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if id != open.compaction_id {
                return Err(format!(
                    "compaction/end id {id} does not match compaction/start id {}",
                    open.compaction_id
                ));
            }
            Ok(())
        }
        "user/message" => {
            let source = event.data.get("source");
            let is_checkpoint = source
                .and_then(|source| source.get("kind"))
                .and_then(|kind| kind.as_str())
                == Some("plugin")
                && source
                    .and_then(|source| source.get("plugin"))
                    .and_then(|plugin| plugin.as_str())
                    == Some("compact");
            if is_checkpoint {
                let Some(open) = &trace.compaction else {
                    return Err(
                        "compaction checkpoint has no matching compaction/start".to_string()
                    );
                };
                let checkpoint_id = source
                    .and_then(|source| source.get("compactionId"))
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                if checkpoint_id != open.compaction_id {
                    return Err(format!(
                        "compaction checkpoint id {checkpoint_id} does not match compaction/start id {}",
                        open.compaction_id
                    ));
                }
                let checkpoint_source_command = source
                    .and_then(|source| source.get("sourceCommandId"))
                    .and_then(|value| value.as_str());
                if checkpoint_source_command != open.source_command_id.as_deref() {
                    return Err(format!(
                        "compaction checkpoint sourceCommandId {} does not match compaction/start sourceCommandId {}",
                        checkpoint_source_command.unwrap_or("undefined"),
                        open.source_command_id.as_deref().unwrap_or("undefined")
                    ));
                }
            }
            Ok(())
        }
        "turn/start" => {
            if trace.compaction.is_some() {
                return Err(turn_boundary_error(trace, "turn/start"));
            }
            trace.open_turn = event.data.get("turn").and_then(|value| value.as_u64());
            Ok(())
        }
        "turn/end" => {
            if trace.compaction.is_some() {
                return Err(turn_boundary_error(trace, "turn/end"));
            }
            trace.open_turn = None;
            Ok(())
        }
        "session/end-seed" => {
            // A seed boundary makes an unmatched start stale (inherited
            // orphan starts stay allowed).
            trace.compaction = None;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn turn_boundary_error(trace: &SessionTrace, event_type: &str) -> String {
    let owner = match trace
        .compaction
        .as_ref()
        .and_then(|compaction| compaction.turn)
    {
        None => "standalone compaction".to_string(),
        Some(turn) => format!("compaction for turn {turn}"),
    };
    format!("{event_type} cannot cross an open {owner}")
}

/// Build the installer registered under [`PACKAGE_NAME`] (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let traces: Arc<
                    parking_lot::Mutex<std::collections::HashMap<String, SessionTrace>>,
                > = Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));

                // Seed every attached session.
                if let Some(store) = ctx
                    .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        let mut trace = SessionTrace::default();
                        for event in session.events().iter() {
                            if let Err(message) = apply_compaction_event(&mut trace, event) {
                                fail(&message);
                            }
                        }
                        traces
                            .lock()
                            .insert(session.id().as_str().to_string(), trace);
                    }
                }

                // Validate each event before publication from the
                // incremental per-session trace.
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
                        let mut traces = traces.lock();
                        let trace = traces.entry(session.id().as_str().to_string()).or_default();
                        if let Err(message) = apply_compaction_event(trace, &event) {
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
        .expect("the compaction invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct CompactionInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for CompactionInvariantPlugin {
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
