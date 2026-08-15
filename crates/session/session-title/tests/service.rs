//! SessionTitleService basics: immediate deterministic fallback,
//! eligibility filtering, and replay folds. Rust port of the core
//! `packages/session/session-title/tests/session-title.spec.ts` behaviors.

use std::sync::Arc;

use cordis::Context;
use dsh_session::{Session, SessionStore, session_id};
use dsh_session_title::{
    Config, SessionTitleService, SessionTitleSource, fold_session_title,
    session_title_provider_id,
};

fn config() -> Config {
    Config {
        fallback_max_words: 5,
        fallback_max_bytes: 40,
        max_title_bytes: 80,
    }
}

async fn setup() -> (Context, Arc<SessionStore>, Arc<SessionTitleService>) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let service = SessionTitleService::install(&ctx, config()).expect("install");
    (ctx, store, service)
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

fn append_intent() -> dsh_session::SurfaceIntent {
    dsh_session::SurfaceIntent {
        surface_op: dsh_session::SurfaceOp::Append,
        source_event_seqs: None,
    }
}

fn user_message_data(id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "role": "user",
        "content": [{"type": "text", "text": text}],
        "source": {"kind": "user"},
    })
}

fn append_human(session: &Session, id: &str, text: &str) -> dsh_session::SessionEvent {
    session
        .append("user/message", user_message_data(id, text), Some(append_intent()))
        .expect("append")
}

async fn start_session(store: &SessionStore, ctx: &Context, id: &str) -> Session {
    let session = store
        .create(
            &ctx,
            Some(session_id(id)),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append("turn/start", dsh_session::turn_start_data(1), None)
        .expect("turn/start");
    session
}

#[tokio::test(flavor = "current_thread")]
async fn logs_and_folds_an_immediate_fallback_after_the_first_eligible_human_text_message() {
    let (ctx, store, service) = setup().await;
    let session = start_session(&store, &ctx, "fresh").await;
    let message = append_human(&session, "m1", "  Build\nlog-backed session titles please  ");

    settle().await;

    let events = session.events();
    let title_event = events
        .iter()
        .rev()
        .find(|event| event.type_ == "session/title")
        .expect("title event");
    assert_eq!(title_event.seq, 2);
    assert_eq!(title_event.data["title"], "Build log-backed session titles please");
    assert_eq!(title_event.data["messageSeqs"], serde_json::json!([message.seq]));
    assert_eq!(title_event.data["source"], serde_json::json!({"kind": "fallback"}));

    let snapshot = service.get(&session).expect("title");
    assert_eq!(snapshot.title, "Build log-backed session titles please");
    assert_eq!(snapshot.message_seqs, vec![message.seq]);
    assert!(matches!(snapshot.source, SessionTitleSource::Fallback));
    assert_eq!(snapshot.event_seq, 2);
    assert_eq!(snapshot.updated_at, title_event.time);
    assert_eq!(session.derive_messages().expect("messages").len(), 1);
    assert_eq!(session.surface().expect("surface").nodes, vec![message.seq]);
    let _ = ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn derives_a_fallback_title_from_the_direct_prompt() {
    let (_ctx, store, service) = setup().await;
    let session = start_session(&store, &_ctx, "prefixed-title").await;
    append_human(&session, "m1", "Explain this referenced session");
    settle().await;
    assert_eq!(
        service.get(&session).expect("title").title,
        "Explain this referenced session"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn waits_through_synthetic_empty_and_non_text_messages_then_keeps_the_first_fallback() {
    let (_ctx, store, service) = setup().await;
    let session = start_session(&store, &_ctx, "eligibility").await;
    // Plugin-sourced message: not eligible.
    session
        .append(
            "user/message",
            serde_json::json!({
                "id": "p1",
                "role": "user",
                "content": [{"type": "text", "text": "plugin text"}],
                "source": {"kind": "plugin", "plugin": "seed"},
            }),
            Some(append_intent()),
        )
        .expect("append");
    // Reasoning-only user message: no text blocks.
    session
        .append(
            "user/message",
            serde_json::json!({
                "id": "r1",
                "role": "user",
                "content": [{"type": "reasoning", "text": "not visible text"}],
                "source": {"kind": "user"},
            }),
            Some(append_intent()),
        )
        .expect("append");
    // Whitespace-only user message: normalizes away.
    append_human(&session, "w1", " \n\t ");
    settle().await;
    assert!(service.get(&session).is_none());

    let eligible = append_human(&session, "e1", "first real prompt");
    settle().await;
    let first = service.get(&session).expect("title").clone();
    append_human(&session, "e2", "later prompt");
    settle().await;

    assert_eq!(first.message_seqs, vec![eligible.seq]);
    assert_eq!(service.get(&session).as_ref(), Some(&first));
    assert_eq!(
        session
            .events()
            .iter()
            .filter(|event| event.type_ == "session/title")
            .count(),
        1
    );
}

#[test]
fn folds_the_latest_title_event_during_replay() {
    let seed = Session::create(session_id("source"), None, None).expect("create");
    seed.append(
        "session/title",
        serde_json::json!({
            "title": "Earlier",
            "messageSeqs": [1],
            "source": {"kind": "fallback"},
        }),
        None,
    )
    .expect("append");
    seed.append(
        "session/title",
        serde_json::json!({
            "title": "Later",
            "messageSeqs": [1, 4],
            "source": {
                "kind": "provider",
                "provider": "test-provider",
                "model": {"provider": "mock", "model": "title-model"},
            },
        }),
        None,
    )
    .expect("append");

    let folded = fold_session_title(&seed.events()).expect("title");
    assert_eq!(folded.title, "Later");
    assert_eq!(folded.message_seqs, vec![1, 4]);
    assert_eq!(
        folded.source,
        SessionTitleSource::Provider {
            provider: session_title_provider_id("test-provider"),
            model: Some(dsh_session_title::SessionTitleModelProvenance {
                provider: "mock".to_string(),
                model: "title-model".to_string(),
            }),
        }
    );
    assert_eq!(folded.event_seq, 1);
    assert_eq!(folded.updated_at, seed.events()[1].time);
}

#[test]
fn requires_explicit_positive_limits_with_a_fallback_cap_no_larger_than_the_accepted_title_cap() {
    let ctx = Context::root();
    let mut bad_words = config();
    bad_words.fallback_max_words = 0;
    let error = SessionTitleService::install(&ctx, bad_words).err().expect("reject");
    assert!(error.contains("fallbackMaxWords must be a positive integer"), "{error}");

    let mut oversized = config();
    oversized.fallback_max_bytes = 81;
    let error = SessionTitleService::install(&ctx, oversized).err().expect("reject");
    assert!(error.contains("fallbackMaxBytes must not exceed maxTitleBytes"), "{error}");
}
