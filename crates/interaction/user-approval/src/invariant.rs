//! Package-owned approval audit-stream invariants: `approval/asked` and
//! `approval/decided` pair by id inside one open turn, and the policy
//! vocabulary stays closed. Rust port of
//! `packages/interaction/user-approval/src/invariant.ts`.
//!
//! # Deviations
//!
//! - The `internal/dispatch` pre-hook runs while `Session::append` holds the
//!   session state lock, so the companion keeps its own per-session trace
//!   (keyed by session id) instead of re-reading `session.events()` inside
//!   the pre-hook (the established session-invariant pattern).
//! - The TS companion stages validated transitions at pre-commit and applies
//!   them at the `session/event` post-commit hook; this port validates AND
//!   applies in the pre-hook (the commands-companion pattern), because the
//!   port contains internal-listener panics instead of vetoing the append.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{Session, SessionEvent};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-user-approval";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "user-approval-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

const APPROVAL_OUTCOMES: &[&str] = &["allowed-once", "rejected", "cancelled", "unavailable"];
const APPROVAL_POLICIES: &[&str] = &["ask", "never"];

/// Per-session trace (TS `ApprovalTrace`): the open turn and the pending
/// asked/decided pairings.
#[derive(Default)]
pub struct ApprovalTrace {
    open_turn: Option<u64>,
    pending: HashSet<String>,
}

pub type Traces = parking_lot::Mutex<HashMap<String, ApprovalTrace>>;

/// Validate one approval event against the per-session trace and apply the
/// accepted pairing transition (TS `validateApprovalEvent` +
/// `applyApprovalTransition`; failures carry the exact TS messages).
pub fn validate_event(
    traces: &Traces,
    session_id: &str,
    event: &SessionEvent,
) -> Result<(), String> {
    match event.type_.as_str() {
        "approval/asked" => {
            let id = event
                .data
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let tool_name = event
                .data
                .get("toolName")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let mut traces = traces.lock();
            let trace = traces.entry(session_id.to_string()).or_default();
            if trace.open_turn.is_none() {
                return Err("approval/asked appended outside any open turn".to_string());
            }
            if tool_name.is_empty() {
                return Err("approval/asked toolName must be non-empty".to_string());
            }
            if trace.pending.contains(id) {
                return Err(format!(
                    "approval/asked repeated open id {}",
                    serde_json::to_string(id).expect("id")
                ));
            }
            trace.pending.insert(id.to_string());
            Ok(())
        }
        "approval/decided" => {
            let id = event
                .data
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let mut traces = traces.lock();
            let trace = traces.entry(session_id.to_string()).or_default();
            if trace.open_turn.is_none() {
                return Err("approval/decided appended outside any open turn".to_string());
            }
            if !trace.pending.contains(id) {
                return Err(format!(
                    "approval/decided has no matching approval/asked for id {}",
                    serde_json::to_string(id).expect("id")
                ));
            }
            let outcome = event
                .data
                .get("outcome")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if !APPROVAL_OUTCOMES.contains(&outcome) {
                return Err(format!(
                    "approval/decided carries unknown outcome {}",
                    serde_json::to_string(outcome).expect("outcome")
                ));
            }
            trace.pending.remove(id);
            Ok(())
        }
        "approval/policy" => {
            let policy = event
                .data
                .get("policy")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if !APPROVAL_POLICIES.contains(&policy) {
                return Err(format!(
                    "approval/policy carries unknown policy {}",
                    serde_json::to_string(policy).expect("policy")
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Track the open-turn boundary in the per-session trace (TS post-hook turn
/// transitions; folded into the pre-hook here).
pub fn apply_turn(traces: &Traces, session_id: &str, event: &SessionEvent) {
    match event.type_.as_str() {
        "turn/start" => {
            let turn = event.data.get("turn").and_then(|value| value.as_u64());
            let mut traces = traces.lock();
            traces.entry(session_id.to_string()).or_default().open_turn = turn;
        }
        "turn/end" => {
            let mut traces = traces.lock();
            traces.entry(session_id.to_string()).or_default().open_turn = None;
        }
        _ => {}
    }
}

/// Build the installer registered under [`PACKAGE_NAME`] (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let traces: Arc<Traces> = Arc::new(parking_lot::Mutex::new(HashMap::new()));

                // Seed every attached session.
                if let Some(store) = ctx
                    .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        let events: Vec<SessionEvent> = session.events().iter().cloned().collect();
                        for event in &events {
                            apply_turn(&traces, session.id().as_str(), event);
                            if let Err(message) =
                                validate_event(&traces, session.id().as_str(), event)
                            {
                                fail(&message);
                            }
                        }
                    }
                }

                // Validate each approval event before publication from the
                // incremental trace (no session-state read inside the
                // pre-hook).
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
                        apply_turn(&traces, session.id().as_str(), &event);
                        if let Err(message) = validate_event(&traces, session.id().as_str(), &event)
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
        .expect("the user-approval invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct UserApprovalInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for UserApprovalInvariantPlugin {
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
