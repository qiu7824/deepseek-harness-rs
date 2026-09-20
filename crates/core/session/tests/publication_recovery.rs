use cordis::{Context, EventOptions};
use dsh_session::{SessionStore, session_id};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn synchronous_observers_reject_reentry_and_continue_after_pending_listener() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let session = store
        .create(&ctx, Some(session_id("publication")), None)
        .await
        .unwrap();
    let recursive = session.clone();
    let rejected = Arc::new(AtomicUsize::new(0));
    let count = rejected.clone();
    ctx.on(
        "session/event",
        Arc::new(move |_, _| {
            let session = recursive.clone();
            let count = count.clone();
            Box::pin(async move {
                assert!(
                    session
                        .append("custom/nested", serde_json::json!({}), None)
                        .is_err()
                );
                count.fetch_add(1, Ordering::SeqCst);
                None
            })
        }),
        EventOptions::default().global(true),
    )
    .await;
    ctx.on(
        "session/event",
        Arc::new(|_, _| {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                panic!("pending observer must not be resumed");
            })
        }),
        EventOptions::default().global(true),
    )
    .await;
    let observed = Arc::new(AtomicUsize::new(0));
    let count = observed.clone();
    ctx.on(
        "session/event",
        Arc::new(move |_, _| {
            let count = count.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                None
            })
        }),
        EventOptions::default().global(true),
    )
    .await;
    session
        .append("custom/one", serde_json::json!({}), None)
        .unwrap();
    session
        .append("custom/two", serde_json::json!({}), None)
        .unwrap();
    assert_eq!(rejected.load(Ordering::SeqCst), 2);
    assert_eq!(observed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn dispatch_hooks_can_read_the_precommit_prefix_and_panics_release_publication() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let session = store
        .create(&ctx, Some(session_id("precommit")), None)
        .await
        .unwrap();
    let reader = session.clone();
    let initial = session.events().len();
    let reads = Arc::new(AtomicUsize::new(0));
    let count = reads.clone();
    ctx.on(
        "internal/dispatch",
        Arc::new(move |_, args| {
            let reader = reader.clone();
            let count = count.clone();
            Box::pin(async move {
                if cordis::downcast_arc::<String>(&args[1])
                    .is_some_and(|s| s.as_str() == "session/event")
                {
                    assert_eq!(reader.events().len(), initial);
                    count.fetch_add(1, Ordering::SeqCst);
                }
                None
            })
        }),
        EventOptions::default().global(true),
    )
    .await;
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            session.append_if("custom/panic", serde_json::json!({}), None, |_| {
                panic!("condition failed")
            })
        }))
        .is_err()
    );
    session
        .append("custom/recovered", serde_json::json!({}), None)
        .unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(session.events().len(), initial + 1);
}
