use cordis::{Context, arc};
use dsh_llm::{ContentBlock, call_id};
use dsh_tools::{ToolExecutionInput, ToolRuntime};
use std::{path::PathBuf, sync::Arc};

fn text(result: &dsh_tools::ToolExecutionResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn registers_real_glob_and_grep_and_searches_disk() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("dsh-tool-fs-search-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn needle() {}\n").unwrap();
    std::fs::write(root.join("notes.md"), "needle\n").unwrap();
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    dsh_subprocess_local::LocalSubprocessRuntime::install(&ctx);
    let fiber = ctx.plugin(
        Arc::new(dsh_tool_fs_search::ToolFsSearchPlugin),
        arc(serde_json::json!({"sampleOverCapGlobResults":false})),
    );
    fiber.settle().await.unwrap();
    let mut names = tools
        .schemas(None)
        .into_iter()
        .map(|s| s.name)
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, vec!["glob", "grep"]);
    let call = |name: &str, arguments| {
        tools.execute(ToolExecutionInput {
            call_id: call_id(format!("{name}-call")),
            root_call_id: None,
            name: name.into(),
            arguments,
            agent: None,
            parent: None,
            signal: Arc::new(|| false),
        })
    };
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();
    let glob = call("glob", serde_json::json!({"pattern":"**/*.rs"})).await;
    assert!(!glob.is_error, "{}", text(&glob));
    assert!(text(&glob).contains(&format!("src{}a.rs", std::path::MAIN_SEPARATOR)));
    let grep = call(
        "grep",
        serde_json::json!({"pattern":"needle","include":"*.rs"}),
    )
    .await;
    assert!(!grep.is_error, "{}", text(&grep));
    assert!(text(&grep).contains("Line 1: fn needle() {}"));
    fiber.dispose().await;
    std::env::set_current_dir(original_cwd).unwrap();
    drop(ctx);
    std::fs::remove_dir_all(root).unwrap();
}
