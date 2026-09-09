use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn input(arguments: JsonValue) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: dsh_llm::call_id("schema-check"),
        root_call_id: None,
        name: "bounded-operation".into(),
        arguments,
        agent: None,
        parent: None,
        signal: Arc::new(|| false),
    }
}

#[tokio::test]
async fn current_schema_blocks_invalid_input_before_body_and_reports_a_coded_failure() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Config::default()).unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let tool_runs = runs.clone();
    tools.register(&ctx,ToolDefinition{
        name:"bounded-operation".into(),description:"Read a bounded number of entries".into(),
        parameters:serde_json::json!({"type":"object","required":["limit"],"properties":{"limit":{"type":"integer","enum":[1,2,3,4,5]}},"additionalProperties":false}),
        output:ToolOutputDefinition{schema:serde_json::json!({"type":"boolean"}),render:Arc::new(|_,_|Ok(vec![])),presentation_meta:None},
        timeout_ms:None,is_concurrency_safe:None,execute:Arc::new(move|_,_|{tool_runs.fetch_add(1,Ordering::SeqCst);Box::pin(async{Ok(serde_json::json!(true))})}),
        finalize_content:None,present_call:None,present_result:None,
    }).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = seen.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_, args| {
        let observed = observed.clone();
        let result = downcast_arc::<Arc<ToolExecutionResult>>(&args[1])
            .unwrap()
            .as_ref()
            .clone();
        Box::pin(async move {
            observed.lock().push(
                result
                    .error
                    .as_ref()
                    .and_then(|error| error.info.as_ref())
                    .map(|info| info.code.clone()),
            );
            None
        })
    });
    ctx.on(
        "tools/result",
        listener,
        cordis::EventOptions::default().global(true),
    )
    .await;
    let invalid = tools.execute(input(serde_json::json!({"limit":20}))).await;
    assert!(invalid.is_error);
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert_eq!(
        invalid.error.as_ref().unwrap().info.as_ref().unwrap().code,
        "TOOL_INPUT_INVALID"
    );
    assert_eq!(seen.lock()[0].as_deref(), Some("TOOL_INPUT_INVALID"));
    let corrected = tools.execute(input(serde_json::json!({"limit":3}))).await;
    assert!(!corrected.is_error);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn finite_negative_scroll_deltas_reach_the_body_while_invalid_numbers_do_not() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Config::default()).unwrap();
    let deltas = Arc::new(Mutex::new(Vec::new()));
    let received = deltas.clone();
    tools.register(&ctx, ToolDefinition {
        name: "bounded-operation".into(), description: "Accept a signed scroll delta".into(),
        parameters: serde_json::json!({"type":"object","required":["deltaY"],"properties":{"deltaY":{"type":"number"}},"additionalProperties":false}),
        output: ToolOutputDefinition { schema: serde_json::json!({"type":"number"}), render: Arc::new(|_,_|Ok(vec![])), presentation_meta: None },
        timeout_ms: None, is_concurrency_safe: None,
        execute: Arc::new(move |args,_| {
            let delta = args["deltaY"].clone(); received.lock().push(delta.clone());
            Box::pin(async move { Ok(delta) })
        }),
        finalize_content: None, present_call: None, present_result: None,
    }).unwrap();
    for delta in [
        serde_json::json!(-600),
        serde_json::json!(-120),
        serde_json::json!(-0.25),
        serde_json::json!(120),
    ] {
        let result = tools
            .execute(input(serde_json::json!({"deltaY":delta})))
            .await;
        assert!(
            !result.is_error,
            "signed input/output was rejected: {:?}",
            result.error
        );
        assert_eq!(result.value.as_ref(), Some(&delta));
    }
    let accepted = deltas.lock().len();
    assert_eq!(accepted, 4);
    for delta in [
        serde_json::json!(-0.0),
        serde_json::json!("-120"),
        serde_json::json!(null),
        serde_json::json!(true),
        serde_json::json!(f64::NAN),
        serde_json::json!(f64::INFINITY),
    ] {
        let result = tools
            .execute(input(serde_json::json!({"deltaY":delta})))
            .await;
        assert!(result.is_error);
        assert_eq!(
            result.error.as_ref().unwrap().info.as_ref().unwrap().code,
            "TOOL_INPUT_INVALID"
        );
        assert_eq!(
            deltas.lock().len(),
            accepted,
            "invalid value reached the tool body"
        );
    }
}
