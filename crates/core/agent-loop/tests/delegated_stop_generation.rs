#[path = "lifecycle_failures/support.rs"]
#[allow(dead_code)]
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use dsh_agent::{
    Agent, AgentCancelCause, AgentInboxMessagePayload, AgentStatus, CancelOptions, InboxTarget,
};
use support::{BlockingFirstTurnAdapter, harness, message, register_adapter};

fn adapter() -> Arc<BlockingFirstTurnAdapter> {
    Arc::new(BlockingFirstTurnAdapter {
        calls: AtomicUsize::new(0),
        first_entered: Arc::new(tokio::sync::Notify::new()),
        release_first: Arc::new(tokio::sync::Notify::new()),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_delegation_is_retained_without_waking_but_new_input_and_delegation_run() {
    let h = harness().await;
    let adapter = adapter();
    register_adapter(&h, adapter.clone());
    h.agent.followup(message("original parent task"));
    tokio::time::timeout(Duration::from_secs(2), adapter.first_entered.notified())
        .await
        .unwrap();
    let delegated_at = h.agent.cancellation_generation();
    for _ in 0..2 {
        h.agent.cancel(
            AgentCancelCause::User,
            Some(&CancelOptions { keep_inbox: true }),
        );
    }
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert!(!h.agent.send_from_generation(
        message("late child result"),
        InboxTarget::NextStep,
        delegated_at
    ));
    assert_eq!(h.agent.status(), AgentStatus::Idle);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.agent.inbox().next_turn().len(), 1);
    assert!(
        h.agent.inbox().next_step().is_empty(),
        "old delegation cannot steer a newer parent step"
    );

    h.agent.followup(message("explicit new user request"));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(
        adapter.calls.load(Ordering::SeqCst),
        3,
        "the retained result and explicit input each keep their own queued turn"
    );
    let durable = h
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "user/message")
        .map(|event| event.data.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(durable.contains("late child result"));
    assert!(durable.contains("explicit new user request"));

    let renewed = h.agent.cancellation_generation();
    assert_ne!(renewed, delegated_at);
    assert!(h.agent.send_from_generation(
        message("new delegation completed"),
        InboxTarget::NextTurn,
        renewed
    ));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synchronous_inbox_listener_can_stop_without_deadlock_or_late_wake() {
    let h = harness().await;
    let adapter = adapter();
    register_adapter(&h, adapter.clone());
    let cancelled = Arc::new(AtomicBool::new(false));
    let observed = cancelled.clone();
    h.ctx
        .on(
            "agent/inbox/inserted",
            Arc::new(move |_, args| {
                let payload = args
                    .first()
                    .and_then(|value| cordis::downcast_arc::<AgentInboxMessagePayload>(value));
                let observed = observed.clone();
                Box::pin(async move {
                    if let Some(payload) = payload {
                        if !observed.swap(true, Ordering::SeqCst) {
                            payload.agent.cancel(
                                AgentCancelCause::User,
                                Some(&CancelOptions { keep_inbox: true }),
                            );
                        }
                    }
                    None
                })
            }),
            Default::default(),
        )
        .await;
    let delegated_at = h.agent.cancellation_generation();
    let agent = h.agent.clone();
    let delivered = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking(move || {
            agent.send_from_generation(
                message("cancel in enqueue notification"),
                InboxTarget::NextTurn,
                delegated_at,
            )
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        !delivered,
        "wake permission must be rechecked after synchronous listeners"
    );
    assert!(cancelled.load(Ordering::SeqCst));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(h.agent.status(), AgentStatus::Idle);
    assert_eq!(h.agent.inbox().next_turn().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn normally_completed_parent_still_wakes_for_current_delegation() {
    let h = harness().await;
    let adapter = adapter();
    adapter.calls.store(1, Ordering::SeqCst);
    register_adapter(&h, adapter.clone());
    let delegated_at = h.agent.cancellation_generation();
    h.agent.followup(message("normally completed parent task"));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(h.agent.cancellation_generation(), delegated_at);
    assert!(h.agent.send_from_generation(
        message("timely child result"),
        InboxTarget::NextTurn,
        delegated_at
    ));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 3);
}
