//! Code Mode `run_code` transport.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use dsh_code_runtime::{CodeBindingErrorClass, CodeBindingFunction, CodeBindingNamespace};
use dsh_llm::{ContentBlock, call_id};
use serde_json::{Value as JsonValue, json};

use crate::{
    ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition, ToolRuntime,
};

pub(crate) const TYPESCRIPT_RUN_CODE_DESCRIPTION: &str = "Execute a TypeScript program against the available tools. Takes two required arguments: `code`, the BODY of an async function (erasable syntax only; top-level `await` and `return` work), and `description`, a short summary of what the program does. Call tools as `await tools.name(args)` per the declarations in the system prompt. Only what you print or return comes back — curate it.";

pub(crate) fn create_run_code_tool(runtime: Weak<ToolRuntime>) -> Arc<ToolDefinition> {
    Arc::new(ToolDefinition {
        name: crate::RUN_CODE_NAME.to_string(),
        description: TYPESCRIPT_RUN_CODE_DESCRIPTION.to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "The program: the body of an async TypeScript function."
                },
                "description": {
                    "type": "string",
                    "description": "Clear, concise description of what this program does in active voice, 5-10 words (shown in the UI)."
                }
            },
            "required": ["code", "description"]
        }),
        output: ToolOutputDefinition {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "logs": { "type": "array", "items": { "type": "string" } },
                    "result": {}
                },
                "required": ["logs"]
            }),
            render: Arc::new(|_args, value| {
                let logs = value["logs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(JsonValue::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
                let rendered = value.get("result").map(render_value).unwrap_or_default();
                let text = [logs, rendered]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(vec![ContentBlock::Text {
                    text: if text.is_empty() {
                        "(run_code completed with no output)".to_string()
                    } else {
                        text
                    },
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args, exec| {
            let runtime = runtime.clone();
            let code = args["code"].as_str().unwrap_or_default().to_string();
            let signal = exec.signal.lock().clone();
            let agent = exec.agent.clone();
            let root_call_id = exec.root_call_id.clone();
            let parent_call_id = exec.call_id.clone();
            let parent = exec.token;
            Box::pin(async move {
                let owner = runtime
                    .upgrade()
                    .ok_or_else(|| ToolBodyError::plain("tool runtime is unavailable"))?;
                let code_runtime = owner.code_runtime().map_err(ToolBodyError::plain)?;
                let sequence = Arc::new(AtomicU64::new(0));
                let mut functions = owner
                    .schemas(agent.as_ref().map(|agent| agent.scope_key()))
                    .into_iter()
                    .filter(|schema| schema.name != crate::RUN_CODE_NAME)
                    .map(|schema| {
                        let name = schema.name;
                        let owner = Arc::clone(&owner);
                        let agent = agent.clone();
                        let root_call_id = root_call_id.clone();
                        let parent_call_id = parent_call_id.clone();
                        let signal = signal.clone();
                        let sequence = Arc::clone(&sequence);
                        let binding_name = name.clone();
                        let function: CodeBindingFunction = Arc::new(move |arguments| {
                            let owner = Arc::clone(&owner);
                            let agent = agent.clone();
                            let root_call_id = root_call_id.clone();
                            let parent_call_id = parent_call_id.clone();
                            let signal = signal.clone();
                            let name = binding_name.clone();
                            let n = sequence.fetch_add(1, Ordering::Relaxed) + 1;
                            Box::pin(async move {
                                let result = owner
                                    .execute(crate::ToolExecutionInput {
                                        call_id: call_id(format!(
                                            "{}:code:{n}",
                                            parent_call_id.as_str()
                                        )),
                                        root_call_id: Some(root_call_id),
                                        name: name.clone(),
                                        arguments,
                                        agent,
                                        parent: Some(parent),
                                        signal,
                                    })
                                    .await;
                                if result.is_error {
                                    panic!(
                                        "{}",
                                        result
                                            .error
                                            .as_ref()
                                            .map(|error| error.message.as_str())
                                            .unwrap_or("tool call failed")
                                    );
                                }
                                result.value.clone().unwrap_or(JsonValue::Null)
                            })
                        });
                        (name, function)
                    })
                    .collect::<Vec<_>>();
                functions.sort_by(|left, right| left.0.cmp(&right.0));
                let outcome = code_runtime
                    .run(dsh_code_runtime::CodeRunRequest {
                        program: code,
                        bindings: vec![CodeBindingNamespace {
                            global: "tools".to_string(),
                            functions,
                            error_class: Some(CodeBindingErrorClass {
                                name: "ToolCallError".to_string(),
                                member_name_property: "toolName".to_string(),
                            }),
                        }],
                        signal: Some(signal),
                    })
                    .await
                    .map_err(ToolBodyError::plain)?;
                if let Some(error) = outcome.error {
                    return Err(ToolBodyError::coded(
                        format!(
                            "code run failed ({}): {}{}",
                            error.kind.as_str(),
                            error.message,
                            if outcome.logs.is_empty() {
                                String::new()
                            } else {
                                format!("\nCaptured output:\n{}", outcome.logs.join("\n"))
                            }
                        ),
                        "CodeRunFailedError",
                        "CODE_RUN_FAILED",
                    ));
                }
                let mut value = serde_json::Map::new();
                value.insert("logs".to_string(), json!(outcome.logs));
                if let Some(result) = outcome.value {
                    value.insert("result".to_string(), result);
                }
                Ok(JsonValue::Object(value))
            })
        }),
        finalize_content: None,
        present_call: Some(Arc::new(|args| {
            Some(ToolCallView::Generic {
                title: args["description"].as_str().unwrap_or_default().to_string(),
                kind: Some(ToolCallKind::Execute),
                raw_input: args.get("code").cloned(),
                content: None,
                locations: None,
            })
        })),
        present_result: None,
    })
}

fn render_value(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}
