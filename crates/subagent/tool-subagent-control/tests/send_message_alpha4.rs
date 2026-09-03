use cordis::Context;
use dsh_subagent::SubagentRuntime;
use dsh_system_prompt::{Config as SystemPromptConfig, SystemPrompt};
use dsh_tool_subagent_control::apply;
use dsh_tools::{Config as ToolsConfig, ToolRuntime};

#[tokio::test]
async fn send_message_schema_uses_agent_id_and_adjacent_agent_contract() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, SystemPromptConfig::default()).expect("system prompt");
    let tools = ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SubagentRuntime::install(&ctx);
    let _disposers = apply(&ctx).expect("control tools");

    let send = tools.get("send_message", None).expect("send_message");
    let properties = send.parameters["properties"]
        .as_object()
        .expect("properties");
    let mut names = properties.keys().map(String::as_str).collect::<Vec<_>>();
    names.sort_unstable();

    assert_eq!(names, vec!["agent_id", "message"]);
    assert_eq!(
        send.parameters["required"],
        serde_json::json!(["agent_id", "message"])
    );
    assert!(send.description.contains("direct continuable child"));
    assert!(send.description.contains("resident continuable child"));
    assert!(send.description.contains("nearest step"));
    assert_eq!(
        properties["agent_id"]["description"],
        "The agent id of your direct continuable child, or your direct parent when you are a resident continuable child."
    );

    let rendered = (send.output.render)(
        &serde_json::json!({ "agent_id": "child-1", "message": "hello" }),
        &serde_json::json!({ "messageId": "message-1" }),
    )
    .expect("render");
    assert_eq!(
        rendered,
        vec![dsh_llm::ContentBlock::Text {
            text: "message delivered to agent child-1".to_string(),
        }]
    );
}
