//! Session title persistence round trips. Rust port of the core
//! `packages/session/session-title/tests/persistence.spec.ts` behaviors.

use std::sync::Arc;

use cordis::Context;
use dsh_session::{Session, SessionStore, session_id};
use dsh_session_persistence::{SessionInspection, SessionPersistenceApi};
use dsh_session_persistence_jsonl::{JsonlCompression, JsonlConfig, JsonlSessionPersistence};
use dsh_session_persistence_sqlite::{JournalMode, SqliteConfig, SqliteSessionPersistence};
use dsh_session_title::{Config, SessionTitleService, fold_session_title};

fn config() -> Config {
    Config {
        fallback_max_words: 5,
        fallback_max_bytes: 40,
        max_title_bytes: 80,
    }
}

fn append_intent() -> dsh_session::SurfaceIntent {
    dsh_session::SurfaceIntent {
        surface_op: dsh_session::SurfaceOp::Append,
        source_event_seqs: None,
    }
}

fn append_human(session: &Session, id: &str, text: &str) {
    session
        .append(
            "user/message",
            serde_json::json!({
                "id": id,
                "role": "user",
                "content": [{"type": "text", "text": text}],
                "source": {"kind": "user"},
            }),
            Some(append_intent()),
        )
        .expect("append");
}

async fn append_persisted_title(
    ctx: &Context,
    store: &SessionStore,
    service: &Arc<SessionTitleService>,
    id: &str,
) {
    let session = store
        .create(
            ctx,
            Some(session_id(id)),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append("turn/start", dsh_session::turn_start_data(1), None)
        .expect("turn/start");
    append_human(&session, "p1", "Persist this session title");
    session
        .append(
            "turn/end",
            dsh_session::turn_end_data(1, &dsh_session::TurnEndReason::Completed),
            None,
        )
        .expect("turn/end");
    let _ = service.refresh(&session, None).await.expect("refresh");
    // Durability: the store drain publishes through the backend.
    assert!(store.flush(&session).await.expect("flush"));
    let _ = ctx;
}

fn expect_persisted_title(inspection: SessionInspection) {
    let folded = fold_session_title(&inspection.events).expect("title");
    assert_eq!(folded.title, "Persist this session title");
    assert_eq!(folded.message_seqs, vec![1]);
    assert!(matches!(
        folded.source,
        dsh_session_title::SessionTitleSource::Fallback
    ));
    assert_eq!(folded.event_seq, 3);
    let event_types: Vec<&str> = inspection
        .events
        .iter()
        .map(|event| event.type_.as_str())
        .collect();
    assert_eq!(
        event_types,
        vec!["turn/start", "user/message", "turn/end", "session/title"]
    );
}

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("dsh-title-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    root
}

#[tokio::test(flavor = "multi_thread")]
async fn round_trips_through_a_remounted_jsonl_backend() {
    let root = temp_root("jsonl");
    let id = session_id("title-jsonl");

    {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let _backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().to_string(),
                compression: JsonlCompression::None,
                ..Default::default()
            },
        )
        .expect("jsonl backend");
        let service = SessionTitleService::install(&ctx, config()).expect("install");
        append_persisted_title(&ctx, &store, &service, "title-jsonl").await;
        // Flush the batch before dropping the writer.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    {
        let ctx = Context::root();
        let _store = SessionStore::install(&ctx);
        let backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().to_string(),
                compression: JsonlCompression::None,
                ..Default::default()
            },
        )
        .expect("jsonl backend");
        let loaded = backend.load(&id).await.expect("load");
        expect_persisted_title(loaded);
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn round_trips_through_a_remounted_sqlite_backend() {
    let root = temp_root("sqlite");
    let path = root.join("sessions.db");
    let id = session_id("title-sqlite");

    {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let _backend = SqliteSessionPersistence::install(
            &ctx,
            SqliteConfig {
                path: path.to_string_lossy().to_string(),
                journal_mode: JournalMode::Wal,
                ..Default::default()
            },
        )
        .expect("sqlite backend");
        let service = SessionTitleService::install(&ctx, config()).expect("install");
        append_persisted_title(&ctx, &store, &service, "title-sqlite").await;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    {
        let ctx = Context::root();
        let _store = SessionStore::install(&ctx);
        let backend = SqliteSessionPersistence::install(
            &ctx,
            SqliteConfig {
                path: path.to_string_lossy().to_string(),
                journal_mode: JournalMode::Wal,
                ..Default::default()
            },
        )
        .expect("sqlite backend");
        let loaded = backend.load(&id).await.expect("load");
        expect_persisted_title(loaded);
    }

    let _ = std::fs::remove_dir_all(&root);
}
