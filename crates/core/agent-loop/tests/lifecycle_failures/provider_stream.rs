use std::sync::Arc;
use std::time::Duration;

use dsh_agent::{Agent, AgentStatus};

use super::support::{
    BlockingFirstTurnAdapter, ErrorAdapter, harness, message, register_adapter, turn_end_kinds,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_stream_error_settles_idle_and_accepts_the_next_prompt() {
    let harness = harness().await;
    register_adapter(&harness, Arc::new(ErrorAdapter));

    harness.agent.followup(message("first"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .expect("failed turn must settle idle");
    assert_eq!(harness.agent.status(), AgentStatus::Idle);
    assert_eq!(turn_end_kinds(&harness.agent), ["error"]);

    harness.agent.followup(message("second"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .expect("second prompt must not remain gated after failure");
    assert_eq!(harness.agent.status(), AgentStatus::Idle);
    assert_eq!(turn_end_kinds(&harness.agent), ["error", "error"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_arriving_during_turn_shutdown_runs_as_the_next_turn() {
    let harness = harness().await;
    let first_entered = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let adapter = Arc::new(BlockingFirstTurnAdapter {
        calls: std::sync::atomic::AtomicUsize::new(0),
        first_entered: Arc::clone(&first_entered),
        release_first: Arc::clone(&release_first),
    });
    register_adapter(&harness, adapter.clone());

    harness.agent.followup(message("first"));
    tokio::time::timeout(Duration::from_secs(3), first_entered.notified())
        .await
        .expect("first request must enter the adapter");
    harness.agent.followup(message("second"));
    release_first.notify_one();

    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .expect("a prompt queued during turn shutdown must not remain pending");
    assert_eq!(adapter.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(turn_end_kinds(&harness.agent), ["completed", "completed"]);
    assert!(!harness.agent.inbox().has_pending());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_prompts_across_idle_boundaries_are_all_consumed() {
    let harness = harness().await;
    let first_entered = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let adapter = Arc::new(BlockingFirstTurnAdapter {
        calls: std::sync::atomic::AtomicUsize::new(0),
        first_entered: Arc::clone(&first_entered),
        release_first: Arc::clone(&release_first),
    });
    register_adapter(&harness, adapter.clone());

    harness.agent.followup(message("prompt-0"));
    tokio::time::timeout(Duration::from_secs(3), first_entered.notified())
        .await
        .expect("first request must enter the adapter");
    release_first.notify_one();
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .expect("first prompt must settle idle");

    for index in 1..=200 {
        harness.agent.followup(message(&format!("prompt-{index}")));
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap_or_else(|_| panic!("prompt {index} remained pending across an idle boundary"));
        assert_eq!(harness.agent.status(), AgentStatus::Idle);
        assert!(!harness.agent.inbox().has_pending());
    }

    assert_eq!(adapter.calls.load(std::sync::atomic::Ordering::SeqCst), 201);
    assert_eq!(turn_end_kinds(&harness.agent).len(), 201);
}
