//! Model-facing read-only LSP tool over the provider registry.

use std::sync::Arc;

use cordis::Context;
use dsh_llm::ContentBlock;
use dsh_lsp::{Lsp, LspOperation, LspPosition, LspQueryRequest};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use serde_json::{Value, json};

/// Register the model-facing `lsp` tool over one provider registry.
pub fn apply(ctx: &Context, lsp: Arc<Lsp>) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-lsp requires the tools service".to_string())?;
    let definition = ToolDefinition {
        name: "lsp".to_string(),
        description: "Query a language server for precise definitions, references, implementations, or hover information. line and character are one-based UTF-16 coordinates.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["goToDefinition", "findReferences", "goToImplementation", "hover"]
                },
                "file_path": { "type": "string" },
                "line": { "type": "integer" },
                "character": { "type": "integer" }
            },
            "required": ["operation", "file_path", "line", "character"],
            "additionalProperties": false
        }),
        output: ToolOutputDefinition {
            schema: json!({
                "oneOf": [
                    {
                        "type": "object",
                        "properties": {
                            "kind": { "type": "string", "const": "locations" },
                            "locations": { "type": "array", "items": {} },
                            "resolvedWorkspaceUri": { "type": "string" }
                        },
                        "required": ["kind", "locations", "resolvedWorkspaceUri"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "kind": { "type": "string", "const": "hover" },
                            "hover": {}
                        },
                        "required": ["kind", "hover"],
                        "additionalProperties": false
                    }
                ]
            }),
            render: Arc::new(|_args, value| {
                Ok(vec![ContentBlock::Text {
                    text: serde_json::to_string_pretty(value).map_err(|error| error.to_string())?,
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: Some(60_000),
        is_concurrency_safe: Some(Arc::new(|_args| true)),
        execute: Arc::new(move |args, run| {
            let lsp = lsp.clone();
            let args = args.clone();
            let agent = run.agent.clone();
            let signal = run.execution.signal.lock().clone();
            Box::pin(async move {
                let workspace_root = agent
                    .as_ref()
                    .and_then(|agent| agent.session().header().cwd.clone())
                    .ok_or_else(|| {
                        ToolBodyError::plain("the lsp tool requires a session workspace cwd")
                    })?;
                let operation = match required_string(&args, "operation")? {
                    "goToDefinition" => LspOperation::GoToDefinition,
                    "findReferences" => LspOperation::FindReferences,
                    "goToImplementation" => LspOperation::GoToImplementation,
                    "hover" => LspOperation::Hover,
                    other => {
                        return Err(ToolBodyError::plain(format!(
                            "unsupported lsp operation {other:?}"
                        )));
                    }
                };
                let file_path = required_string(&args, "file_path")?.to_string();
                let line = one_based(&args, "line")?;
                let character = one_based(&args, "character")?;
                let cancellation = dsh_lsp::LspCancellation::from_predicate(signal);
                let result = lsp
                    .query_with_signal(LspQueryRequest {
                        operation,
                        file_path,
                        position: LspPosition {
                            line: line - 1,
                            character: character - 1,
                        },
                        workspace_root,
                    }, Some(cancellation))
                    .await
                    .map_err(|failure| ToolBodyError::plain(failure.to_string()))?;
                serde_json::to_value(result)
                    .map_err(|failure| ToolBodyError::plain(failure.to_string()))
            })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    };
    tools.register(ctx, definition).map(|_| ())
}

fn required_string<'a>(args: &'a Value, name: &str) -> Result<&'a str, ToolBodyError> {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolBodyError::plain(format!("lsp {name} must be a non-empty string")))
}

fn one_based(args: &Value, name: &str) -> Result<u64, ToolBodyError> {
    args.get(name)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| ToolBodyError::plain(format!("lsp {name} must be a positive integer")))
}
