use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cordis::{Context, EventOptions, arc};
use dsh_workflow::{
    WorkflowEngine, WorkflowError, WorkflowErrorCode, WorkflowEvent, WorkflowMeta, WorkflowResult,
    WorkflowRunInfo, is_fatal_workflow_error, new_workflow_run, workflow_run_id,
};

struct StubEngine {
    ctx: Context,
}

impl WorkflowEngine for StubEngine {
    fn context(&self) -> &Context {
        &self.ctx
    }

    fn start(
        &self,
        _request: dsh_workflow::WorkflowStartRequest,
    ) -> Result<Arc<dyn dsh_workflow::WorkflowRun>, WorkflowError> {
        Err(WorkflowError::new(
            "not under test",
            WorkflowErrorCode::ScriptParse,
        ))
    }
}

fn meta() -> WorkflowMeta {
    WorkflowMeta {
        name: "audit".to_string(),
        description: "audit files".to_string(),
        when_to_use: None,
        phases: Vec::new(),
    }
}

#[test]
fn browser_safe_types_and_typed_fatal_errors_keep_wire_identity() {
    let id = workflow_run_id("run-1");
    assert_eq!(id.as_str(), "run-1");
    assert_eq!(
        serde_json::to_value(&id).unwrap(),
        serde_json::json!("run-1")
    );

    let fatal = WorkflowError::new("cap hit", WorkflowErrorCode::AgentCap);
    assert_eq!(fatal.code(), "AGENT_CAP");
    assert!(fatal.fatal);
    assert!(is_fatal_workflow_error(&fatal));

    let soft = WorkflowError::with_fatal("advisory", WorkflowErrorCode::ItemCap, false);
    assert!(!is_fatal_workflow_error(&soft));
    assert!(!is_fatal_workflow_error(&std::io::Error::other("plain")));
}

#[tokio::test(flavor = "current_thread")]
async fn holder_run_result_only_resolves_and_cancel_dispose_are_idempotent_and_bounded() {
    let cancellations = Arc::new(AtomicUsize::new(0));
    let cleanups = Arc::new(AtomicUsize::new(0));
    let cancel_count = cancellations.clone();
    let cleanup_count = cleanups.clone();
    let (run, _controller) = new_workflow_run(
        workflow_run_id("run-1"),
        meta(),
        Duration::from_millis(20),
        Arc::new(move |_reason| {
            cancel_count.fetch_add(1, Ordering::SeqCst);
        }),
        Arc::new(move || {
            cleanup_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::pending())
        }),
    );

    run.cancel(Some("user".to_string()));
    run.cancel(Some("duplicate".to_string()));
    let result = run.result().await;
    assert_eq!(result.stop_reason.as_str(), "cancelled");
    assert_eq!(result.error.as_deref(), Some("user"));

    tokio::time::timeout(Duration::from_millis(100), run.dispose())
        .await
        .expect("dispose must be bounded");
    run.dispose().await;
    assert_eq!(cancellations.load(Ordering::SeqCst), 1);
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn event_listener_failure_is_contained_per_listener() {
    let ctx = Context::root();
    let reached = Arc::new(AtomicUsize::new(0));
    ctx.on(
        "workflow/phase",
        Arc::new(
            |_ctx, _args| -> cordis::BoxFuture<'static, Option<cordis::ArcValue>> {
                panic!("bad listener")
            },
        ),
        EventOptions::default(),
    )
    .await;
    let reached_peer = reached.clone();
    ctx.on(
        "workflow/phase",
        Arc::new(move |_ctx, _args| {
            reached_peer.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { None })
        }),
        EventOptions::default(),
    )
    .await;

    let engine = StubEngine { ctx };
    engine.emit_workflow_event(WorkflowEvent::Phase {
        run: WorkflowRunInfo {
            id: workflow_run_id("run-1"),
            meta: meta(),
        },
        title: "Scan".to_string(),
    });
    tokio::task::yield_now().await;
    assert_eq!(reached.load(Ordering::SeqCst), 1);
    drop(arc(()));
}

#[tokio::test(flavor = "current_thread")]
async fn controller_settles_a_completed_run_without_a_rejection_channel() {
    let (run, controller) = new_workflow_run(
        workflow_run_id("run-2"),
        meta(),
        Duration::from_millis(20),
        Arc::new(|_| {}),
        Arc::new(|| Box::pin(async {})),
    );
    assert!(controller.settle(WorkflowResult::completed(
        serde_json::json!({"ok": true}),
        3
    )));
    assert!(!controller.settle(WorkflowResult::error("late", 3)));
    let result = run.result().await;
    assert_eq!(result.value, serde_json::json!({"ok": true}));
    assert_eq!(result.agents_started, 3);
}
