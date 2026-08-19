use std::sync::Arc;

use cordis::Context;
use dsh_code_runtime::{CodeRunRequest, CodeRunResult, CodeRuntime};
use dsh_llm::{ContentBlock, call_id};
use dsh_system_prompt::{AssembleContext, SystemPrompt};
use dsh_tools::{
    Config, RUN_CODE_NAME, ToolBodyError, ToolDefinition, ToolOutputDefinition,
    ToolPresentationMode, ToolRuntime,
};
use serde_json::{Value as JsonValue, json};

struct StubCodeRuntime {
    requests: parking_lot::Mutex<Vec<CodeRunRequest>>,
    result: parking_lot::Mutex<CodeRunResult>,
}

impl StubCodeRuntime {
    fn returning(result: CodeRunResult) -> Self {
        Self {
            requests: parking_lot::Mutex::new(Vec::new()),
            result: parking_lot::Mutex::new(result),
        }
    }
}

impl CodeRuntime for StubCodeRuntime {
    fn language(&self) -> String {
        "typescript".to_string()
    }

    fn isolation(&self) -> String {
        "stub".to_string()
    }

    fn run(
        &self,
        request: CodeRunRequest,
    ) -> futures::future::BoxFuture<'static, Result<CodeRunResult, String>> {
        self.requests.lock().push(request);
        let result = self.result.lock().clone();
        Box::pin(async move { Ok(result) })
    }
}

fn echo_tool() -> ToolDefinition {
    ToolDefinition {
        name: "echo".to_string(),
        description: "Echo a value.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        }),
        output: ToolOutputDefinition {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_args, value| {
                Ok(vec![ContentBlock::Text {
                    text: value.as_str().unwrap_or_default().to_string(),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(|args, _exec| {
            let value = args["value"].clone();
            Box::pin(async move { Ok::<JsonValue, ToolBodyError>(value) })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

async fn assembled_tools(mode: ToolPresentationMode) -> Vec<dsh_llm::ToolSchema> {
    let ctx = Context::root();
    let prompt =
        SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("systemPrompt");
    let code_runtime: Arc<dyn CodeRuntime> =
        Arc::new(StubCodeRuntime::returning(CodeRunResult::default()));
    ctx.register_service(code_runtime);
    let tools = ToolRuntime::install(
        &ctx,
        Config {
            mode: Some(mode),
            max_parallel_sub_calls: None,
        },
    )
    .expect("tools");
    tools.register(&ctx, echo_tool()).expect("echo");
    prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .expect("assemble")
        .tools
}

#[tokio::test]
async fn code_mode_wire_schema_is_only_typescript_run_code() {
    let schemas = assembled_tools(ToolPresentationMode::Code).await;
    assert_eq!(schemas.len(), 1);
    let schema = &schemas[0];
    assert_eq!(schema.name, RUN_CODE_NAME);
    assert!(schema.description.contains("Execute a TypeScript program"));
    assert_eq!(schema.parameters["type"], "object");
    assert_eq!(
        schema.parameters["required"],
        json!(["code", "description"])
    );
    assert_eq!(schema.parameters["properties"]["code"]["type"], "string");
    assert!(
        schema.parameters["properties"]["code"]["description"]
            .as_str()
            .is_some_and(|text| text.contains("TypeScript"))
    );
    assert_eq!(
        schema.parameters["properties"]["description"]["type"],
        "string"
    );
}

#[tokio::test]
async fn both_mode_wire_schema_is_native_plus_run_code() {
    let schemas = assembled_tools(ToolPresentationMode::Both).await;
    let names = schemas
        .iter()
        .map(|schema| schema.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["echo", RUN_CODE_NAME]);
}

fn code_input(code: &str, description: &str) -> dsh_tools::ToolExecutionInput {
    dsh_tools::ToolExecutionInput {
        call_id: call_id("call-1"),
        root_call_id: None,
        name: RUN_CODE_NAME.to_string(),
        arguments: json!({ "code": code, "description": description }),
        agent: None,
        parent: None,
        signal: Arc::new(|| false),
    }
}

struct BindingStubRuntime;

impl CodeRuntime for BindingStubRuntime {
    fn language(&self) -> String {
        "typescript".to_string()
    }

    fn isolation(&self) -> String {
        "binding-stub".to_string()
    }

    fn run(
        &self,
        request: CodeRunRequest,
    ) -> futures::future::BoxFuture<'static, Result<CodeRunResult, String>> {
        Box::pin(async move {
            assert_eq!(request.bindings.len(), 1);
            let namespace = &request.bindings[0];
            assert_eq!(namespace.global, "tools");
            let error_class = namespace.error_class.as_ref().expect("typed errors");
            assert_eq!(error_class.name, "ToolCallError");
            assert_eq!(error_class.member_name_property, "toolName");
            assert_eq!(
                namespace
                    .functions
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
                vec!["echo"]
            );
            let echo = namespace.functions[0].1.clone();
            let value = echo(json!({ "value": "nested" })).await;
            Ok(CodeRunResult {
                logs: Vec::new(),
                value: Some(value),
                error: None,
            })
        })
    }
}

#[tokio::test]
async fn run_code_calls_code_runtime_with_program_and_returns_logs_and_value() {
    let ctx = Context::root();
    let _prompt =
        SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("systemPrompt");
    let stub = Arc::new(StubCodeRuntime::returning(CodeRunResult {
        logs: vec!["printed".to_string()],
        value: Some(json!({ "answer": 42 })),
        error: None,
    }));
    let service: Arc<dyn CodeRuntime> = stub.clone();
    ctx.register_service(service);
    let tools = ToolRuntime::install(
        &ctx,
        Config {
            mode: Some(ToolPresentationMode::Code),
            max_parallel_sub_calls: None,
        },
    )
    .expect("tools");

    let result = tools
        .execute(code_input(
            "return { answer: 42 }",
            "Return the computed answer",
        ))
        .await;

    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(
        result.value,
        Some(json!({ "logs": ["printed"], "result": { "answer": 42 } }))
    );
    let requests = stub.requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].program, "return { answer: 42 }");
    assert!(requests[0].signal.is_some());
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "printed\n{\n  \"answer\": 42\n}".to_string()
        }]
    );
}

#[tokio::test]
async fn tools_binding_dispatches_native_tool_as_a_nested_call() {
    let ctx = Context::root();
    let _prompt =
        SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("systemPrompt");
    let service: Arc<dyn CodeRuntime> = Arc::new(BindingStubRuntime);
    ctx.register_service(service);
    let tools = ToolRuntime::install(
        &ctx,
        Config {
            mode: Some(ToolPresentationMode::Code),
            max_parallel_sub_calls: None,
        },
    )
    .expect("tools");
    tools.register(&ctx, echo_tool()).expect("echo");

    let result = tools
        .execute(code_input(
            "return await tools.echo({ value: 'nested' })",
            "Echo nested value",
        ))
        .await;

    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(
        result.value,
        Some(json!({ "logs": [], "result": "nested" }))
    );
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "nested".to_string()
        }]
    );
}
