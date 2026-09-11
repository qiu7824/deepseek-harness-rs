use std::sync::Arc;

use cordis::Context;
use dsh_tools::{ToolExecutionInput, ToolRuntime};

#[tokio::test]
async fn shipped_schema_mounts_and_timeout_bounds_are_enforced_before_execution() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    dsh_subprocess_local::LocalSubprocessRuntime::install(&ctx);
    dsh_jobs_local::LocalJobRegistry::install(&ctx, Default::default());
    let missing = std::env::temp_dir().join(format!(
        "dsh-schema-missing-pwsh-{}.exe",
        std::process::id()
    ));
    assert!(!missing.exists());
    dsh_pwsh_local::LocalPwshExecutor::install(
        &ctx,
        dsh_pwsh_local::Config {
            pwsh_path: Some(missing.to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    dsh_tool_pwsh::ToolPwshService::install(&ctx).unwrap();
    for (timeout, expected) in [
        (0, "TOOL_INPUT_INVALID"),
        (600_001, "TOOL_INPUT_INVALID"),
        (3000, "SHELL_STARTUP_FAILED"),
    ] {
        let result = tools.execute(ToolExecutionInput {
            call_id: dsh_llm::call_id(format!("timeout-{timeout}")), root_call_id: None,
            name: "pwsh".into(), arguments: serde_json::json!({"command":"Write-Output never", "description":"schema validation", "timeout_ms":timeout}),
            agent: None, parent: None, signal: Arc::new(|| false),
        }).await;
        assert_eq!(
            result.error.as_ref().unwrap().info.as_ref().unwrap().code,
            expected
        );
    }
}
