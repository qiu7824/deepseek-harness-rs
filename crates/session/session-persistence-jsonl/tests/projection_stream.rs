use dsh_session::{SessionEvent, SessionHeader, session_id};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_session_persistence_jsonl::{
    JsonlCompression, JsonlConfig, JsonlSessionPersistence, log_path,
};
use parking_lot::Mutex;
use serde_json::json;
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn fixture_with(
    compression: JsonlCompression,
) -> (
    PathBuf,
    cordis::Context,
    Arc<JsonlSessionPersistence>,
    SessionHeader,
) {
    let root = std::env::temp_dir().join(format!("projection-stream-{}", uuid::Uuid::new_v4()));
    let ctx = cordis::Context::root();
    let backend = JsonlSessionPersistence::install(
        &ctx,
        JsonlConfig {
            root: root.to_string_lossy().into_owned(),
            compression,
            ..Default::default()
        },
    )
    .unwrap();
    let meta: SessionHeader = serde_json::from_value(
        json!({"version":4,"id":"projection","createdAt":0,"isSeeded":false}),
    )
    .unwrap();
    (root, ctx, backend, meta)
}
fn fixture() -> (
    PathBuf,
    cordis::Context,
    Arc<JsonlSessionPersistence>,
    SessionHeader,
) {
    fixture_with(JsonlCompression::Zstd)
}
fn event(seq: u64, kind: &str, data: serde_json::Value) -> SessionEvent {
    serde_json::from_value(json!({"type":kind,"seq":seq,"time":seq,"data":data})).unwrap()
}
async fn close(root: PathBuf, ctx: cordis::Context) {
    for disposer in ctx.fiber.disposables.clear() {
        disposer().await;
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn streamed_values_match_existing_legacy_normalization() {
    let (root, ctx, backend, meta) = fixture();
    let events = vec![
        event(
            0,
            "user/message",
            json!({"content":[{"type":"text","text":"legacy"}],"provenance":{"kind":"user"}}),
        ),
        event(1, "turn/start", json!({"turn":1,"trigger":{"kind":"user"}})),
        event(
            2,
            "tool/result",
            json!({"callId":"old-call","content":[{"type":"text","text":"kept"}],"isError":false}),
        ),
        event(
            3,
            "turn/end",
            json!({"turn":1,"reason":{"kind":"disposed"}}),
        ),
    ];
    let path = dsh_session_persistence_jsonl::session_dir(&root.to_string_lossy(), None, &meta.id).join("session.v3.jsonl.zstd");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut old = meta.clone(); old.version = 3;
    let header = format!("{}\n", serde_json::to_string(&dsh_session_persistence_jsonl::to_header_line(&old, None).unwrap()).unwrap());
    let body = format!("{}\n", dsh_session_persistence_jsonl::event_lines(&events, false));
    let mut bytes = dsh_session_persistence_jsonl::compress_zstd_frame(header.as_bytes()).unwrap();
    bytes.extend(dsh_session_persistence_jsonl::compress_zstd_frame(body.as_bytes()).unwrap());
    std::fs::write(path, bytes).unwrap();
    let mut normalizer = dsh_session_persistence::StoredEventNormalizer::new(meta.id.clone());
    let expected: Vec<_> = events.iter().map(|event| normalizer.normalize(event).unwrap().into_owned()).collect();
    let actual = Arc::new(Mutex::new(vec![]));
    let collect = actual.clone();
    assert!(
        backend
            .try_visit_projection_events(
                &meta.id,
                Arc::new(AtomicBool::new(false)),
                Arc::new(move |event| {
                    collect.lock().push(event.clone());
                    Ok(())
                })
            )
            .await
            .unwrap()
    );
    assert_eq!(*actual.lock(), expected);
    close(root, ctx).await;
}

#[tokio::test]
async fn incomplete_physical_tail_selects_existing_recovery_before_emitting() {
    let (root, ctx, backend, meta) = fixture();
    let events = vec![event(0, "feedback/record", json!({"text":"complete row"}))];
    backend.create(meta.clone(), None).await.unwrap();
    backend.append(&meta.id, &events).await.unwrap();
    let path = log_path(
        &root.to_string_lossy(),
        None,
        &meta.id,
        JsonlCompression::Zstd,
    );
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.pop();
    std::fs::write(&path, &bytes).unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let observer = called.clone();
    assert!(
        !backend
            .try_visit_projection_events(
                &meta.id,
                Arc::new(AtomicBool::new(false)),
                Arc::new(move |_| {
                    observer.store(true, Ordering::Release);
                    Ok(())
                })
            )
            .await
            .unwrap()
    );
    assert!(!called.load(Ordering::Acquire));
    assert_eq!(backend.read_from(&meta.id, 0).await.unwrap().events, events);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    close(root, ctx).await;
}

#[tokio::test]
async fn cancellation_stops_replay_and_unknown_required_rows_keep_their_refusal() {
    let (root, ctx, backend, meta) = fixture();
    let events: Vec<_> = (0..30)
        .map(|seq| event(seq, "feedback/record", json!({"value":seq})))
        .collect();
    backend.create(meta.clone(), None).await.unwrap();
    backend.append(&meta.id, &events).await.unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel = cancelled.clone();
    let count = Arc::new(Mutex::new(0));
    let observed = count.clone();
    let result = backend
        .try_visit_projection_events(
            &meta.id,
            cancelled,
            Arc::new(move |_| {
                *observed.lock() += 1;
                if *observed.lock() == 8 {
                    cancel.store(true, Ordering::Release);
                }
                Ok(())
            }),
        )
        .await;
    assert!(result.unwrap_err().contains("cancelled"));
    assert_eq!(*count.lock(), 8);
    let path = log_path(
        &root.to_string_lossy(),
        None,
        &meta.id,
        JsonlCompression::Zstd,
    );
    let row = serde_json::to_string(&event(30, "unknown/required", json!({}))).unwrap() + "\n";
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&dsh_session_persistence_jsonl::compress_zstd_frame(row.as_bytes()).unwrap())
        .unwrap();
    assert!(
        !backend
            .try_visit_projection_events(
                &meta.id,
                Arc::new(AtomicBool::new(false)),
                Arc::new(|_| panic!("preflight must refuse before emitting"))
            )
            .await
            .unwrap()
    );
    assert!(
        backend
            .read_from(&session_id("projection"), 0)
            .await
            .is_err()
    );
    close(root, ctx).await;
}

#[tokio::test]
async fn plain_jsonl_uses_streaming_and_malformed_complete_suffix_keeps_recovery_behavior() {
    let (root, ctx, backend, meta) = fixture_with(JsonlCompression::None);
    let events: Vec<_> = (0..20)
        .map(|seq| event(seq, "feedback/record", json!({"value":seq})))
        .collect();
    backend.create(meta.clone(), None).await.unwrap();
    backend.append(&meta.id, &events).await.unwrap();
    let actual = Arc::new(Mutex::new(vec![]));
    let collect = actual.clone();
    assert!(
        backend
            .try_visit_projection_events(
                &meta.id,
                Arc::new(AtomicBool::new(false)),
                Arc::new(move |event| {
                    collect.lock().push(event.clone());
                    Ok(())
                })
            )
            .await
            .unwrap()
    );
    assert_eq!(*actual.lock(), events);
    let path = log_path(
        &root.to_string_lossy(),
        None,
        &meta.id,
        JsonlCompression::None,
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"{broken}\n")
        .unwrap();
    assert!(
        !backend
            .try_visit_projection_events(
                &meta.id,
                Arc::new(AtomicBool::new(false)),
                Arc::new(|_| panic!("recovery must be selected before emitting"))
            )
            .await
            .unwrap()
    );
    assert_eq!(backend.read_from(&meta.id, 0).await.unwrap().events, events);
    close(root, ctx).await;
}
