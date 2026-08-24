use std::{path::PathBuf, sync::Arc};

use cordis::{Context, arc};
use dsh_fs::FileSystem;
use dsh_fs_local::LocalFileSystem;
use dsh_llm::{ContentBlock, call_id};
use dsh_tools::{ToolExecutionInput, ToolRuntime};

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("dsh-tool-fs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

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
async fn registers_real_read_write_edit_and_mutates_disk() {
    let root = temp_root();
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    let fs = LocalFileSystem::build(dsh_fs_local::Config {
        cwd: Some(root.to_string_lossy().into_owned()),
        diff_basis_max_bytes: None,
    })
    .unwrap();
    let erased: Arc<dyn FileSystem> = fs;
    ctx.register_service(erased);
    let fiber = ctx.plugin(
        Arc::new(dsh_tool_fs::ToolFsPlugin),
        arc(serde_json::json!({})),
    );
    fiber.settle().await.unwrap();
    let mut names = tools
        .schemas(None)
        .into_iter()
        .map(|s| s.name)
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, vec!["edit", "read", "write"]);

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
    let written = call(
        "write",
        serde_json::json!({"file_path":"a.txt","content":"alpha\nbeta"}),
    )
    .await;
    assert!(!written.is_error, "{}", text(&written));
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "alpha\nbeta"
    );
    let read = call(
        "read",
        serde_json::json!({"file_path":"a.txt","offset":2,"limit":1}),
    )
    .await;
    assert_eq!(
        text(&read),
        format!(
            "<path>{}</path>\n<type>file</type>\n<content>\n2: beta\n\n(End of file - total 2 lines)\n</content>",
            root.join("a.txt").display()
        )
    );
    let edited = call(
        "edit",
        serde_json::json!({"file_path":"a.txt","old_string":"beta","new_string":"gamma"}),
    )
    .await;
    assert!(!edited.is_error, "{}", text(&edited));
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "alpha\ngamma"
    );
    std::fs::remove_dir_all(root).unwrap();
}
