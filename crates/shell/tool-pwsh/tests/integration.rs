use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cordis::Context;
use dsh_jobs_local::LocalJobRegistry;
use dsh_llm::{ContentBlock, call_id};
use dsh_pwsh_local::{Config as PwshConfig, LocalPwshExecutor};
use dsh_subprocess_local::LocalSubprocessRuntime;
use dsh_system_prompt::SystemPrompt;
use dsh_tool_jobs::ToolJobsService;
use dsh_tool_pwsh::ToolPwshService;
use dsh_tools::{ToolExecutionInput, ToolExecutionResult, ToolRuntime};

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

async fn call(
    tools: &Arc<ToolRuntime>,
    name: &str,
    arguments: serde_json::Value,
) -> Arc<ToolExecutionResult> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    tools
        .execute(ToolExecutionInput {
            call_id: call_id(format!("pwsh-it-{}", NEXT.fetch_add(1, Ordering::SeqCst))),
            root_call_id: None,
            name: name.to_string(),
            arguments,
            agent: None,
            parent: None,
            signal: never_abort(),
        })
        .await
}

fn text(result: &ToolExecutionResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_pwsh_flows_through_jobs_output_and_kill() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    LocalJobRegistry::install(&ctx, dsh_jobs_local::Config::default());
    ToolJobsService::install(&ctx, dsh_tool_jobs::Config::default())
        .await
        .expect("job tools");
    LocalSubprocessRuntime::install(&ctx);
    LocalPwshExecutor::install(
        &ctx,
        PwshConfig {
            grace_ms: Some(200),
            ..Default::default()
        },
    );
    ToolPwshService::install(&ctx).expect("pwsh tool");

    let started = call(
        &tools,
        "pwsh",
        serde_json::json!({
            "command": "Write-Output bg-ready; Start-Sleep -Seconds 60",
            "description": "Start a background PowerShell process",
            "run_in_background": true
        }),
    )
    .await;
    assert!(!started.is_error, "{:?}", started.error);
    let job_id = started.value.as_ref().unwrap()["jobId"]
        .as_str()
        .expect("job id")
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = String::new();
    while Instant::now() < deadline && !output.contains("bg-ready") {
        let read = call(
            &tools,
            "job_output",
            serde_json::json!({ "job_id": job_id }),
        )
        .await;
        output.push_str(&text(&read));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(output.contains("bg-ready"), "{output}");

    let killed = call(&tools, "job_kill", serde_json::json!({ "job_id": job_id })).await;
    assert!(!killed.is_error, "{:?}", killed.error);
    let final_read = call(
        &tools,
        "job_output",
        serde_json::json!({ "job_id": job_id, "wait": true, "timeout_ms": 5_000 }),
    )
    .await;
    assert!(
        text(&final_read).contains("[status: killed"),
        "{}",
        text(&final_read)
    );
}
