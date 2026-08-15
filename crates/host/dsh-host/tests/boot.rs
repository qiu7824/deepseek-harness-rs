//! Host-spine boot integration: the shared composition plus the binary's
//! report contract, exercised in-process.
//!
//! # Deviations
//!
//! - The composition nests `futures::executor::block_on` for synchronous
//!   installers (SQLite open, inject fibers); the tests run on a
//!   multi-threaded runtime so those nested executors can always make
//!   progress (a current-thread runtime can deadlock).

use std::sync::Arc;

use cordis::Context;
use dsh_host::{compose_host, mount_companions};
use dsh_session::{Session, SessionId, session_id};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composes_the_core_spine_and_boots_a_report() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    mount_companions(&spine);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // Every core service resolves through its registered name.
    assert!(ctx
        .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
        .is_some());
    assert!(ctx
        .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
        .is_some());
    assert!(ctx
        .get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
        .is_some());
    assert!(ctx
        .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
        .is_some());

    let report = dsh_host::boot_report(&spine).await.expect("report");
    assert_eq!(
        report["session"]["id"],
        serde_json::json!("host-boot")
    );
    assert_eq!(report["session"]["seq"], serde_json::json!(1));
    assert!(report["services"].as_array().is_some_and(|items| items.len() == 10));
    // The durability + FTS5 probe observes both the live and the
    // persisted-only corpus.
    assert_eq!(report["probe"]["flushAcknowledged"], serde_json::json!(true));
    assert_eq!(report["probe"]["liveSearchHits"], serde_json::json!(1));
    assert_eq!(report["probe"]["persistedSearchHits"], serde_json::json!(1));

    // A second report over the same composition reuses the boot session id
    // (the store rejects duplicates), so the spine reports once per process.
    let duplicate = dsh_host::boot_report(&spine).await;
    assert!(duplicate.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_created_through_the_spine_are_live() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let session = spine
        .sessions
        .create(
            &ctx,
            Some(session_id("spine-session")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    assert!(spine.sessions.get(&session_id("spine-session")).is_some());
    assert_eq!(session.id(), &session_id("spine-session"));
}

// Keep the SessionId/Session imports referenced for parity documentation.
#[allow(dead_code)]
fn _vocabulary(_session: &Session, _id: &SessionId) {}
