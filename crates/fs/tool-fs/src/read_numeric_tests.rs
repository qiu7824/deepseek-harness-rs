use super::*;
use serde_json::{Value, json};

#[tokio::test]
async fn recorded_integral_float_offsets_reach_the_file_reader() {
    let root = std::env::temp_dir().join(format!("dsh-read-number-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("proof.txt");
    std::fs::write(&path, (1..=500).map(|line| format!("line {line}\n")).collect::<String>()).unwrap();
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
    dsh_fs_local::LocalFileSystem::install(&ctx, dsh_fs_local::Config { cwd: Some(root.to_string_lossy().into_owned()), diff_basis_max_bytes: None }).unwrap();
    Service::install(&ctx).unwrap();
    let run = |args: Value| {
        let tools = tools.clone();
        async move {
            tools.execute(dsh_tools::ToolExecutionInput { call_id: dsh_llm::call_id(uuid::Uuid::new_v4().to_string()), root_call_id: None, name: "read".into(), arguments: args, agent: None, parent: None, signal: Arc::new(|| false) }).await
        }
    };
    // JSON numeric spellings observed in the reported session. Do not coerce
    // strings or round fractional line numbers to make these cases pass.
    for (offset, limit, first, count) in [(1.0, 70.0, 1, 70), (365.0, 120.0, 365, 120), (3.65e2, 1e1, 365, 10), (0.0, 2.0, 1, 2)] {
        let result = run(json!({"file_path":"proof.txt","offset":offset,"limit":limit})).await;
        assert!(!result.is_error, "integral {offset}/{limit} was rejected: {:?}", result.content);
        let value = result.value.as_ref().unwrap();
        assert_eq!(value["offset"], first);
        assert_eq!(value["lines"][0]["number"], first);
        assert_eq!(value["lines"][0]["text"], format!("line {first}"));
        assert_eq!(value["lines"].as_array().unwrap().len(), count);
    }
    for (offset, limit) in [(json!(1.2), json!(2)), (json!(-1), json!(2)), (json!("1"), json!(2)), (Value::Null, json!(2)), (json!(1e30), json!(2)), (json!(1), json!(0)), (json!(1), json!(2.5)), (json!(1), json!(2001))] {
        let result = run(json!({"file_path":"proof.txt","offset":offset,"limit":limit})).await;
        assert!(result.is_error, "invalid read arguments were accepted");
    }
    assert!(std::fs::read_to_string(&path).unwrap().starts_with("line 1\n"));
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&root).unwrap();
}
