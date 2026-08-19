use dsh_goal::{GoalActivation, GoalPhase, GoalView, goal_id};
use dsh_llm::{ContentBlock, MessageSource, create_user_message};
use dsh_session::{SessionEvent, SessionStore, SurfaceOp, session_id};
use std::sync::Arc;

fn event(seq: u64, type_: &str, data: serde_json::Value) -> SessionEvent {
    SessionEvent {
        type_: type_.to_string(),
        seq,
        time: (seq + 1) as i64,
        data,
        ignorable: None,
        surface_op: (type_ == "user/message").then_some(SurfaceOp::Append),
        source_event_seqs: None,
    }
}

fn goal_change() -> SessionEvent {
    event(
        0,
        "goal/change",
        serde_json::json!({
            "kind": "goal/change",
            "version": 1,
            "operation": "create",
            "goal": {
                "id": "invariant-goal",
                "revision": 1,
                "objective": "verify every continuation prompt",
                "phase": "active",
                "maxGoalRounds": 2,
            },
            "roundsStarted": 0,
            "createdAt": 1,
            "updatedAt": 1,
        }),
    )
}

fn view(rounds_started: u64) -> GoalView {
    GoalView {
        id: goal_id("invariant-goal"),
        revision: 1,
        objective: "verify every continuation prompt".to_string(),
        phase: GoalPhase::Active,
        blocked_reason: None,
        max_goal_rounds: 2,
        rounds_started,
        created_at: 1,
        updated_at: 1,
        activation: GoalActivation::Armed,
    }
}

fn round_event(seq: u64, round: u64, content: Vec<ContentBlock>) -> SessionEvent {
    event(
        seq,
        "user/message",
        serde_json::to_value(create_user_message(
            content,
            MessageSource::Goal {
                goal_id: "invariant-goal".to_string(),
                revision: 1,
                round,
            },
        ))
        .expect("message JSON"),
    )
}

#[test]
fn reconstructs_existing_rounds_and_accepts_the_next_canonical_prompt() {
    let first = round_event(
        1,
        1,
        dsh_goal_round_driver::render_goal_round_prompt(&view(0), 1),
    );
    let prefix = vec![goal_change(), first];
    let second = round_event(
        2,
        2,
        dsh_goal_round_driver::render_goal_round_prompt(&view(1), 2),
    );

    dsh_goal_round_driver::invariant::validate_goal_round_event(&prefix, &second)
        .expect("canonical next round");
}

#[test]
fn rejects_counterfeit_content_and_missing_goal_state() {
    let prefix = vec![goal_change()];
    let counterfeit = round_event(
        1,
        1,
        vec![ContentBlock::Text {
            text: "counterfeit continuation".to_string(),
        }],
    );
    let error = dsh_goal_round_driver::invariant::validate_goal_round_event(&prefix, &counterfeit)
        .expect_err("counterfeit must fail");
    assert!(error.contains("content does not match"), "{error}");

    let canonical = round_event(
        0,
        1,
        dsh_goal_round_driver::render_goal_round_prompt(&view(0), 1),
    );
    let error = dsh_goal_round_driver::invariant::validate_goal_round_event(&[], &canonical)
        .expect_err("missing goal must fail");
    assert!(error.contains("cannot be reconstructed"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn companion_reservation_releases_on_dispose() {
    let ctx = cordis::Context::root();
    SessionStore::install(&ctx);
    dsh_invariants::InvariantRegistry::new(
        &ctx,
        dsh_invariants::InvariantConfig {
            enabled: true,
            ..Default::default()
        },
    );

    let dispose = dsh_goal_round_driver::invariant::apply(&ctx);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dsh_goal_round_driver::invariant::apply(&ctx)
        }))
        .is_err(),
        "the companion must reserve its package exactly once"
    );
    dispose().await;

    let again = dsh_goal_round_driver::invariant::apply(&ctx);
    again().await;
}

#[tokio::test(flavor = "current_thread")]
async fn installation_does_not_misclassify_a_valid_event_appended_during_scan() {
    let ctx = cordis::Context::root();
    let store = SessionStore::install(&ctx);
    let session = store
        .create(
            &ctx,
            Some(session_id("goal-round-invariant-valid-install-race")),
            Some(Default::default()),
        )
        .await
        .expect("session");
    session
        .append("goal/change", goal_change().data, None)
        .expect("goal change");
    session
        .append(
            "user/message",
            round_event(
                1,
                1,
                vec![ContentBlock::Text {
                    text: "first counterfeit".to_string(),
                }],
            )
            .data,
            Some(dsh_session::SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("first counterfeit");

    let failures = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let failures_for_callback = failures.clone();
    let appended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let appended_for_callback = appended.clone();
    let session_for_callback = session.clone();
    let fail: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |message| {
        failures_for_callback.lock().push(message.to_string());
        if !appended_for_callback.swap(true, std::sync::atomic::Ordering::SeqCst) {
            session_for_callback
                .append(
                    "user/message",
                    round_event(
                        2,
                        2,
                        dsh_goal_round_driver::render_goal_round_prompt(&view(1), 2),
                    )
                    .data,
                    Some(dsh_session::SurfaceIntent {
                        surface_op: SurfaceOp::Append,
                        source_event_seqs: None,
                    }),
                )
                .expect("valid second round");
        }
    });
    let installer = dsh_goal_round_driver::invariant::installer();
    (installer.install)(&ctx, fail).await;

    assert_eq!(
        failures.lock().len(),
        1,
        "a valid event committed during installation must use the live durable prefix"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn installation_validates_an_event_appended_during_late_load_scan() {
    let ctx = cordis::Context::root();
    let store = SessionStore::install(&ctx);
    let session = store
        .create(
            &ctx,
            Some(session_id("goal-round-invariant-install-race")),
            Some(Default::default()),
        )
        .await
        .expect("session");
    session
        .append("goal/change", goal_change().data, None)
        .expect("goal change");
    session
        .append(
            "user/message",
            round_event(
                1,
                1,
                vec![ContentBlock::Text {
                    text: "first counterfeit".to_string(),
                }],
            )
            .data,
            Some(dsh_session::SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("first counterfeit");

    let failures = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let failures_for_callback = failures.clone();
    let appended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let appended_for_callback = appended.clone();
    let session_for_callback = session.clone();
    let fail: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |message| {
        failures_for_callback.lock().push(message.to_string());
        if !appended_for_callback.swap(true, std::sync::atomic::Ordering::SeqCst) {
            session_for_callback
                .append(
                    "user/message",
                    round_event(
                        2,
                        2,
                        vec![ContentBlock::Text {
                            text: "second counterfeit during install".to_string(),
                        }],
                    )
                    .data,
                    Some(dsh_session::SurfaceIntent {
                        surface_op: SurfaceOp::Append,
                        source_event_seqs: None,
                    }),
                )
                .expect("second counterfeit");
        }
    });
    let installer = dsh_goal_round_driver::invariant::installer();
    (installer.install)(&ctx, fail).await;

    assert_eq!(
        failures.lock().len(),
        2,
        "listeners must close the gap before late-load validation snapshots existing sessions"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn late_load_reports_a_counterfeit_existing_round() {
    let ctx = cordis::Context::root();
    let store = SessionStore::install(&ctx);
    let session = store
        .create(
            &ctx,
            Some(session_id("goal-round-invariant-late-load")),
            Some(Default::default()),
        )
        .await
        .expect("session");
    session
        .append("goal/change", goal_change().data, None)
        .expect("goal change");
    session
        .append(
            "user/message",
            round_event(
                1,
                1,
                vec![ContentBlock::Text {
                    text: "counterfeit continuation".to_string(),
                }],
            )
            .data,
            Some(dsh_session::SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("counterfeit event");

    let failures = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let failures_for_callback = failures.clone();
    let fail: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |message| {
        failures_for_callback.lock().push(message.to_string());
    });
    let installer = dsh_goal_round_driver::invariant::installer();
    (installer.install)(&ctx, fail).await;

    assert!(
        failures
            .lock()
            .iter()
            .any(|message| message.contains("content does not match")),
        "late-load scan must attribute the counterfeit prefix"
    );
}
