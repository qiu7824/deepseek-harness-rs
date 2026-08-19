use std::collections::HashMap;
use std::sync::Arc;

use cordis::{Context, EventOptions, downcast_arc};
use dsh_llm::ContentBlock;
use dsh_system_prompt::{PromptSection, PromptText, SystemPrompt};
use dsh_tools::{
    ToolBodyError, ToolDefinition, ToolExecution, ToolExecutionResult, ToolOutputDefinition,
    ToolRuntime,
};
use parking_lot::Mutex;
use serde_json::{Value, json};

pub const STRUCTURED_OUTPUT_TOOL: &str = "structured_output";
pub const STRUCTURED_OUTPUT_INSTRUCTION: &str = "When you have your final answer, you MUST report it by calling the `structured_output` tool with arguments matching its parameter schema exactly. Do not finish with a plain text answer: only the tool call counts as your result.";

#[derive(Clone)]
pub struct StructuredAttachment {
    captured: Arc<Mutex<Option<Value>>>,
}

impl StructuredAttachment {
    pub fn captured(&self) -> Option<Value> {
        self.captured.lock().clone()
    }
}

pub async fn attach_structured_runtime(
    child_ctx: &Context,
    schema: Value,
) -> Result<StructuredAttachment, String> {
    let tools = child_ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "structured output requires the tools service".to_string())?;
    let prompt = child_ctx
        .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "structured output requires the systemPrompt service".to_string())?;
    let staged = Arc::new(Mutex::new(HashMap::<u64, Value>::new()));
    let captured = Arc::new(Mutex::new(None::<Value>));

    let staged_body = staged.clone();
    tools.register(
        child_ctx,
        ToolDefinition {
            name: STRUCTURED_OUTPUT_TOOL.to_string(),
            description: "Report the final structured result exactly once.".to_string(),
            parameters: schema,
            output: ToolOutputDefinition {
                schema: json!({
                    "type": "object",
                    "properties": { "recorded": { "type": "boolean", "const": true } },
                    "required": ["recorded"],
                    "additionalProperties": false
                }),
                render: Arc::new(|_, _| {
                    Ok(vec![ContentBlock::Text {
                        text: "Structured output recorded.".to_string(),
                    }])
                }),
                presentation_meta: None,
            },
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args, run| {
                let value = args.clone();
                let token = run.execution.token;
                let staged = staged_body.clone();
                run.conclude_turn();
                Box::pin(async move {
                    staged.lock().insert(token, value);
                    Ok::<Value, ToolBodyError>(json!({ "recorded": true }))
                })
            }),
            finalize_content: None,
            present_call: None,
            present_result: None,
        },
    )?;

    prompt.section(
        child_ctx,
        PromptSection {
            name: format!("tool:{STRUCTURED_OUTPUT_TOOL}"),
            order: 190.0,
            text: PromptText::Static(STRUCTURED_OUTPUT_INSTRUCTION.to_string()),
            complete: None,
        },
    );

    let guard_staged = staged.clone();
    let guard_captured = captured.clone();
    tools.guard(
        child_ctx,
        Arc::new(move |_execution| {
            if guard_captured.lock().is_some() || !guard_staged.lock().is_empty() {
                Some("structured output already recorded: the run is complete".to_string())
            } else {
                None
            }
        }),
    )?;

    let listener_staged = staged;
    let listener_captured = captured.clone();
    child_ctx
        .on(
            "tools/result",
            Arc::new(move |_ctx, args| {
                let staged = listener_staged.clone();
                let captured = listener_captured.clone();
                Box::pin(async move {
                    let execution = downcast_arc::<Arc<ToolExecution>>(&args[0]);
                    let result = downcast_arc::<Arc<ToolExecutionResult>>(&args[1]);
                    let (Some(execution), Some(result)) = (execution, result) else {
                        return None;
                    };
                    if execution.name != STRUCTURED_OUTPUT_TOOL {
                        return None;
                    }
                    let value = staged.lock().remove(&execution.token);
                    if !result.is_error
                        && execution.parent.is_none()
                        && captured.lock().is_none()
                        && let Some(value) = value
                    {
                        *captured.lock() = Some(value);
                    }
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;

    Ok(StructuredAttachment { captured })
}
