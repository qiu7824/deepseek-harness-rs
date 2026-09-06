use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize},
};
use std::time::Duration;

use dsh_agent::Agent;
use dsh_llm::{ChunkStream, GenerateOptions, LlmAdapter};

use super::support::{NamedToolThenTextAdapter, harness, message, quick_tool, register_adapter};

struct RecordingAdapter {
    delegate: NamedToolThenTextAdapter,
    requests: Arc<Mutex<Vec<String>>>,
}

impl LlmAdapter for RecordingAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(&options.messages).unwrap());
        self.delegate.stream(options)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_boundary_consumes_steering_and_task_changes_before_next_request() {
    let harness = harness().await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    register_adapter(
        &harness,
        Arc::new(RecordingAdapter {
            delegate: NamedToolThenTextAdapter {
                name: "quick",
                calls: AtomicUsize::new(0),
            },
            requests: Arc::clone(&requests),
        }),
    );
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut tool = quick_tool(Arc::new(AtomicBool::new(false)));
    let entered_for_tool = Arc::clone(&entered);
    let release_for_tool = Arc::clone(&release);
    tool.execute = Arc::new(move |_, run| {
        let entered = Arc::clone(&entered_for_tool);
        let release = Arc::clone(&release_for_tool);
        run.defer_context(message("background-result-marker"));
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            Ok(serde_json::json!("tool-completed"))
        })
    });
    harness.tools.register(&harness.ctx, tool).expect("tool");
    harness.agent.followup(message("original-request-marker"));
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    harness.agent.steer(message("steering-marker"));
    harness.agent.inject(message("updated-task-list-marker"));
    harness.agent.followup(message("queued-next-turn-marker"));
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for marker in [
        "steering-marker",
        "updated-task-list-marker",
        "background-result-marker",
    ] {
        assert!(!requests[0].contains(marker));
        assert!(
            requests[1].contains(marker),
            "{marker} must reach the very next request"
        );
    }
    assert!(!requests[1].contains("queued-next-turn-marker"));
    assert!(requests[2].contains("queued-next-turn-marker"));
    let steps: Vec<_> = harness
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "step/start")
        .map(|event| {
            (
                event.data["turn"].as_u64().unwrap(),
                event.data["step"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(steps, [(1, 1), (1, 2), (2, 1)]);
    assert!(!harness.agent.inbox().has_pending());
    assert!(requests[1].contains("tool-completed"));
    assert_eq!(harness.agent.status(), dsh_agent::AgentStatus::Idle);
    assert_eq!(requests[1].matches("steering-marker").count(), 1);
}
