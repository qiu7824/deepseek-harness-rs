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

fn placeholder_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: "pre-existing test tool".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {}
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
            render: Arc::new(|_, _| Ok(Vec::new())),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(|_, _| Box::pin(async { Ok(serde_json::json!({})) })),
        finalize_content: None,
        present_call: None,
        present_result: None,
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

#[tokio::test(flavor = "current_thread")]
async fn failed_registration_leaves_no_partial_tools_or_prompt_section() {
    let ctx = Context::root();
    let system_prompt = SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    AgentRegistry::install(&ctx);
    let tools = ToolRuntime::install(&ctx, Default::default()).expect("tools");
    GoalService::install(&ctx, Default::default());
    tools
        .register(&ctx, placeholder_tool("create_goal"))
        .expect("reserve create_goal");

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_tool_goal::apply(&ctx, &Config::default())
    }));
    assert!(
        outcome.is_err() || outcome.as_ref().is_ok_and(|result| result.is_err()),
        "the conflicting installation must reject"
    );
    assert!(tools.get("get_goal", None).is_none(), "get_goal leaked");
    assert!(
        tools.get("update_goal", None).is_none(),
        "update_goal leaked"
    );
    assert!(
        tools.get("create_goal", None).is_some(),
        "pre-existing tool removed"
    );
    let assembly = system_prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .expect("assemble");
    assert!(
        !assembly
            .sections
            .iter()
            .any(|section| section.name == "tool:goal"),
        "guidance section leaked from the rejected install"
    );
}
