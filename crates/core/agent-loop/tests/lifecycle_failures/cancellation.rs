use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dsh_agent::{Agent, AgentStatus};

use super::support::{
    DropFlag, NamedToolThenTextAdapter, ToolThenTextAdapter, hanging_tool, harness, message,
    quick_context_tool, register_adapter, turn_end_kinds,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_cancel_discards_old_queue_before_another_model_request() {
    let harness = harness().await;
    let entered = Arc::new(AtomicBool::new(false));
    harness
        .tools
        .register(
            &harness.ctx,
            hanging_tool(entered.clone(), Arc::new(AtomicBool::new(false))),
        )
        .unwrap();
    let adapter = Arc::new(ToolThenTextAdapter {
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("original-running-request"));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    harness.agent.followup(message("old-queued-request"));
    harness.agent.steer(message("old-steering-request"));
    harness
        .agent
        .cancel(dsh_agent::AgentCancelCause::User, None);
    tokio::time::timeout(Duration::from_secs(1), harness.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(
        adapter.calls.load(Ordering::SeqCst),
        1,
        "cancellation must not restart queued work"
    );
    assert!(!harness.agent.inbox().has_pending());
    assert_eq!(turn_end_kinds(&harness.agent), ["aborted"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_input_during_cancel_cleanup_is_not_discarded_with_the_old_queue() {
    let harness = harness().await;
    let entered = Arc::new(AtomicBool::new(false));
    let finalizing = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let mut tool = hanging_tool(entered.clone(), Arc::new(AtomicBool::new(false)));
    let inside = finalizing.clone();
    let finish = release.clone();
    tool.finalize_content = Some(Arc::new(move |_, _| {
        inside.store(true, Ordering::SeqCst);
        while !finish.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        None
    }));
    harness.tools.register(&harness.ctx, tool).unwrap();
    register_adapter(
        &harness,
        Arc::new(ToolThenTextAdapter {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    harness.agent.followup(message("original-running-request"));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    harness.agent.followup(message("old-queued-request"));
    harness
        .agent
        .cancel(dsh_agent::AgentCancelCause::User, None);
    let ready = tokio::time::timeout(Duration::from_secs(1), async {
        while !finalizing.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    if ready.is_err() {
        release.store(true, Ordering::SeqCst);
        panic!("cancellation finalizer did not start");
    }
    harness
        .agent
        .followup(message("fresh-request-after-cancel"));
    release.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(1), harness.agent.when_idle())
        .await
        .unwrap();
    let events = harness.agent.session().events();
    let consumed = events
        .iter()
        .filter(|event| event.type_ == "user/message")
        .map(|event| event.data.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(consumed.contains("fresh-request-after-cancel"));
    assert!(!consumed.contains("old-queued-request"));
    assert_eq!(turn_end_kinds(&harness.agent), ["aborted", "completed"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_interrupts_a_non_cooperative_post_execute_hook() {
    let harness = harness().await;
    let body_entered = Arc::new(AtomicBool::new(false));
    let post_entered = Arc::new(AtomicBool::new(false));
    let post_dropped = Arc::new(AtomicBool::new(false));
    let finalized = Arc::new(AtomicBool::new(false));
    let notified = Arc::new(AtomicBool::new(false));

    let post_entered_for_listener = Arc::clone(&post_entered);
    let post_dropped_for_listener = Arc::clone(&post_dropped);
    harness
        .ctx
        .on(
            "tools/post-execute",
            Arc::new(move |_ctx, _args| {
                let entered = Arc::clone(&post_entered_for_listener);
                let dropped = Arc::clone(&post_dropped_for_listener);
                Box::pin(async move {
                    let _drop_flag = DropFlag(dropped);
                    entered.store(true, Ordering::SeqCst);
                    futures::future::pending::<Option<cordis::ArcValue>>().await
                })
            }),
            cordis::EventOptions::default().global(true).prepend(true),
        )
        .await;
    let notified_for_listener = Arc::clone(&notified);
    harness
        .ctx
        .on(
            "tools/result",
            Arc::new(move |_ctx, _args| {
                let notified = Arc::clone(&notified_for_listener);
                Box::pin(async move {
                    notified.store(true, Ordering::SeqCst);
                    None
                })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;
    let mut tool = quick_context_tool(Arc::clone(&body_entered));
    let finalized_for_tool = Arc::clone(&finalized);
    tool.finalize_content = Some(Arc::new(move |_, _| {
        finalized_for_tool.store(true, Ordering::SeqCst);
        None
    }));
    harness
        .tools
        .register(&harness.ctx, tool)
        .expect("register tool behind post-execute hook");
    register_adapter(
        &harness,
        Arc::new(NamedToolThenTextAdapter {
            name: "quick-context",
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );

    harness.agent.followup(message("wait in post-execute"));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !post_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("post-execute hook must start after the tool body");
    assert!(body_entered.load(Ordering::SeqCst));
    harness.agent.cancel(
        dsh_agent::AgentCancelCause::User,
        Some(&dsh_agent::CancelOptions { keep_inbox: true }),
    );
    tokio::time::timeout(Duration::from_millis(250), harness.agent.when_idle())
        .await
        .expect("cancel must interrupt a non-cooperative post-execute hook");
    assert!(
        post_dropped.load(Ordering::SeqCst),
        "post-execute future was not dropped"
    );
    assert_eq!(harness.agent.status(), AgentStatus::Idle);
    assert_eq!(turn_end_kinds(&harness.agent), ["aborted", "completed"]);
    let events = harness.agent.session().events();
    let result = events
        .iter()
        .find(|event| event.type_ == "tool/result")
        .expect("cancelled post-execute result");
    assert_eq!(result.data["error"]["code"], "ABORTED");
    assert!(
        finalized.load(Ordering::SeqCst),
        "tool finalizer was skipped"
    );
    assert!(
        notified.load(Ordering::SeqCst),
        "tools/result was not emitted"
    );
    assert!(harness.agent.inbox().next_step().is_empty());
    let consumed_context = harness
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "user/message")
        .any(|event| event.data["content"][0]["text"] == "quick-context");
    assert!(consumed_context, "deferred context was not consumed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_drops_a_non_cooperative_tool_future_and_settles_idle() {
    let harness = harness().await;
    let entered = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let finalized = Arc::new(AtomicBool::new(false));
    let notified = Arc::new(AtomicBool::new(false));
    let notified_for_listener = Arc::clone(&notified);
    harness
        .ctx
        .on(
            "tools/result",
            Arc::new(move |_ctx, _args| {
                let notified = Arc::clone(&notified_for_listener);
                Box::pin(async move {
                    notified.store(true, Ordering::SeqCst);
                    None
                })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;
    let mut tool = hanging_tool(Arc::clone(&entered), Arc::clone(&dropped));
    let finalized_for_tool = Arc::clone(&finalized);
    tool.finalize_content = Some(Arc::new(move |_, _| {
        finalized_for_tool.store(true, Ordering::SeqCst);
        None
    }));
    harness
        .tools
        .register(&harness.ctx, tool)
        .expect("register hanging tool");
    register_adapter(
        &harness,
        Arc::new(ToolThenTextAdapter {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );

    harness.agent.followup(message("run hanging tool"));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("tool body must start");
    harness.agent.cancel(
        dsh_agent::AgentCancelCause::User,
        Some(&dsh_agent::CancelOptions { keep_inbox: true }),
    );
    tokio::time::timeout(Duration::from_millis(250), harness.agent.when_idle())
        .await
        .expect("cancel must not wait for a non-cooperative tool body");
    assert!(
        dropped.load(Ordering::SeqCst),
        "tool future was not dropped"
    );
    assert_eq!(harness.agent.status(), AgentStatus::Idle);
    assert_eq!(turn_end_kinds(&harness.agent), ["aborted"]);
    let events = harness.agent.session().events();
    let calls = events
        .iter()
        .filter(|event| event.type_ == "tool/call")
        .count();
    let results = events
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .count();
    assert_eq!(
        calls, results,
        "started tool calls must stay durably paired"
    );
    let result = events
        .iter()
        .find(|event| event.type_ == "tool/result")
        .expect("cancelled started tool result");
    assert_eq!(result.data["error"]["code"], "ABORTED");
    assert!(
        finalized.load(Ordering::SeqCst),
        "tool finalizer was skipped"
    );
    assert!(
        notified.load(Ordering::SeqCst),
        "tools/result was not emitted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_interrupts_a_non_cooperative_pre_execute_hook() {
    let harness = harness().await;
    let pre_entered = Arc::new(AtomicBool::new(false));
    let body_entered = Arc::new(AtomicBool::new(false));
    let body_dropped = Arc::new(AtomicBool::new(false));
    let pre_entered_for_listener = Arc::clone(&pre_entered);
    harness
        .ctx
        .on(
            "tools/pre-execute",
            Arc::new(move |_ctx, _args| {
                let entered = Arc::clone(&pre_entered_for_listener);
                Box::pin(async move {
                    entered.store(true, Ordering::SeqCst);
                    futures::future::pending::<Option<cordis::ArcValue>>().await
                })
            }),
            cordis::EventOptions::default().global(true).prepend(true),
        )
        .await;
    harness
        .tools
        .register(
            &harness.ctx,
            hanging_tool(Arc::clone(&body_entered), body_dropped),
        )
        .expect("register tool behind pre-execute hook");
    register_adapter(
        &harness,
        Arc::new(ToolThenTextAdapter {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    );

    harness.agent.followup(message("wait in pre-execute"));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !pre_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pre-execute hook must start");
    harness.agent.cancel(
        dsh_agent::AgentCancelCause::User,
        Some(&dsh_agent::CancelOptions { keep_inbox: true }),
    );
    tokio::time::timeout(Duration::from_millis(250), harness.agent.when_idle())
        .await
        .expect("cancel must interrupt a non-cooperative pre-execute hook");
    assert!(!body_entered.load(Ordering::SeqCst));
    let events = harness.agent.session().events();
    let result = events
        .iter()
        .find(|event| event.type_ == "tool/result")
        .expect("pre-dispatch cancellation result");
    assert_eq!(result.data["error"]["code"], "ABORTED_BEFORE_DISPATCH");
}
