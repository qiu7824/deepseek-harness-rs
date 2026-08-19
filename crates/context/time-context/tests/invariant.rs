//! Package-owned durable clock-context invariants. Rust port of
//! `packages/context/time-context/tests/invariant.spec.ts` (the append veto
//! of the TS internal/dispatch path is contained in this port, so the pure
//! checker plus the installed companion are exercised here).

use std::sync::Arc;

use cordis::{Context, arc};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_llm::{
    ContentBlock, ContextForm, ContextSnapshotSection, MessageSource, UserMessage,
    create_user_message,
};
use dsh_session::{Session, SessionEvent, SessionStore, session_id};
use dsh_time_context::invariant::{self, TimeContextInvariantPlugin};
use dsh_time_context::request_zone::{BrowserTimeZoneContext, render_browser_time_zone_context};
use dsh_time_context::timestamp::{TimestampFormatter, format_timestamp};

fn event(session: &Session, type_: &str, data: serde_json::Value) -> SessionEvent {
    let intent = (type_ == "user/message").then(|| dsh_session::SurfaceIntent {
        surface_op: dsh_session::SurfaceOp::Append,
        source_event_seqs: None,
    });
    session.append(type_, data, intent).expect(type_)
}

fn turn_start(session: &Session, turn: u64) -> SessionEvent {
    event(session, "turn/start", serde_json::json!({ "turn": turn }))
}

fn step_start(session: &Session, step: u64) -> SessionEvent {
    event(
        session,
        "step/start",
        serde_json::json!({ "turn": 1, "step": step }),
    )
}

fn message_data(message: &UserMessage) -> serde_json::Value {
    serde_json::to_value(message).expect("message")
}

fn rpc_message(zone: &str) -> UserMessage {
    create_user_message(
        vec![ContentBlock::Text {
            text: "rpc".to_string(),
        }],
        MessageSource::User {
            rpc_id: Some("rpc-1".to_string()),
            client_time_zone: Some(zone.to_string()),
        },
    )
}

/// A structurally valid reading for turn 1/step 1 in the given zone.
fn reading(now: i64, zone: &str) -> UserMessage {
    let formatter = TimestampFormatter::create(Some(zone)).expect("zone");
    let text = format!(
        "Time sampled while preparing turn 1, step 1: {}\n{}\nElapsed since the preceding model-visible message: unavailable.",
        format_timestamp(now, &formatter, zone),
        render_browser_time_zone_context(&BrowserTimeZoneContext::Resolved {
            time_zone: zone.to_string()
        }),
    );
    create_user_message(
        vec![ContentBlock::Text { text: text.clone() }],
        MessageSource::Plugin {
            plugin: "time-context".to_string(),
            form: Some(ContextForm::Snapshot),
            sections: Some(vec![ContextSnapshotSection {
                name: "time-context".to_string(),
                text,
            }]),
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    )
}

/// Prefix history: an open turn/step with one Asia/Shanghai user-rpc message.
fn valid_prefix() -> Vec<SessionEvent> {
    let session = Session::create(session_id("prefix"), None, None).expect("session");
    turn_start(&session, 1);
    step_start(&session, 1);
    event(
        &session,
        "user/message",
        message_data(&rpc_message("Asia/Shanghai")),
    );
    session.events().iter().cloned().collect()
}

fn reading_event(session: &Session, message: &UserMessage) -> SessionEvent {
    event(session, "user/message", message_data(message))
}

#[test]
fn preparation_position_tracks_the_open_step_boundary() {
    let session = Session::create(session_id("positions"), None, None).expect("session");
    assert_eq!(
        invariant::preparation_position(&[]).unwrap_err(),
        "time-context reading must be appended inside an open turn"
    );
    turn_start(&session, 1);
    assert_eq!(
        invariant::preparation_position(&session_events(&session)).unwrap_err(),
        "time-context reading must follow step/start"
    );
    step_start(&session, 2);
    assert_eq!(
        invariant::preparation_position(&session_events(&session)).unwrap(),
        (1, 2)
    );
    event(&session, "request/header", serde_json::json!({}));
    assert_eq!(
        invariant::preparation_position(&session_events(&session)).unwrap_err(),
        "time-context reading must precede request/header"
    );
    event(
        &session,
        "step/end",
        serde_json::json!({ "turn": 1, "step": 2 }),
    );
    event(&session, "turn/end", serde_json::json!({ "turn": 1 }));
    assert_eq!(
        invariant::preparation_position(&session_events(&session)).unwrap_err(),
        "time-context reading must be appended inside an open turn"
    );
}

fn session_events(session: &Session) -> Vec<SessionEvent> {
    session.events().iter().cloned().collect()
}

#[test]
fn a_well_formed_reading_validates_against_its_prefix() {
    let now = chrono::Utc::now().timestamp_millis() - 60_000;
    let session = Session::create(session_id("valid-reading"), None, None).expect("session");
    let event = reading_event(&session, &reading(now, "Asia/Shanghai"));
    invariant::validate_reading(&valid_prefix(), &event).expect("valid reading");
}

#[test]
fn malformed_readings_fail_with_the_ts_messages() {
    let now = chrono::Utc::now().timestamp_millis() - 60_000;
    let prefix = valid_prefix();
    let session = Session::create(session_id("malformed"), None, None).expect("session");

    // Two text blocks.
    let two_blocks = create_user_message(
        vec![
            ContentBlock::Text {
                text: "a".to_string(),
            },
            ContentBlock::Text {
                text: "b".to_string(),
            },
        ],
        reading(now, "Asia/Shanghai").source,
    );
    assert_fail(
        &prefix,
        &reading_event(&session, &two_blocks),
        "time-context messages must contain exactly one text block",
    );

    // Non-matching text.
    let bad_text = create_user_message(
        vec![ContentBlock::Text {
            text: "not a reading".to_string(),
        }],
        reading(now, "Asia/Shanghai").source,
    );
    assert_fail(
        &prefix,
        &reading_event(&session, &bad_text),
        "time-context message does not match the durable reading format",
    );

    // Wrong turn/step pair.
    let wrong_position = create_user_message(
        vec![ContentBlock::Text {
            text: format!(
                "Time sampled while preparing turn 2, step 1: {}\n{}\nElapsed since the preceding model-visible message: unavailable.",
                format_timestamp(
                    now,
                    &TimestampFormatter::create(Some("UTC")).unwrap(),
                    "UTC"
                ),
                render_browser_time_zone_context(&BrowserTimeZoneContext::Missing),
            ),
        }],
        reading(now, "Asia/Shanghai").source,
    );
    assert_fail(
        &prefix,
        &reading_event(&session, &wrong_position),
        "time-context reading names turn 2/step 1, expected turn 1/step 1",
    );

    // A user-role source is not package ownership.
    let foreign = create_user_message(
        vec![ContentBlock::Text {
            text: reading_text(now, "Asia/Shanghai"),
        }],
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    assert_fail(
        &prefix,
        &reading_event(&session, &foreign),
        "time-context source must retain package ownership",
    );

    // Snapshot form without the exact single section.
    let bare_snapshot = create_user_message(
        vec![ContentBlock::Text {
            text: reading_text(now, "Asia/Shanghai"),
        }],
        MessageSource::Plugin {
            plugin: "time-context".to_string(),
            form: Some(ContextForm::Snapshot),
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    assert_fail(
        &prefix,
        &reading_event(&session, &bare_snapshot),
        "time-context source must carry only the exact snapshot text, not request authority",
    );

    // Browser-zone text that disagrees with the turn's user messages.
    let wrong_browser_text = format!(
        "Time sampled while preparing turn 1, step 1: {}\n{}\nElapsed since the preceding model-visible message: unavailable.",
        format_timestamp(
            now,
            &TimestampFormatter::create(Some("Asia/Shanghai")).unwrap(),
            "Asia/Shanghai"
        ),
        render_browser_time_zone_context(&BrowserTimeZoneContext::Resolved {
            time_zone: "America/New_York".to_string()
        }),
    );
    let wrong_browser = create_user_message(
        vec![ContentBlock::Text {
            text: wrong_browser_text.clone(),
        }],
        MessageSource::Plugin {
            plugin: "time-context".to_string(),
            form: Some(ContextForm::Snapshot),
            sections: Some(vec![ContextSnapshotSection {
                name: "time-context".to_string(),
                text: wrong_browser_text,
            }]),
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    assert_fail(
        &prefix,
        &reading_event(&session, &wrong_browser),
        "time-context browser-zone text does not match current-turn user messages",
    );

    // A postdating timestamp.
    let future = chrono::Utc::now().timestamp_millis() + 60 * 60_000;
    assert_fail(
        &prefix,
        &reading_event(&session, &reading(future, "Asia/Shanghai")),
        "time-context rendered timestamp must parse and not postdate its durable event",
    );
}

fn reading_text(now: i64, zone: &str) -> String {
    let message = reading(now, zone);
    match message.content.as_slice() {
        [ContentBlock::Text { text }] => text.clone(),
        _ => panic!("single text block"),
    }
}

fn assert_fail(prefix: &[SessionEvent], event: &SessionEvent, expected: &str) {
    let message = invariant::validate_reading(prefix, event).expect_err("must fail");
    assert!(
        message.contains(expected),
        "expected {expected:?}, got {message:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn companion_installs_and_valid_publications_commit() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let _registry = InvariantRegistry::new(
        &ctx,
        InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let fiber = ctx.plugin(Arc::new(TimeContextInvariantPlugin), arc(()));
    fiber.settle().await.expect("settle");

    let session = store
        .create(
            &ctx,
            Some(session_id("time-invariant-valid")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    turn_start(&session, 1);
    step_start(&session, 1);
    event(
        &session,
        "user/message",
        message_data(&rpc_message("Asia/Shanghai")),
    );
    let now = chrono::Utc::now().timestamp_millis();
    event(
        &session,
        "user/message",
        message_data(&reading(now, "Asia/Shanghai")),
    );
    assert_eq!(session.seq(), 4);
    fiber.dispose().await;
}

#[tokio::test(flavor = "current_thread")]
async fn violating_readings_are_contained_without_vetoing_the_append() {
    // Deviation note: the TS append veto throws from `session.append`; this
    // port contains internal-listener panics, so the companion's failure is
    // observable through the checker instead. The appended log stays
    // structurally valid either way.
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let _registry = InvariantRegistry::new(
        &ctx,
        InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let fiber = ctx.plugin(Arc::new(TimeContextInvariantPlugin), arc(()));
    fiber.settle().await.expect("settle");

    let session = store
        .create(
            &ctx,
            Some(session_id("time-invariant-invalid")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    turn_start(&session, 1);
    step_start(&session, 1);
    // A reading with a two-block content violates the durable shape; the
    // append commits (containment), but the checker rejects the same shape.
    let bad = create_user_message(
        vec![
            ContentBlock::Text {
                text: "x".to_string(),
            },
            ContentBlock::Text {
                text: "y".to_string(),
            },
        ],
        MessageSource::Plugin {
            plugin: "time-context".to_string(),
            form: Some(ContextForm::Snapshot),
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    let event = event(&session, "user/message", message_data(&bad));
    assert!(invariant::validate_reading(&session_events(&session)[..2], &event).is_err());
    fiber.dispose().await;
}
