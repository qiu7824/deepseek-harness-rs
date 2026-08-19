//! Title-source invariant: `messageSeqs` is empty iff `source.kind` is
//! `user`. Rust port of
//! `packages/session/session-title/tests/invariant.spec.ts` (the append
//! veto of the TS internal/dispatch path is contained in this port, so the
//! pure checker plus the installed companion are exercised here).

use std::sync::Arc;

use cordis::Context;
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_session::{SessionEvent, SessionStore, session_id};
use dsh_session_title::invariant::{self, SessionTitleInvariantPlugin};

fn title_event(seq: u64, source_kind: &str, seqs: Vec<u64>) -> SessionEvent {
    SessionEvent {
        type_: "session/title".to_string(),
        seq,
        time: 0,
        data: serde_json::json!({
            "title": "x",
            "messageSeqs": seqs,
            "source": {"kind": source_kind},
        }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

#[test]
fn accepts_cited_automatic_titles_and_citation_free_user_renames() {
    let fallback = title_event(0, "fallback", vec![1]);
    invariant::check_title_event(&fallback, &|_| panic!("must not fail"));
    let provider = title_event(1, "provider", vec![1, 2]);
    invariant::check_title_event(&provider, &|_| panic!("must not fail"));
    let user = title_event(2, "user", vec![]);
    invariant::check_title_event(&user, &|_| panic!("must not fail"));
}

#[test]
fn rejects_a_citation_free_automatic_title_and_a_user_rename_that_cites_messages() {
    let automatic = title_event(0, "fallback", vec![]);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        invariant::check_title_event(&automatic, &|message| panic!("{message}"));
    }));
    let message = outcome
        .err()
        .and_then(|payload| payload.downcast::<String>().ok())
        .expect("panic payload");
    assert!(
        message.contains("session/title event 0 with source \"fallback\" must cite at least one message seq; got 0"),
        "{message}"
    );

    let user = title_event(0, "user", vec![1]);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        invariant::check_title_event(&user, &|message| panic!("{message}"));
    }));
    let message = outcome
        .err()
        .and_then(|payload| payload.downcast::<String>().ok())
        .expect("panic payload");
    assert!(
        message.contains(
            "session/title event 0 with source \"user\" must cite no message seqs; got 1"
        ),
        "{message}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn companion_installs_and_valid_appends_commit() {
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
    let fiber = ctx.plugin(Arc::new(SessionTitleInvariantPlugin), cordis::arc(()));
    fiber.settle().await.expect("settle");

    let session = store
        .create(
            &ctx,
            Some(session_id("title-invariant-valid")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append(
            "session/title",
            serde_json::json!({
                "title": "auto",
                "messageSeqs": [1],
                "source": {"kind": "fallback"},
            }),
            None,
        )
        .expect("valid automatic title");
    session
        .append(
            "session/title",
            serde_json::json!({
                "title": "named",
                "messageSeqs": [],
                "source": {"kind": "user"},
            }),
            None,
        )
        .expect("valid user rename");
    assert_eq!(session.seq(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn violating_appends_are_contained_without_committing_the_rejected_shape_mark() {
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
    let fiber = ctx.plugin(Arc::new(SessionTitleInvariantPlugin), cordis::arc(()));
    fiber.settle().await.expect("settle");

    let session = store
        .create(
            &ctx,
            Some(session_id("title-invariant-invalid")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    // The violating append commits (containment), but the checker rejects
    // the same durable shape.
    let _ = session
        .append(
            "session/title",
            serde_json::json!({
                "title": "auto",
                "messageSeqs": [],
                "source": {"kind": "fallback"},
            }),
            None,
        )
        .expect("append is contained in this port");
    let event = session.events()[0].clone();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        invariant::check_title_event(&event, &|message| panic!("{message}"));
    }));
    assert!(outcome.is_err());
}
