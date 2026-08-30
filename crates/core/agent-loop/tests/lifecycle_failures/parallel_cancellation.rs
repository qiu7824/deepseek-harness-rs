use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dsh_agent::Agent;
use dsh_agent_loop::execute_tool_calls;
use dsh_llm::{ContentBlock, call_id};

use super::support::{
    ParallelToolAdapter, hanging_tool, harness, message, quick_context_tool, quick_tool,
    register_adapter,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_pairs_every_started_parallel_tool_call() {
    let harness = harness().await;
    let slow_entered = Arc::new(AtomicBool::new(false));
    let slow_dropped = Arc::new(AtomicBool::new(false));
    let quick_entered = Arc::new(AtomicBool::new(false));
    let mut slow = hanging_tool(Arc::clone(&slow_entered), Arc::clone(&slow_dropped));
    slow.is_concurrency_safe = Some(Arc::new(|_| true));
    harness
        .tools
        .register(&harness.ctx, slow)
        .expect("register slow tool");
    harness
        .tools
        .register(&harness.ctx, quick_tool(Arc::clone(&quick_entered)))
        .expect("register quick tool");
    register_adapter(&harness, Arc::new(ParallelToolAdapter));

    harness.agent.followup(message("run parallel tools"));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !slow_entered.load(Ordering::SeqCst) || !quick_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both parallel tool bodies must start");
    harness.agent.cancel(
        dsh_agent::AgentCancelCause::User,
        Some(&dsh_agent::CancelOptions { keep_inbox: true }),
    );
    tokio::time::timeout(Duration::from_millis(250), harness.agent.when_idle())
        .await
        .expect("parallel cancellation must settle idle");
    assert!(slow_dropped.load(Ordering::SeqCst));

    let events = harness.agent.session().events();
    let calls = events
        .iter()
        .filter(|event| event.type_ == "tool/call")
        .count();
    let results = events
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .count();
    assert_eq!(calls, 2);
    assert_eq!(results, calls);
    let codes: Vec<_> = events
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .filter_map(|event| event.data["error"]["code"].as_str())
        .collect();
    assert!(codes.contains(&"ABORTED"), "results={events:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_preserves_a_settled_parallel_result_waiting_for_model_order() {
    let harness = harness().await;
    let slow_entered = Arc::new(AtomicBool::new(false));
    let slow_dropped = Arc::new(AtomicBool::new(false));
    let quick_entered = Arc::new(AtomicBool::new(false));
    let mut slow = hanging_tool(Arc::clone(&slow_entered), Arc::clone(&slow_dropped));
    slow.is_concurrency_safe = Some(Arc::new(|_| true));
    harness
        .tools
        .register(&harness.ctx, slow)
        .expect("register slow tool");
    harness
        .tools
        .register(&harness.ctx, quick_tool(Arc::clone(&quick_entered)))
        .expect("register quick tool");
    register_adapter(&harness, Arc::new(ParallelToolAdapter));

    harness
        .agent
        .followup(message("preserve settled parallel result"));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !slow_entered.load(Ordering::SeqCst) || !quick_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both parallel tool bodies must start");
    // The quick body has returned, but its model-ordered result cannot commit
    // while the preceding slow call is still pending. Give the scheduler a
    // deterministic opportunity to move that completion into its slot.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    harness.agent.cancel(
        dsh_agent::AgentCancelCause::User,
        Some(&dsh_agent::CancelOptions { keep_inbox: true }),
    );
    tokio::time::timeout(Duration::from_millis(250), harness.agent.when_idle())
        .await
        .expect("parallel cancellation must settle idle");
    assert!(slow_dropped.load(Ordering::SeqCst));

    let events = harness.agent.session().events();
    let results: Vec<_> = events
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .collect();
    assert_eq!(results.len(), 2, "results={results:#?}");
    assert_eq!(results[0].data["error"]["code"], "ABORTED");
    assert!(
        results[1].data.get("error").is_none(),
        "settled quick result was overwritten by cancellation: {results:#?}"
    );
    assert_eq!(
        results[1].data["message"]["content"][0]["content"][0]["text"],
        "quick"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_commits_settled_parallel_context_and_conclusion() {
    let harness = harness().await;
    let slow_entered = Arc::new(AtomicBool::new(false));
    let slow_dropped = Arc::new(AtomicBool::new(false));
    let quick_entered = Arc::new(AtomicBool::new(false));
    let mut slow = hanging_tool(Arc::clone(&slow_entered), Arc::clone(&slow_dropped));
    slow.is_concurrency_safe = Some(Arc::new(|_| true));
    harness
        .tools
        .register(&harness.ctx, slow)
        .expect("register slow tool");
    harness
        .tools
        .register(&harness.ctx, quick_context_tool(Arc::clone(&quick_entered)))
        .expect("register quick context tool");

    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_for_signal = Arc::clone(&cancelled);
    let accepted = Arc::new(std::sync::Mutex::new(Vec::new()));
    let accepted_for_callback = Arc::clone(&accepted);
    let tools = Arc::clone(&harness.tools);
    let agent: Arc<dyn Agent> = harness.agent.clone();
    let scheduler = tokio::spawn(async move {
        execute_tool_calls(
            &tools,
            agent,
            10,
            1,
            1,
            vec![
                dsh_llm::ToolCallBlock {
                    id: call_id("slow-context"),
                    name: "hang".to_string(),
                    arguments: "{}".to_string(),
                },
                dsh_llm::ToolCallBlock {
                    id: call_id("quick-context"),
                    name: "quick-context".to_string(),
                    arguments: "{}".to_string(),
                },
            ],
            Arc::new(move || cancelled_for_signal.load(Ordering::SeqCst)),
            Arc::new(move |context| {
                accepted_for_callback
                    .lock()
                    .expect("accepted context lock")
                    .push(context);
            }),
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(1), async {
        while !slow_entered.load(Ordering::SeqCst) || !quick_entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both parallel tool bodies must start");
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancelled.store(true, Ordering::SeqCst);

    let concluded = tokio::time::timeout(Duration::from_millis(250), scheduler)
        .await
        .expect("scheduler cancellation must settle")
        .expect("scheduler task")
        .expect("execute tool calls");
    assert!(slow_dropped.load(Ordering::SeqCst));
    assert!(concluded, "settled tool conclusion was dropped");
    let accepted = accepted.lock().expect("accepted context lock");
    assert_eq!(accepted.len(), 1, "accepted={accepted:#?}");
    assert_eq!(
        accepted[0].content,
        vec![ContentBlock::Text {
            text: "quick-context".to_string()
        }]
    );
}
