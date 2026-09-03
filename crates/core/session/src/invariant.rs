//! Package-owned relational invariants for the session event log. Rust port
#![allow(clippy::type_complexity, clippy::unnecessary_unwrap)]
// Callback graph aliases would obscure the invariant transitions; checked Options remain in diagnostics.
//! of `packages/core/session/src/invariant.ts`.
//!
//! Load this companion beside `dsh-invariants` to enable the checks. The
//! staged-transition table is keyed by `(session identity, event seq)`
//! instead of the TS event-object `WeakMap` (Rust log entries have no stable
//! object identity).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

use crate::types::SessionEvent;
use crate::{Session, SessionStore, TOOL_NOT_STARTED};
use cordis::{
    ArcValue, BoxFuture, Context, Disposer, EventOptions, InjectSpec, Listener, downcast,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use parking_lot::Mutex;

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-session";

/// Cordis companion plugin name.
pub const NAME: &str = "session-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Per-session bookkeeping for relational log checks.
#[derive(Debug, Clone)]
struct SessionTrace {
    last_seq: i64,
    open_turn: Option<u64>,
    open_step: Option<u64>,
    next_turn: u64,
    next_step: u64,
    pending_calls: HashSet<String>,
}

/// One accepted event's deferred mutation of a committed session trace.
#[derive(Debug, Clone)]
enum PendingTransition {
    None,
    Add(String),
    Delete(String),
    Clear,
}

/// One accepted event's deferred mutation of a committed session trace.
#[derive(Debug, Clone)]
struct SessionTraceTransition {
    last_seq: i64,
    open_turn: Option<u64>,
    open_step: Option<u64>,
    next_turn: u64,
    next_step: u64,
    pending_calls: PendingTransition,
}

fn fresh_trace() -> SessionTrace {
    SessionTrace {
        last_seq: -1,
        open_turn: None,
        open_step: None,
        next_turn: 1,
        next_step: 1,
        pending_calls: HashSet::new(),
    }
}

/// Report an invariant violation (always aborts).
fn invariant_fail(fail: &dyn Fn(&str), message: String) -> ! {
    fail(&message);
    unreachable!("invariant failure must abort: {message}")
}

/// Assert that a step-scoped event names the currently open turn and step.
fn require_open_step(trace: &SessionTrace, kind: &str, turn: u64, step: u64, fail: &dyn Fn(&str)) {
    if trace.open_turn != Some(turn) || trace.open_step != Some(step) {
        invariant_fail(
            fail,
            format!(
                "{kind} names turn {turn}/step {step} but open is turn {}/step {}",
                render_option(trace.open_turn),
                render_option(trace.open_step)
            ),
        );
    }
}

fn render_option(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string())
}

/// Validate one candidate event without mutating the committed trace
/// (TS `validateEvent`).
fn validate_event(
    trace: &SessionTrace,
    event: &SessionEvent,
    fail: &dyn Fn(&str),
) -> SessionTraceTransition {
    if (event.seq.get() as i64) <= trace.last_seq {
        invariant_fail(
            fail,
            format!(
                "seq must strictly increase: saw {} after {}",
                event.seq, trace.last_seq
            ),
        );
    }
    let mut open_turn = trace.open_turn;
    let mut open_step = trace.open_step;
    let mut next_turn = trace.next_turn;
    let mut next_step = trace.next_step;
    let mut pending_calls = PendingTransition::None;

    let data_turn = || event.data.get("turn").and_then(|value| value.as_u64());
    let data_step = || event.data.get("step").and_then(|value| value.as_u64());

    match event.type_.as_str() {
        "turn/start" => {
            let turn = data_turn().unwrap_or(0);
            if trace.open_turn.is_some() {
                invariant_fail(
                    fail,
                    format!(
                        "turn/start {turn} while turn {} is still open",
                        trace.open_turn.unwrap()
                    ),
                );
            }
            if turn != trace.next_turn {
                invariant_fail(
                    fail,
                    format!("turn/start expected turn {}, got {turn}", trace.next_turn),
                );
            }
            open_turn = Some(turn);
            next_step = 1;
        }
        "turn/end" => {
            let turn = data_turn().unwrap_or(0);
            if trace.open_turn != Some(turn) {
                invariant_fail(
                    fail,
                    format!(
                        "turn/end {turn} does not match open turn {}",
                        render_option(trace.open_turn)
                    ),
                );
            }
            if trace.open_step.is_some() {
                invariant_fail(
                    fail,
                    format!(
                        "turn/end {turn} while step {} is still open",
                        trace.open_step.unwrap()
                    ),
                );
            }
            open_turn = None;
            next_turn += 1;
        }
        "step/start" => {
            let turn = data_turn().unwrap_or(0);
            let step = data_step().unwrap_or(0);
            if trace.open_turn != Some(turn) {
                invariant_fail(
                    fail,
                    format!(
                        "step/start in turn {turn} but open turn is {}",
                        render_option(trace.open_turn)
                    ),
                );
            }
            if trace.open_step.is_some() {
                invariant_fail(
                    fail,
                    format!(
                        "step/start {step} while step {} is still open",
                        trace.open_step.unwrap()
                    ),
                );
            }
            if step != trace.next_step {
                invariant_fail(
                    fail,
                    format!(
                        "step/start expected step {} in turn {turn}, got {step}",
                        trace.next_step
                    ),
                );
            }
            open_step = Some(step);
        }
        "step/end" => {
            let turn = data_turn().unwrap_or(0);
            let step = data_step().unwrap_or(0);
            require_open_step(trace, "step/end", turn, step, fail);
            pending_calls = PendingTransition::Clear;
            open_step = None;
            next_step += 1;
        }
        "assistant/chunk" => {
            let turn = data_turn().unwrap_or(0);
            let step = data_step().unwrap_or(0);
            require_open_step(trace, "assistant/chunk", turn, step, fail);
        }
        "assistant/message" => {
            let turn = data_turn().unwrap_or(0);
            let step = data_step().unwrap_or(0);
            require_open_step(trace, "assistant/message", turn, step, fail);
        }
        "tool/call" => {
            let turn = data_turn().unwrap_or(0);
            let step = data_step().unwrap_or(0);
            require_open_step(trace, "tool/call", turn, step, fail);
            let call_id = event
                .data
                .get("callId")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            pending_calls = PendingTransition::Add(call_id);
        }
        "tool/result" => {
            // Session has already validated a content rewrite that cites its
            // replaced event; it is durable turn work, not a second
            // execution of the original call.
            if !matches!(event.surface_op, Some(crate::types::SurfaceOp::Append)) {
                if trace.open_turn.is_none() {
                    invariant_fail(
                        fail,
                        "tool/result surface replacement appended outside any open turn"
                            .to_string(),
                    );
                }
            } else {
                let turn = data_turn().unwrap_or(0);
                let step = data_step().unwrap_or(0);
                require_open_step(trace, "tool/result", turn, step, fail);
                let call_id = event
                    .data
                    .get("message")
                    .and_then(|value| value.get("source"))
                    .and_then(|value| value.get("callId"))
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let synthetic_not_started = event
                    .data
                    .get("message")
                    .and_then(|value| value.get("content"))
                    .and_then(|value| value.as_array())
                    .and_then(|content| content.first())
                    .and_then(|block| block.get("isError"))
                    .and_then(|value| value.as_bool())
                    == Some(true)
                    && event
                        .data
                        .get("error")
                        .and_then(|value| value.get("code"))
                        .and_then(|value| value.as_str())
                        == Some(TOOL_NOT_STARTED);
                if !trace.pending_calls.contains(&call_id) && !synthetic_not_started {
                    invariant_fail(
                        fail,
                        format!("tool/result for {call_id} with no prior tool/call in this step"),
                    );
                }
                pending_calls = PendingTransition::Delete(call_id);
            }
        }
        "user/message" | "session/end-seed" => {}
        "todo/write" | "request/header" | "request/context" if trace.open_turn.is_none() => {
            invariant_fail(
                fail,
                format!(
                    "{} appended outside any open turn (core execution events must be turn-enclosed)",
                    event.type_
                ),
            );
        }
        // Merge-extensible event relations belong to their owning plugin.
        _ => {}
    }
    SessionTraceTransition {
        last_seq: event.seq.get() as i64,
        open_turn,
        open_step,
        next_turn,
        next_step,
        pending_calls,
    }
}

/// Apply one already-validated transition after its event commits.
fn apply_transition(trace: &mut SessionTrace, transition: SessionTraceTransition) {
    trace.last_seq = transition.last_seq;
    trace.open_turn = transition.open_turn;
    trace.open_step = transition.open_step;
    trace.next_turn = transition.next_turn;
    trace.next_step = transition.next_step;
    match transition.pending_calls {
        PendingTransition::None => {}
        PendingTransition::Add(call_id) => {
            trace.pending_calls.insert(call_id);
        }
        PendingTransition::Delete(call_id) => {
            trace.pending_calls.remove(&call_id);
        }
        PendingTransition::Clear => trace.pending_calls.clear(),
    }
}

/// One staged pre-commit validation awaiting its publication.
#[derive(Debug, Clone)]
struct StagedTransition {
    session: Session,
    /// Installation/reconciliation gaps rebuild after publication, when the
    /// session lock is no longer held. The steady state validates in O(1).
    rebuild_after_publish: bool,
}

fn session_ptr(session: &Session) -> usize {
    Arc::as_ptr(&session.inner) as *const () as usize
}

/// Install the session contribution into its child registration fiber
/// (TS `install`).
async fn install_inner(ctx: &Context, fail: &(dyn Fn(&str) + Send + Sync)) {
    // Traces keyed by session identity, each guarded by a weak session
    // reference: a recycled allocation address must never observe a dead
    // session's trace (the TS `WeakMap` contract).
    let traces: Arc<Mutex<HashMap<usize, (Weak<crate::store::SessionInner>, SessionTrace)>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let staged: Arc<Mutex<HashMap<(usize, u64), StagedTransition>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let seed_session: Arc<dyn Fn(&Session, &dyn Fn(&str)) + Send + Sync> = {
        let traces = Arc::clone(&traces);
        Arc::new(move |session: &Session, fail: &dyn Fn(&str)| {
            let mut trace = fresh_trace();
            for event in session.events().iter() {
                let transition = validate_event(&trace, event, fail);
                apply_transition(&mut trace, transition);
            }
            traces.lock().insert(
                session_ptr(session),
                (Arc::downgrade(&session.inner), trace),
            );
        })
    };
    // Seed the live sessions that predate this registration.
    let sessions = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .expect("sessions service required");
    for session in sessions.list() {
        seed_session(&session, fail);
    }

    let seed_session_created = Arc::clone(&seed_session);
    {
        let listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let seed_session_created = Arc::clone(&seed_session_created);
            Box::pin(async move {
                seed_session_created(&session, &|message| panic!("{message}"));
                None
            })
        });
        ctx.on(
            "session/created",
            listener,
            EventOptions::default().global(true),
        )
        .await;
    }

    {
        let staged = Arc::clone(&staged);
        let traces_for_publish = Arc::clone(&traces);
        let seed_after_publish = Arc::clone(&seed_session);
        let listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let event = downcast::<SessionEvent>(&args[1])
                .expect("event arg")
                .clone();
            let staged = Arc::clone(&staged);
            let traces = Arc::clone(&traces_for_publish);
            let seed_after_publish = Arc::clone(&seed_after_publish);
            Box::pin(async move {
                let key = (session_ptr(&session), event.seq.get());
                let entry = { staged.lock().remove(&key) };
                match entry {
                    Some(entry) if session_ptr(&entry.session) == session_ptr(&session) => {
                        if entry.rebuild_after_publish {
                            seed_after_publish(&session, &|message| panic!("{message}"));
                        } else {
                            let trace_is_current = traces
                                .lock()
                                .get(&session_ptr(&session))
                                .is_some_and(|(_, trace)| trace.last_seq == event.seq.get() as i64);
                            if !trace_is_current {
                                seed_after_publish(&session, &|message| panic!("{message}"));
                            }
                        }
                    }
                    _ => panic!(
                        "session/event reached publication without matching pre-commit validation"
                    ),
                }
                None
            })
        });
        ctx.on(
            "session/event",
            listener,
            EventOptions::default().global(true),
        )
        .await;
    }

    {
        // Pre-commit validation: the internal/dispatch pre-hook runs inline
        // inside `Session::append` BEFORE the listener snapshot resolves.
        // Because this port contains internal-listener panics (the TS veto
        // path cannot cancel the append), the validated transition commits
        // at stage time — the same sequential order the TS synchronous
        // session/event observers give the committed trace.
        let traces = Arc::clone(&traces);
        let staged = Arc::clone(&staged);
        let listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
            let event_name = downcast::<String>(&args[1]).cloned().unwrap_or_default();
            if event_name != "session/event" {
                return Box::pin(async { None });
            }
            let dispatch_args = downcast::<Vec<ArcValue>>(&args[2])
                .cloned()
                .unwrap_or_default();
            let Some(session) = dispatch_args
                .first()
                .and_then(|value| downcast::<Session>(value).cloned())
            else {
                return Box::pin(async { None });
            };
            let Some(event) = dispatch_args
                .get(1)
                .and_then(|value| downcast::<SessionEvent>(value).cloned())
            else {
                return Box::pin(async { None });
            };
            let traces = Arc::clone(&traces);
            let staged = Arc::clone(&staged);
            Box::pin(async move {
                let ptr = session_ptr(&session);
                let trace = traces
                    .lock()
                    .get(&ptr)
                    .and_then(|(weak, trace)| weak.upgrade().is_some().then(|| trace.clone()));
                let rebuild_after_publish = trace
                    .as_ref()
                    .is_none_or(|trace| trace.last_seq + 1 != event.seq.get() as i64);
                if let Some(mut trace) = trace.filter(|_| !rebuild_after_publish) {
                    let transition = validate_event(&trace, &event, &|message| panic!("{message}"));
                    apply_transition(&mut trace, transition);
                    traces
                        .lock()
                        .insert(ptr, (Arc::downgrade(&session.inner), trace));
                }
                staged.lock().insert(
                    (ptr, event.seq.get()),
                    StagedTransition {
                        session,
                        rebuild_after_publish,
                    },
                );
                None
            })
        });
        ctx.on(
            "internal/dispatch",
            listener,
            EventOptions::default().global(true),
        )
        .await;
    }
}

/// Register the session invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> BoxFuture<'static, Disposer> {
    let ctx = ctx.clone();
    Box::pin(async move {
        let invariants = ctx
            .get_typed::<Arc<InvariantRegistry>>("invariants", false)
            .expect("invariants service required by session-invariant");
        invariants.register(
            &ctx,
            PACKAGE_NAME,
            InvariantInstaller {
                install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
                    let ctx = ctx.clone();
                    Box::pin(async move { install_inner(&ctx, &*fail).await })
                }),
                inject: Some(InjectSpec::new(["sessions"])),
            },
        )
    })
}
