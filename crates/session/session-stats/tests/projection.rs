//! Rust port of `packages/session/session-stats/tests/projection.spec.ts`:
//! registry-drive behaviors plus the controlled-timestamp wall-time folds.

use std::sync::Arc;

use cordis::ArcValue;
use dsh_session::{Session, SessionEvent, SessionStore, session_id};
use dsh_session_projection::{ProjectionChangeListener, SessionProjectionRegistry};
use parking_lot::Mutex;
use serde_json::{Value, json};

use dsh_session_stats::session_stats_projection_definition;
use dsh_session_stats::types::SessionStatsProjection;

async fn harness(with_stats_plugin: bool) -> (cordis::Context, Arc<SessionProjectionRegistry>, Session) {
    let ctx = cordis::Context::root();
    let store = SessionStore::install(&ctx);
    let registry = SessionProjectionRegistry::install(&ctx);
    if with_stats_plugin {
        dsh_session_stats::apply(&ctx).unwrap();
    }
    let session = store
        .create(
            &ctx,
            Some(session_id("counted")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .unwrap();
    (ctx, registry, session)
}

/// Close one step; returns the counted `step/end` seq.
fn close_step(session: &Session, turn: u64, step: u64) -> u64 {
    session
        .append("step/start", json!({"turn": turn, "step": step}), None)
        .unwrap();
    session
        .append("step/end", json!({"turn": turn, "step": step}), None)
        .unwrap()
        .seq
}

/// Append the max-tokens usage-host shape: an assistant/message with empty
/// content.
fn append_empty_assistant_message(session: &Session, turn: u64, step: u64) {
    session
        .append(
            "assistant/message",
            json!({
                "turn": turn,
                "step": step,
                "message": {
                    "id": "usage-host",
                    "role": "assistant",
                    "content": [],
                    "source": {"kind": "model", "provider": "mock", "model": "mock"},
                },
            }),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: Some(vec![]),
            }),
        )
        .unwrap();
}

/// The all-zero projection value plus overrides (TS `totals`).
fn totals(overrides: impl FnOnce(&mut SessionStatsProjection)) -> SessionStatsProjection {
    let mut value = SessionStatsProjection::zero();
    overrides(&mut value);
    value
}

fn stats_value(snapshot_value: Option<&Value>) -> SessionStatsProjection {
    SessionStatsProjection::from_wire(snapshot_value.expect("sessionStats value")).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_zero_figures_on_the_empty_log() {
    let (_ctx, registry, session) = harness(true).await;
    let snapshot = registry.snapshot(&session);
    assert_eq!(stats_value(snapshot.values.get("sessionStats")), SessionStatsProjection::zero());
}

#[tokio::test(flavor = "multi_thread")]
async fn counts_distinct_turns_and_closed_steps_and_notifies_change_feed() {
    let (ctx, registry, session) = harness(true).await;
    let changes: Arc<Mutex<Vec<(String, Value, i64)>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let changes = Arc::clone(&changes);
        let listener: ProjectionChangeListener = Arc::new(move |_session, key, value, seq| {
            changes.lock().push((key.to_string(), value.clone(), seq));
        });
        registry.on_changed(&ctx, listener);
    }
    session.append("turn/start", json!({"turn": 1}), None).unwrap();
    let first_seq = close_step(&session, 1, 1);
    let second_seq = close_step(&session, 1, 2);
    session
        .append("turn/end", json!({"turn": 1, "reason": {"kind": "completed"}}), None)
        .unwrap();
    session.append("turn/start", json!({"turn": 2}), None).unwrap();
    let third_seq = close_step(&session, 2, 1);
    session
        .append("turn/end", json!({"turn": 2, "reason": {"kind": "completed"}}), None)
        .unwrap();

    let all = changes.lock().clone();
    assert!(all.iter().all(|(key, _, _)| key == "sessionStats"));
    let counted: Vec<(i64, Value)> = all
        .iter()
        .filter(|(_, value, seq)| {
            stats_value(Some(value)).steps > 0 || *seq == first_seq as i64
        })
        .map(|(_, value, seq)| (*seq, value.clone()))
        .collect();
    assert!(counted.contains(&(first_seq as i64, serde_json::to_value(totals(|t| {
        t.turns = 1;
        t.steps = 1;
    }))
    .unwrap())));
    let last = all.last().unwrap();
    assert_eq!(last.0, "sessionStats");
    assert_eq!(last.1, serde_json::to_value(totals(|t| {
        t.turns = 2;
        t.steps = 3;
    }))
    .unwrap());
    assert_eq!(last.2, third_seq as i64);

    let snapshot = registry.snapshot(&session);
    assert_eq!(stats_value(snapshot.values.get("sessionStats")), totals(|t| {
        t.turns = 2;
        t.steps = 3;
    }));
    assert_eq!(snapshot.as_of_seq, session.seq() as i64 - 1);
    assert!(all.iter().any(|(_, _, seq)| *seq == second_seq as i64));
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_count_a_rejected_or_empty_turn() {
    let (_ctx, registry, session) = harness(true).await;
    session.append("turn/start", json!({"turn": 1}), None).unwrap();
    session
        .append("turn/end", json!({"turn": 1, "reason": {"kind": "blocked"}}), None)
        .unwrap();
    assert_eq!(stats_value(registry.snapshot(&session).values.get("sessionStats")), SessionStatsProjection::zero());
}

#[tokio::test(flavor = "multi_thread")]
async fn counts_a_cancelled_step_that_closed_without_message() {
    let (_ctx, registry, session) = harness(true).await;
    session.append("turn/start", json!({"turn": 1}), None).unwrap();
    close_step(&session, 1, 1);
    session
        .append("turn/end", json!({"turn": 1, "reason": {"kind": "aborted", "reason": {"kind": "legacy"}}}), None)
        .unwrap();
    let value = stats_value(registry.snapshot(&session).values.get("sessionStats"));
    assert_eq!((value.turns, value.steps), (1, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn adds_no_extra_step_for_max_tokens_usage_host_message() {
    let (_ctx, registry, session) = harness(true).await;
    session.append("turn/start", json!({"turn": 1}), None).unwrap();
    session.append("step/start", json!({"turn": 1, "step": 1}), None).unwrap();
    append_empty_assistant_message(&session, 1, 1);
    session.append("step/end", json!({"turn": 1, "step": 1}), None).unwrap();
    session
        .append("turn/end", json!({"turn": 1, "reason": {"kind": "max-tokens"}}), None)
        .unwrap();
    let value = stats_value(registry.snapshot(&session).values.get("sessionStats"));
    assert_eq!((value.turns, value.steps, value.ttft_steps, value.decode_tokens), (1, 1, 0, 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn folds_steps_already_in_log_when_plugin_mounts_late() {
    let (ctx, registry, session) = harness(false).await;
    session.append("turn/start", json!({"turn": 1}), None).unwrap();
    close_step(&session, 1, 1);
    close_step(&session, 1, 2);
    session
        .append("turn/end", json!({"turn": 1, "reason": {"kind": "completed"}}), None)
        .unwrap();
    dsh_session_stats::apply(&ctx).unwrap();
    let value = stats_value(registry.snapshot(&session).values.get("sessionStats"));
    assert_eq!((value.turns, value.steps), (1, 2));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_key_without_plugin_and_dropped_when_plugin_unloads() {
    let (ctx, registry, session) = harness(false).await;
    assert!(!registry.snapshot(&session).values.contains_key("sessionStats"));
    let fiber = ctx.plugin(
        Arc::new(dsh_session_stats::StatsPlugin),
        cordis::arc(serde_json::Value::Null),
    );
    fiber.settle().await.unwrap();
    assert!(registry.snapshot(&session).values.contains_key("sessionStats"), "plugin registers the unit");
    // Unloading the plugin fiber removes the key (HMR safety): the
    // registration rides the plugin fiber's effect.
    fiber.dispose().await;
    assert!(!registry.snapshot(&session).values.contains_key("sessionStats"), "unload removes the key");
}

/// Build one synthetic committed event with a controlled timestamp (TS
/// `at`).
fn at(time: i64, type_: &str, data: Value) -> SessionEvent {
    SessionEvent {
        type_: type_.to_string(),
        seq: time as u64,
        time,
        data,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

fn message_json() -> Value {
    json!({
        "id": "m1",
        "role": "assistant",
        "content": [{"type": "text", "text": "answer"}],
        "source": {"kind": "model", "provider": "mock", "model": "mock"},
    })
}

/// Fold a synthetic event list through the definition and view the result
/// (TS `fold`).
fn fold(events: &[SessionEvent]) -> SessionStatsProjection {
    let definition = session_stats_projection_definition();
    let mut state: ArcValue = (definition.init)();
    for event in events {
        state = (definition.apply)(&state, event);
    }
    let viewed = (definition.view)(&state);
    let view: &Value = cordis::downcast(&viewed).expect("view value");
    SessionStatsProjection::from_wire(view).unwrap()
}

#[test]
fn accrues_model_ttft_and_decode_time_from_one_fully_recorded_step() {
    assert_eq!(
        fold(&[
            at(1_000, "step/start", json!({"turn": 1, "step": 1})),
            at(1_800, "assistant/chunk", json!({"turn": 1, "step": 1, "chunk": {"type": "text-delta", "index": 0, "text": "a"}})),
            at(4_800, "assistant/message", json!({"turn": 1, "step": 1, "message": message_json(), "usage": {"inputTokens": 10, "outputTokens": 60}})),
            at(4_900, "step/end", json!({"turn": 1, "step": 1})),
        ]),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
            t.llm_ms = 3_800;
            t.ttft_ms = 800;
            t.ttft_steps = 1;
            t.decode_ms = 3_000;
            t.decode_tokens = 60;
        })
    );
}

#[test]
fn keeps_first_attempt_token_boundary_across_in_step_retry() {
    assert_eq!(
        fold(&[
            at(1_000, "step/start", json!({"turn": 1, "step": 1})),
            at(1_200, "assistant/chunk", json!({"turn": 1, "step": 1, "chunk": {"type": "reasoning-delta", "index": 0, "text": "x"}})),
            at(2_000, "llm/retry", json!({"turn": 1, "step": 1})),
            at(3_000, "assistant/chunk", json!({"turn": 1, "step": 1, "chunk": {"type": "text-delta", "index": 0, "text": "y"}})),
            at(5_000, "assistant/message", json!({"turn": 1, "step": 1, "message": message_json()})),
            at(5_100, "step/end", json!({"turn": 1, "step": 1})),
        ]),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
            t.llm_ms = 4_000;
            t.ttft_ms = 200;
            t.ttft_steps = 1;
        })
    );
}

#[test]
fn ignores_empty_deltas_non_token_chunks_and_chunks_outside_open_step() {
    assert_eq!(
        fold(&[
            // Chunk before any step/start: no open boundary.
            at(500, "assistant/chunk", json!({"turn": 1, "step": 1, "chunk": {"type": "text-delta", "index": 0, "text": "stray"}})),
            at(1_000, "step/start", json!({"turn": 1, "step": 1})),
            at(1_100, "assistant/chunk", json!({"turn": 1, "step": 1, "chunk": {"type": "block-start", "index": 0, "blockType": "text"}})),
            at(1_200, "assistant/chunk", json!({"turn": 1, "step": 1, "chunk": {"type": "text-delta", "index": 0, "text": ""}})),
            at(1_300, "assistant/chunk", json!({"turn": 2, "step": 9, "chunk": {"type": "text-delta", "index": 0, "text": "other"}})),
            at(1_400, "assistant/chunk", json!({"turn": 1, "step": 1, "chunk": {"type": "text-delta", "index": 0, "text": "first"}})),
            at(2_000, "assistant/message", json!({"turn": 1, "step": 1, "message": message_json()})),
            at(2_100, "step/end", json!({"turn": 1, "step": 1})),
        ]),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
            t.llm_ms = 1_000;
            t.ttft_ms = 400;
            t.ttft_steps = 1;
        })
    );
}

#[test]
fn leaves_cancelled_step_untimed() {
    assert_eq!(
        fold(&[
            at(1_000, "step/start", json!({"turn": 1, "step": 1})),
            at(1_500, "assistant/chunk", json!({"turn": 1, "step": 1, "chunk": {"type": "text-delta", "index": 0, "text": "partial"}})),
            at(2_000, "step/end", json!({"turn": 1, "step": 1})),
        ]),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
        })
    );
}

fn tool_result(call_id: &str) -> Value {
    json!({"turn": 1, "step": 1, "message": {"source": {"kind": "tool", "callId": call_id}}})
}

#[test]
fn pairs_tool_wall_time_by_call_id_and_prunes_leftovers_at_turn_end() {
    assert_eq!(
        fold(&[
            at(1_000, "step/start", json!({"turn": 1, "step": 1})),
            at(1_100, "tool/call", json!({"turn": 1, "step": 1, "callId": "a", "name": "read", "arguments": "{}"})),
            at(1_200, "tool/call", json!({"turn": 1, "step": 1, "callId": "b", "name": "read", "arguments": "{}"})),
            // Out-of-order settlement pairs by id, not adjacency.
            at(4_200, "tool/result", tool_result("b")),
            at(1_600, "tool/result", tool_result("a")),
            at(5_000, "tool/result", tool_result("ghost")),
            at(5_100, "step/end", json!({"turn": 1, "step": 1})),
        ]),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
            t.tool_ms = 3_500;
        })
    );
    // An unresolved call is dropped at turn/end; a later result cannot pair.
    assert_eq!(
        fold(&[
            at(1_000, "step/start", json!({"turn": 1, "step": 1})),
            at(1_100, "tool/call", json!({"turn": 1, "step": 1, "callId": "orphan", "name": "read", "arguments": "{}"})),
            at(2_000, "step/end", json!({"turn": 1, "step": 1})),
            at(2_100, "turn/end", json!({"turn": 1, "reason": {"kind": "aborted", "reason": {"kind": "legacy"}}})),
            at(9_000, "tool/result", tool_result("orphan")),
        ]),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
        })
    );
}

#[test]
fn pairs_only_own_pending_calls_keys() {
    // Crash recovery (TOOL_NOT_STARTED) emits results with no preceding
    // tool/call; a provider-minted callId colliding with an Object prototype
    // property must read as absent. (Rust JSON objects have no prototype
    // chain — own-key by construction — but the unmatched-read behavior is
    // pinned identically.)
    assert_eq!(
        fold(&[
            at(1_000, "step/start", json!({"turn": 1, "step": 1})),
            at(1_500, "tool/result", tool_result("toString")),
            at(2_000, "step/end", json!({"turn": 1, "step": 1})),
        ]),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
        })
    );
    // The same name pairs normally once its call is recorded.
    assert_eq!(
        fold(&[
            at(1_000, "step/start", json!({"turn": 1, "step": 1})),
            at(1_100, "tool/call", json!({"turn": 1, "step": 1, "callId": "constructor", "name": "read", "arguments": "{}"})),
            at(1_600, "tool/result", tool_result("constructor")),
            at(2_000, "step/end", json!({"turn": 1, "step": 1})),
        ]),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
            t.tool_ms = 500;
        })
    );
}

#[test]
fn skips_decode_for_invalid_usage_report_and_ignores_duplicate_message() {
    let events = [
        at(1_000, "step/start", json!({"turn": 1, "step": 1})),
        at(1_400, "assistant/chunk", json!({"turn": 1, "step": 1, "chunk": {"type": "text-delta", "index": 0, "text": "a"}})),
        // A malformed provider report: guarded like the window fold.
        at(2_000, "assistant/message", json!({"turn": 1, "step": 1, "message": message_json(), "usage": {"inputTokens": 1, "outputTokens": -5}})),
    ];
    let mut with_end = events.to_vec();
    with_end.push(at(2_100, "step/end", json!({"turn": 1, "step": 1})));
    assert_eq!(
        fold(&with_end),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
            t.llm_ms = 1_000;
            t.ttft_ms = 400;
            t.ttft_steps = 1;
        })
    );
    // The first message closed the step boundary; a defensive duplicate
    // finds no open step and folds to the same reference.
    let definition = session_stats_projection_definition();
    let mut state: ArcValue = (definition.init)();
    for event in &events {
        state = (definition.apply)(&state, event);
    }
    let duplicate = at(2_050, "assistant/message", json!({"turn": 1, "step": 1, "message": message_json()}));
    let next = (definition.apply)(&state, &duplicate);
    assert!(Arc::ptr_eq(&next, &state), "duplicate message must fold to the same reference");
}

#[test]
fn accrues_nothing_for_unrelated_events_and_clamps_negative_clock_skew() {
    let definition = session_stats_projection_definition();
    let state: ArcValue = (definition.init)();
    let untouched = (definition.apply)(&state, &at(1, "user/message", json!({"content": []})));
    assert!(Arc::ptr_eq(&untouched, &state));
    assert_eq!(
        fold(&[
            at(2_000, "step/start", json!({"turn": 1, "step": 1})),
            at(1_000, "assistant/message", json!({"turn": 1, "step": 1, "message": message_json()})),
            at(2_100, "step/end", json!({"turn": 1, "step": 1})),
        ]),
        totals(|t| {
            t.turns = 1;
            t.steps = 1;
        })
    );
}
