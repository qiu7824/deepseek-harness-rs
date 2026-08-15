//! The `title` projection unit. Rust port of the core
//! `packages/session/session-title/tests/projection.spec.ts` behaviors.

use std::sync::Arc;

use cordis::Context;
use dsh_session::{Session, SessionStore, session_id};
use dsh_session_projection::SessionProjectionRegistry;
use dsh_session_title::{Config, SessionTitlePlugin, SessionTitleService};

fn config() -> Config {
    Config {
        fallback_max_words: 8,
        fallback_max_bytes: 64,
        max_title_bytes: 256,
    }
}

async fn harness(with_title_service: bool) -> (Context, Arc<SessionStore>, Arc<SessionProjectionRegistry>, Option<Arc<SessionTitleService>>) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let registry = SessionProjectionRegistry::install(&ctx);
    let service = if with_title_service {
        Some(SessionTitleService::install(&ctx, config()).expect("install"))
    } else {
        None
    };
    (ctx, store, registry, service)
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

async fn start_session(store: &SessionStore, ctx: &Context, id: &str) -> Session {
    store
        .create(
            &ctx,
            Some(session_id(id)),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create")
}

fn append_title(session: &Session, title: &str) -> u64 {
    session
        .append(
            "session/title",
            serde_json::json!({
                "title": title,
                "messageSeqs": [1],
                "source": {"kind": "fallback"},
            }),
            None,
        )
        .expect("append")
        .seq
}

#[tokio::test(flavor = "current_thread")]
async fn serves_null_before_the_first_title_event() {
    let (_ctx, store, registry, service) = harness(true).await;
    let _ = service.expect("service");
    let session = start_session(&store, &_ctx, "titled").await;
    // The inject fiber must settle before the unit is registered.
    settle().await;
    let snapshot = registry.snapshot(&session);
    assert_eq!(snapshot.values.get("title"), Some(&serde_json::Value::Null));
}

#[tokio::test(flavor = "current_thread")]
async fn serves_the_latest_title_last_wins_and_notifies_the_change_feed_with_the_causing_seq() {
    let (ctx, _store, registry, service) = harness(true).await;
    let _ = service.expect("service");
    let session = start_session(&_store, &ctx, "titled").await;
    settle().await;

    let changes: Arc<parking_lot::Mutex<Vec<(String, serde_json::Value, i64)>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let changes_for_listener = changes.clone();
    let _disposer = registry.on_changed(
        &ctx,
        Arc::new(move |_session, key, value, seq| {
            changes_for_listener
                .lock()
                .push((key.to_string(), value.clone(), seq));
        }),
    );

    let first_seq = append_title(&session, "First title");
    let second_seq = append_title(&session, "Second title");
    session
        .append("turn/start", dsh_session::turn_start_data(1), None)
        .expect("append");

    let changes = changes.lock();
    assert_eq!(
        *changes,
        vec![
            ("title".to_string(), serde_json::json!("First title"), first_seq as i64),
            ("title".to_string(), serde_json::json!("Second title"), second_seq as i64),
        ]
    );
    drop(changes);

    let snapshot = registry.snapshot(&session);
    assert_eq!(snapshot.values.get("title"), Some(&serde_json::json!("Second title")));
    assert_eq!(snapshot.as_of_seq, session.seq() as i64 - 1);
}

#[tokio::test(flavor = "current_thread")]
async fn folds_titles_already_in_the_log_when_the_service_mounts_late() {
    let (ctx, store, registry, _) = harness(false).await;
    let session = start_session(&store, &ctx, "titled").await;
    append_title(&session, "Pre-mount title");
    let _service = SessionTitleService::install(&ctx, config()).expect("install");
    settle().await;
    assert_eq!(
        registry.snapshot(&session).values.get("title"),
        Some(&serde_json::json!("Pre-mount title"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn has_no_title_key_without_the_service_and_drops_it_when_the_service_unloads() {
    let (ctx, store, registry, _) = harness(false).await;
    let session = start_session(&store, &ctx, "titled").await;
    settle().await;
    assert!(!registry.snapshot(&session).values.contains_key("title"));

    let fiber = ctx.plugin(Arc::new(SessionTitlePlugin), cordis::arc(config()));
    fiber.settle().await.expect("settle");
    settle().await;
    append_title(&session, "Ephemeral");
    assert_eq!(
        registry.snapshot(&session).values.get("title"),
        Some(&serde_json::json!("Ephemeral"))
    );

    fiber.dispose().await;
    settle().await;
    assert!(!registry.snapshot(&session).values.contains_key("title"));
}
