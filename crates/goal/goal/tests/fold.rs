//! Rust port of the strict fold behaviors from `goal.spec.ts` / the durable
//! stream decoder: snapshot decoding, transition validation, clears, and the
//! round admission fold.

use dsh_goal::{
    GoalChangeMeta, GoalClearChangeMeta, GoalOperation, GoalPhase, GoalSnapshot,
    GoalSnapshotChangeMeta, apply_goal_change, decode_goal_change, empty_goal_fold_state,
    fold_goal, goal_id,
};
use dsh_session::SessionEvent;
use serde_json::{Value, json};

fn snapshot_change(
    operation: GoalOperation,
    goal: GoalSnapshot,
    rounds: u64,
    created: u64,
    updated: u64,
) -> GoalChangeMeta {
    GoalChangeMeta::Snapshot(GoalSnapshotChangeMeta {
        operation,
        goal,
        rounds_started: rounds,
        created_at: created,
        updated_at: updated,
    })
}

fn active_goal(id: &str, revision: u64, max_rounds: u64) -> GoalSnapshot {
    GoalSnapshot {
        id: goal_id(id),
        revision,
        objective: "finish".to_string(),
        phase: GoalPhase::Active,
        max_goal_rounds: max_rounds,
        blocked_reason: None,
    }
}

#[test]
fn folds_a_create_through_phase_ladder_and_clear() {
    let mut state = empty_goal_fold_state();
    let created = active_goal("goal-1", 1, 8);
    apply_goal_change(
        &mut state,
        &snapshot_change(GoalOperation::Create, created.clone(), 0, 10, 10),
    )
    .expect("create");
    assert_eq!(state.goal.as_ref(), Some(&created));
    assert_eq!(state.rounds_started, 0);

    let paused = GoalSnapshot {
        revision: 2,
        phase: GoalPhase::Paused,
        ..created.clone()
    };
    apply_goal_change(
        &mut state,
        &snapshot_change(GoalOperation::Pause, paused, 0, 10, 11),
    )
    .expect("pause");
    assert_eq!(state.goal.as_ref().unwrap().phase, GoalPhase::Paused);

    let resumed = GoalSnapshot {
        revision: 3,
        phase: GoalPhase::Active,
        ..created.clone()
    };
    apply_goal_change(
        &mut state,
        &snapshot_change(GoalOperation::Resume, resumed, 0, 10, 12),
    )
    .expect("resume");

    // A clear requires the exact next revision and a non-regressing
    // timestamp.
    let clear = GoalChangeMeta::Clear(GoalClearChangeMeta {
        cleared: dsh_goal::GoalRef {
            id: goal_id("goal-1"),
            revision: 4,
        },
        cleared_at: 12,
    });
    apply_goal_change(&mut state, &clear).expect("clear");
    assert!(state.goal.is_none());
    assert_eq!(state.rounds_started, 0);
}

#[test]
fn rejects_incoherent_transitions() {
    let mut state = empty_goal_fold_state();
    apply_goal_change(
        &mut state,
        &snapshot_change(GoalOperation::Create, active_goal("goal-1", 1, 8), 0, 10, 10),
    )
    .expect("create");

    // Pause from an already-paused goal.
    let mut paused_state = state.clone();
    apply_goal_change(
        &mut paused_state,
        &snapshot_change(
            GoalOperation::Pause,
            GoalSnapshot {
                revision: 2,
                phase: GoalPhase::Paused,
                ..active_goal("goal-1", 1, 8)
            },
            0,
            10,
            11,
        ),
    )
    .expect("pause once");
    let error = apply_goal_change(
        &mut paused_state,
        &snapshot_change(
            GoalOperation::Pause,
            GoalSnapshot {
                revision: 3,
                phase: GoalPhase::Paused,
                ..active_goal("goal-1", 1, 8)
            },
            0,
            10,
            12,
        ),
    )
    .err()
    .expect("invalid pause");
    assert!(error.contains("invalid phase transition"), "{error}");

    // A skipped revision is refused.
    let error = apply_goal_change(
        &mut state,
        &snapshot_change(
            GoalOperation::Complete,
            GoalSnapshot {
                revision: 5,
                phase: GoalPhase::Complete,
                ..active_goal("goal-1", 1, 8)
            },
            0,
            10,
            12,
        ),
    )
    .err()
    .expect("skipped revision");
    assert!(error.contains("advance the current goal by one revision"), "{error}");

    // A snapshot change that mutates the definition outside edit.
    let error = apply_goal_change(
        &mut state,
        &snapshot_change(
            GoalOperation::Pause,
            GoalSnapshot {
                revision: 2,
                phase: GoalPhase::Paused,
                objective: "different".to_string(),
                ..active_goal("goal-1", 1, 8)
            },
            0,
            10,
            11,
        ),
    )
    .err()
    .expect("definition drift");
    assert!(error.contains("cannot change objective"), "{error}");

    // Resume with an exhausted budget is refused: the rounds accumulate
    // through admitted user messages, then pause keeps the counters.
    let mut exhausted = state.clone();
    for round in 1..=8 {
        let event = SessionEvent {
            type_: "user/message".to_string(),
            seq: round,
            time: 0,
            data: json!({
                "source": { "kind": "goal", "goalId": "goal-1", "revision": 1, "round": round }
            }),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        };
        dsh_goal::apply_goal_event(&mut exhausted, &event).expect("admitted round");
    }
    assert_eq!(exhausted.rounds_started, 8);
    apply_goal_change(
        &mut exhausted,
        &snapshot_change(
            GoalOperation::Pause,
            GoalSnapshot {
                revision: 2,
                phase: GoalPhase::Paused,
                ..active_goal("goal-1", 1, 8)
            },
            8,
            10,
            11,
        ),
    )
    .expect("pause");
    let error = apply_goal_change(
        &mut exhausted,
        &snapshot_change(
            GoalOperation::Resume,
            GoalSnapshot {
                revision: 3,
                phase: GoalPhase::Active,
                ..active_goal("goal-1", 1, 8)
            },
            8,
            10,
            12,
        ),
    )
    .err()
    .expect("exhausted");
    assert!(error.contains("exhausted round budget"), "{error}");
}

#[test]
fn decoder_rejects_malformed_and_ignores_unrelated_values() {
    assert!(decode_goal_change(&json!({ "kind": "other" }))
        .expect("unrelated")
        .is_none());
    let error = decode_goal_change(&json!({
        "kind": "goal/change",
        "version": 2,
        "operation": "create"
    }))
    .err()
    .expect("bad version");
    assert!(error.contains("unsupported goal change version"), "{error}");

    let error = decode_goal_change(&json!({
        "kind": "goal/change",
        "version": 1,
        "operation": "create",
        "goal": { "id": "x", "revision": 1, "objective": "", "phase": "active", "maxGoalRounds": 8 },
        "roundsStarted": 0,
        "createdAt": 0,
        "updatedAt": 0
    }))
    .err()
    .expect("blank objective");
    assert!(error.contains("objective"), "{error}");
}

#[test]
fn admits_only_the_next_round_of_the_active_goal() {
    let mut state = empty_goal_fold_state();
    apply_goal_change(
        &mut state,
        &snapshot_change(GoalOperation::Create, active_goal("goal-1", 1, 8), 0, 10, 10),
    )
    .expect("create");

    let admitted = SessionEvent {
        type_: "user/message".to_string(),
        seq: 1,
        time: 0,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
        data: json!({
            "source": { "kind": "goal", "goalId": "goal-1", "revision": 1, "round": 1 }
        }),
    };
    dsh_goal::apply_goal_event(&mut state, &admitted).expect("admitted");
    assert_eq!(state.rounds_started, 1);

    // A non-sequential round is refused.
    let skipped = SessionEvent {
        type_: "user/message".to_string(),
        seq: 2,
        time: 0,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
        data: json!({
            "source": { "kind": "goal", "goalId": "goal-1", "revision": 1, "round": 3 }
        }),
    };
    let error = dsh_goal::apply_goal_event(&mut state, &skipped).err().expect("skipped round");
    assert!(error.contains("next admitted round"), "{error}");

    // Over the cap is refused.
    let over = SessionEvent {
        type_: "user/message".to_string(),
        seq: 2,
        time: 0,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
        data: json!({
            "source": { "kind": "goal", "goalId": "goal-1", "revision": 1, "round": 9 }
        }),
    };
    let mut other = state.clone();
    other.rounds_started = 8;
    let error = dsh_goal::apply_goal_event(&mut other, &over).err().expect("over cap");
    assert!(error.contains("next admitted round"), "{error}");
}

#[test]
fn fold_goal_returns_a_detached_projection() {
    let events = [SessionEvent {
        type_: "goal/change".to_string(),
        seq: 0,
        time: 0,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
        data: json!({
            "kind": "goal/change",
            "version": 1,
            "operation": "create",
            "goal": {
                "id": "goal-1",
                "revision": 1,
                "objective": "finish",
                "phase": "active",
                "maxGoalRounds": 8
            },
            "roundsStarted": 0,
            "createdAt": 10,
            "updatedAt": 10
        }),
    }];
    let folded = fold_goal(&events).expect("fold");
    assert_eq!(folded.goal.unwrap().id, goal_id("goal-1"));
    assert_eq!(folded.rounds_started, 0);
    assert_eq!(folded.created_at, Some(10));
    assert_eq!(
        folded.last_ref,
        Some(dsh_goal::GoalRef {
            id: goal_id("goal-1"),
            revision: 1
        })
    );
}

/// SessionEvent 鐢ㄥ埌鐨勪复鏃?JSON 宸ュ叿纭锛堥槻鏈娇鐢ㄥ憡璀︼級銆?#[allow(dead_code)]
fn _unused_value_import_anchor() -> Value {
    Value::Null
}
