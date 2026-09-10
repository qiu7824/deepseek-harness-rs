use std::sync::{Arc, Mutex};

use cordis::Context;
use dsh_tools::{ToolDefinition, ToolExecutionInput, ToolOutputDefinition, ToolRuntime};

#[tokio::test]
async fn dropped_dispatch_restores_the_callers_signal() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    dsh_timeout_policy::apply(&ctx)().await;
    let (entered, observed) = tokio::sync::oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    tools
        .register(
            &ctx,
            ToolDefinition {
                name: "pending".into(),
                description: "pending operation".into(),
                parameters: serde_json::json!({"type":"object"}),
                output: ToolOutputDefinition {
                    schema: serde_json::json!({"type":"null"}),
                    render: Arc::new(|_, _| Ok(vec![])),
                    presentation_meta: None,
                },
                timeout_ms: Some(60_000),
                is_concurrency_safe: None,
                execute: Arc::new(move |_, run| {
                    entered
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(run.execution.clone())
                        .ok();
                    Box::pin(std::future::pending())
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )
        .unwrap();
    let upstream: dsh_tools::AbortPredicate = Arc::new(|| false);
    let input = ToolExecutionInput {
        call_id: dsh_llm::call_id("cancelled-dispatch"),
        root_call_id: None,
        name: "pending".into(),
        arguments: serde_json::json!({}),
        agent: None,
        parent: None,
        signal: upstream.clone(),
    };
    let task = tokio::spawn(async move { tools.execute(input).await });
    let execution = observed.await.unwrap();
    assert!(!Arc::ptr_eq(&execution.signal.lock(), &upstream));
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert!(Arc::ptr_eq(&execution.signal.lock(), &upstream));
}
