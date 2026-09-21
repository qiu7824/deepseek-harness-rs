#[path = "lifecycle_failures/support.rs"]
#[allow(dead_code)]
mod support;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use dsh_agent::{Agent, AgentCancelCause, AgentControlBusy, AgentStatus, InboxTarget};
use support::{BlockingFirstTurnAdapter, harness, message, register_adapter};

fn adapter() -> Arc<BlockingFirstTurnAdapter> {
    Arc::new(BlockingFirstTurnAdapter {
        calls: AtomicUsize::new(1),
        first_entered: Arc::new(tokio::sync::Notify::new()),
        release_first: Arc::new(tokio::sync::Notify::new()),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_message_routes_wait_for_control_commit_and_preserve_every_input() {
    let h = harness().await;
    register_adapter(&h, adapter());
    let runtime = tokio::runtime::Handle::current();
    let generation = h.agent.cancellation_generation();
    let control = h.agent.try_idle_control().unwrap();
    let (started, observed) = std::sync::mpsc::channel();
    let (finished, completed) = std::sync::mpsc::channel();
    let committed = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    let mut ids = HashSet::new();
    for route in 0..6 {
        let input = message(&format!("queued route {route}"));
        ids.insert(input.id.as_str().to_owned());
        let (agent, runtime, started, finished, committed) = (
            h.agent.clone(),
            runtime.clone(),
            started.clone(),
            finished.clone(),
            committed.clone(),
        );
        workers.push(std::thread::spawn(move || {
            let _runtime = runtime.enter();
            started.send(()).unwrap();
            match route {
                0 => agent.send(input, InboxTarget::NextTurn, false),
                1 => agent.followup(input), // schedule, goal and job delivery use this path
                2 => agent.steer(input),
                3 => agent.inject(input),
                4 => agent.send_with_context(input, InboxTarget::NextTurn, None),
                5 => {
                    agent.send_from_generation(input, InboxTarget::NextStep, generation);
                }
                _ => unreachable!(),
            }
            assert!(
                committed.load(Ordering::SeqCst),
                "message publication crossed the control transaction"
            );
            finished.send(()).unwrap();
        }));
    }
    for _ in 0..6 {
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    assert!(completed.recv_timeout(Duration::from_millis(30)).is_err());
    assert!(!h.agent.inbox().has_pending());
    assert_eq!(h.agent.status(), AgentStatus::Idle);
    assert_eq!(
        h.agent.try_idle_control().err(),
        Some(AgentControlBusy::Contended)
    );
    committed.store(true, Ordering::SeqCst);
    drop(control);
    for _ in 0..6 {
        completed.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    for worker in workers {
        worker.join().unwrap();
    }
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    let consumed: Vec<String> = h
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "user/message")
        .filter_map(|event| {
            event
                .data
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        })
        .chain(
            h.agent
                .inbox()
                .next_turn()
                .into_iter()
                .chain(h.agent.inbox().next_step())
                .map(|input| input.id.as_str().to_owned()),
        )
        .collect();
    for id in ids {
        assert_eq!(
            consumed.iter().filter(|observed| **observed == id).count(),
            1,
            "publication must neither drop nor duplicate input {id}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_rejects_pending_input_maintenance_running_and_driver_handoff() {
    let h = harness().await;
    let adapter = adapter();
    register_adapter(&h, adapter.clone());
    h.agent.inject(message("pending context"));
    assert_eq!(
        h.agent.try_idle_control().err(),
        Some(AgentControlBusy::PendingInput)
    );
    h.agent.cancel(AgentCancelCause::User, None);
    let maintenance = h.agent.run_maintenance(Arc::new(|| Box::pin(async {})));
    assert_eq!(h.agent.status(), AgentStatus::Idle);
    assert_eq!(
        h.agent.try_idle_control().err(),
        Some(AgentControlBusy::Active),
        "maintenance is busy even though the public status is Idle"
    );
    drop(maintenance);
    drop(h.agent.try_idle_control().unwrap());

    let saw_handoff = Arc::new(AtomicBool::new(false));
    let observed = saw_handoff.clone();
    h.ctx
        .on(
            "agent/status",
            Arc::new(move |_, args| {
                let payload = args
                    .first()
                    .and_then(|value| cordis::downcast_arc::<dsh_agent::AgentStatusPayload>(value));
                let observed = observed.clone();
                Box::pin(async move {
                    if let Some(payload) = payload {
                        if payload.status == AgentStatus::Idle {
                            assert!(matches!(
                                payload.agent.try_idle_control().err(),
                                Some(AgentControlBusy::Active | AgentControlBusy::Contended)
                            ));
                            observed.store(true, Ordering::SeqCst);
                        }
                    }
                    None
                })
            }),
            Default::default(),
        )
        .await;
    adapter.calls.store(0, Ordering::SeqCst);
    h.agent.followup(message("running parent"));
    tokio::time::timeout(Duration::from_secs(2), adapter.first_entered.notified())
        .await
        .unwrap();
    assert_eq!(
        h.agent.try_idle_control().err(),
        Some(AgentControlBusy::Active)
    );
    h.agent.cancel(AgentCancelCause::User, None);
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert!(saw_handoff.load(Ordering::SeqCst));
    drop(h.agent.try_idle_control().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_callbacks_cannot_enter_control_during_publication() {
    let h = harness().await;
    let rejected = Arc::new(AtomicBool::new(false));
    let observed = rejected.clone();
    h.ctx
        .on(
            "agent/inbox/inserted",
            Arc::new(move |_, args| {
                let payload = args.first().and_then(|value| {
                    cordis::downcast_arc::<dsh_agent::AgentInboxMessagePayload>(value)
                });
                let observed = observed.clone();
                Box::pin(async move {
                    if let Some(payload) = payload {
                        assert_eq!(
                            payload.agent.try_idle_control().err(),
                            Some(AgentControlBusy::Contended)
                        );
                        observed.store(true, Ordering::SeqCst);
                    }
                    None
                })
            }),
            Default::default(),
        )
        .await;
    h.agent.inject(message("notify while publishing"));
    assert!(rejected.load(Ordering::SeqCst));
}
