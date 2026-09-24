use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn valid_actions() -> Vec<Value> {
    vec![
        json!({"action":"allocate"}),
        json!({"action":"prepare_copy","files":["source.rs"]}),
        json!({"action":"list"}),
        json!({"action":"inspect","target":"report.docx"}),
        json!({"action":"promote","id":"candidate","path":"report.docx","target":"report.docx","expectedSha256":null}),
        json!({"action":"write","id":"script","path":"probe.ps1","content":""}),
        json!({"action":"read","id":"script","path":"probe.ps1","offset":0,"limit":32000}),
        json!({"action":"pin","id":"candidate","pinned":false}),
        json!({"action":"release","id":"candidate"}),
    ]
}

#[test]
fn action_contract_is_object_rooted_and_requires_each_action_dependency() {
    let schema = scratch_parameters();
    dsh_tools::assert_object_json_schema(&schema).unwrap();
    for args in valid_actions() {
        assert!(
            dsh_tools::validate_json_schema_value(&schema, &args, "arguments").is_empty(),
            "{args}"
        );
        for field in [
            "action",
            "id",
            "path",
            "content",
            "target",
            "expectedSha256",
        ] {
            if args.get(field).is_none() {
                continue;
            }
            let mut missing = args.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                !dsh_tools::validate_json_schema_value(&schema, &missing, "arguments").is_empty(),
                "must require {field} in {args}"
            );
        }
    }
    for args in [
        json!({"action":"delete","id":"candidate"}),
        json!({"action":"write","id":"script","path":"probe.ps1","content":null}),
        json!({"action":"release","id":""}),
        json!({"action":"read","id":"script","path":"probe.ps1","offset":-1}),
        json!({"action":"read","id":"script","path":"probe.ps1","limit":0}),
        json!({"action":"read","id":"script","path":"probe.ps1","limit":32001}),
        json!({"action":"read","id":"script","path":"probe.ps1","limit":1.5}),
        json!({"action":"list","unknown":true}),
        json!({"action":"promote","id":"candidate","path":"report.docx","target":"report.docx","expectedSha256":"null"}),
        json!({"action":"promote","id":"candidate","path":"report.docx","target":"report.docx","expectedSha256":""}),
    ] {
        assert!(
            !dsh_tools::validate_json_schema_value(&schema, &args, "arguments").is_empty(),
            "{args}"
        );
    }
}

#[tokio::test]
async fn missing_scratch_dependencies_are_rejected_before_dispatch() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
    let effects = Arc::new(AtomicUsize::new(0));
    let dispatched = effects.clone();
    tools
        .register(
            &ctx,
            dsh_tools::ToolDefinition {
                name: "workspace_scratch".into(),
                description: "Manage scratch files".into(),
                parameters: scratch_parameters(),
                output: dsh_tools::ToolOutputDefinition {
                    schema: json!({"type":"boolean"}),
                    render: Arc::new(|_, _| Ok(vec![])),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: None,
                finalize_content: None,
                present_call: None,
                present_result: None,
                execute: Arc::new(move |_, _| {
                    dispatched.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(json!(true)) })
                }),
            },
        )
        .unwrap();
    for args in [
        json!({"action":"write","path":"probe.ps1","content":"test"}),
        json!({"action":"promote","id":"candidate","path":"report.docx","target":"report.docx"}),
        json!({"action":"release"}),
        json!({"action":"promote","id":"candidate","path":"report.docx","target":"report.docx","expectedSha256":"null"}),
    ] {
        let result = tools
            .execute(dsh_tools::ToolExecutionInput {
                call_id: dsh_llm::call_id("scratch-preflight"),
                root_call_id: None,
                name: "workspace_scratch".into(),
                arguments: args,
                agent: None,
                parent: None,
                signal: Arc::new(|| false),
            })
            .await;
        assert!(result.is_error);
        assert_eq!(
            result.error.as_ref().unwrap().info.as_ref().unwrap().code,
            "TOOL_INPUT_INVALID"
        );
        assert_eq!(effects.load(Ordering::SeqCst), 0);
    }
    for args in valid_actions() {
        let result = tools
            .execute(dsh_tools::ToolExecutionInput {
                call_id: dsh_llm::call_id("scratch-valid"),
                root_call_id: None,
                name: "workspace_scratch".into(),
                arguments: args,
                agent: None,
                parent: None,
                signal: Arc::new(|| false),
            })
            .await;
        assert!(!result.is_error, "{:?}", result.error);
    }
    assert_eq!(effects.load(Ordering::SeqCst), 9);
}
