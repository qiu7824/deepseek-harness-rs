use std::sync::Arc;

use cordis::Context;
use dsh_agent::AgentRegistry;
use dsh_goal::GoalService;
use dsh_llm::call_id;
use dsh_system_prompt::{AssembleContext, SystemPrompt};
use dsh_tool_goal::Config;
use dsh_tools::{
    ToolDefinition, ToolExecutionInput, ToolExecutionMode, ToolOutputDefinition, ToolRuntime,
};

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

fn tool_input(name: &str) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id(format!("call-{name}")),
        root_call_id: None,
        name: name.to_string(),
        arguments: serde_json::json!({}),
        agent: None,
        parent: None,
        signal: never_abort(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn registers_three_exclusive_tools_and_guidance_then_disposes_everything() {
    let ctx = Context::root();
    let system_prompt = SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());

    let disposer = dsh_tool_goal::apply(
        &ctx,
        &Config {
            blocked_after_consecutive_rounds: Some(5),
        },
    )
    .expect("apply");

    for name in ["create_goal", "get_goal", "update_goal"] {
        assert_eq!(
            tools.get(name, None).map(|tool| tool.name.clone()),
            Some(name.to_string())
        );
        assert_eq!(
            tools.execution_mode(&tool_input(name)),
            ToolExecutionMode::Exclusive
        );
    }
    let assembly = system_prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .expect("assemble");
    let section = assembly
        .sections
        .iter()
        .find(|section| section.name == "tool:goal")
        .expect("goal guidance");
    assert!(
        section.text.contains("infer goal intent"),
        "{}",
        section.text
    );
    assert!(
        section.text.contains("at least 5 consecutive goal rounds"),
        "{}",
        section.text
    );

    disposer().await;
    assert!(tools.get("get_goal", None).is_none());
    let assembly = system_prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .expect("assemble after dispose");
    assert!(
        !assembly
            .sections
            .iter()
            .any(|section| section.name == "tool:goal")
    );
}
