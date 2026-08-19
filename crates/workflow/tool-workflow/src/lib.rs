//! Model-facing workflow tool and durable parent-session projection.

use std::sync::Arc;

use cordis::{Context, Disposer};
use dsh_llm::ContentBlock;
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use dsh_workflow::{WorkflowEngine, WorkflowMeta, WorkflowStartRequest, WorkflowStopReason};
use serde_json::{Value, json};

pub const NAME: &str = "workflow";

pub fn apply(ctx: &Context) -> Result<Disposer, String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-workflow requires the tools service".to_string())?;
    let engine = ctx
        .get_typed::<Arc<dyn WorkflowEngine>>("workflowEngine", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-workflow requires the workflowEngine service".to_string())?;

    tools.register(
        ctx,
        ToolDefinition {
            name: NAME.to_string(),
            description: "Run a JavaScript orchestration workflow that can start fresh subagents and return one curated JSON result.".to_string(),
            parameters: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "The body of an async JavaScript workflow function."
                    },
                    "meta": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "whenToUse": { "type": "string" },
                            "phases": { "type": "array" }
                        },
                        "required": ["name", "description"]
                    },
                    "args": {}
                },
                "required": ["script", "meta"]
            }),
            output: ToolOutputDefinition {
                schema: json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "runId": { "type": "string" },
                        "agentsStarted": { "type": "integer" },
                        "result": {}
                    },
                    "required": ["runId", "agentsStarted", "result"]
                }),
                render: Arc::new(|_args, value| {
                    Ok(vec![ContentBlock::Text {
                        text: serde_json::to_string_pretty(value)
                            .unwrap_or_else(|_| "workflow completed".to_string()),
                    }])
                }),
                presentation_meta: None,
            },
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args, run| {
                let engine = engine.clone();
                let args = args.clone();
                let parent = run.execution.agent.clone();
                let signal = run.execution.signal.lock().clone();
                Box::pin(async move {
                    let parent = parent.ok_or_else(|| {
                        ToolBodyError::plain("workflow requires a live parent agent")
                    })?;
                    let script = args
                        .get("script")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ToolBodyError::plain("workflow.script must be a string"))?
                        .to_string();
                    let meta: WorkflowMeta = serde_json::from_value(
                        args.get("meta").cloned().unwrap_or(Value::Null),
                    )
                    .map_err(|error| ToolBodyError::plain(format!("invalid workflow meta: {error}")))?;
                    let workflow = engine
                        .start(WorkflowStartRequest {
                            script,
                            meta: meta.clone(),
                            args: args.get("args").cloned(),
                            subagent_provider: None,
                            max_total_agents: None,
                            parent: parent.clone(),
                            signal: Some(signal),
                        })
                        .map_err(|error| {
                            ToolBodyError::coded(
                                error.to_string(),
                                "WorkflowError",
                                error.code(),
                            )
                        })?;
                    if let Err(error) = parent.session().append(
                        "tool-workflow/run-start",
                        json!({
                            "runId": workflow.id().as_str(),
                            "meta": meta,
                        }),
                        None,
                    ) {
                        workflow.dispose().await;
                        return Err(ToolBodyError::plain(format!(
                            "workflow run-start persistence failed: {error}"
                        )));
                    }
                    let result = workflow.result().await;
                    let run_id = workflow.id().as_str().to_string();
                    let append = parent.session().append(
                        "tool-workflow/run-end",
                        json!({
                            "runId": run_id,
                            "stopReason": result.stop_reason.as_str(),
                            "error": result.error,
                            "agentsStarted": result.agents_started,
                        }),
                        None,
                    );
                    workflow.dispose().await;
                    append.map_err(|error| {
                        ToolBodyError::plain(format!(
                            "workflow run-end persistence failed: {error}"
                        ))
                    })?;
                    match result.stop_reason {
                        WorkflowStopReason::Completed => Ok(json!({
                            "runId": run_id,
                            "agentsStarted": result.agents_started,
                            "result": result.value,
                        })),
                        WorkflowStopReason::Cancelled | WorkflowStopReason::Error => {
                            Err(ToolBodyError::coded(
                                result.error.unwrap_or_else(|| {
                                    format!("workflow {}", result.stop_reason.as_str())
                                }),
                                "WorkflowRunError",
                                if result.stop_reason == WorkflowStopReason::Cancelled {
                                    "WORKFLOW_CANCELLED"
                                } else {
                                    "WORKFLOW_FAILED"
                                },
                            ))
                        }
                    }
                })
            }),
            finalize_content: None,
            present_call: None,
            present_result: None,
        },
    )
}
