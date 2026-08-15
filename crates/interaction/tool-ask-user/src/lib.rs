//! Model-facing consumer of the `ctx.userQuestions` capability seam. The
//! tool pauses until a UI provider returns a human answer, then feeds that
//! answer back into the agent loop as an ordinary tool result.
//! Rust port of `packages/interaction/tool-ask-user/src/index.ts`.
//!
//! # Deviations
//!
//! - Input arguments are validated in the body with the shared JSON Schema
//!   engine (the Rust tool runtime does not validate before dispatch).
//! - The abort seam is a predicate; aborts surface through the
//!   user-questions service.

use std::sync::Arc;

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_tools::{
    ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRunContext, ToolRuntime,
    validate_json_schema_value,
};
use dsh_user_questions::{
    AskUserQuestionItem, AskUserQuestionOption, AskUserQuestionRequest, UserQuestionService,
};

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "tool-ask-user";

/// Services required by the tool.
pub const INJECT: [&str; 2] = ["tools", "userQuestions"];

const DESCRIPTION: &str = "Ask the user a concise question when you need confirmation, a choice, or missing information before proceeding. Send one or more questions, each with a stable id that will be echoed in the answer.";

fn parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": true,
                    "properties": {
                        "id": { "type": "string" },
                        "question": { "type": "string" },
                        "header": { "type": "string" },
                        "options": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": true,
                                "properties": {
                                    "label": { "type": "string" },
                                    "description": { "type": "string" }
                                },
                                "required": ["label"]
                            }
                        },
                        "multi_select": { "type": "boolean" }
                    },
                    "required": ["id", "question"]
                }
            }
        },
        "required": ["questions"]
    })
}

fn output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "answers": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "id": { "type": "string" },
                        "selected": { "type": "array", "items": { "type": "string" } },
                        "custom": { "type": "string" }
                    },
                    "required": ["id", "selected"]
                }
            }
        },
        "required": ["answers"]
    })
}

/// Register the `ask_user_question` tool on `ctx.tools` (TS `apply`).
pub fn apply(ctx: &Context) -> Result<cordis::Disposer, String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-ask-user requires the tools service".to_string())?;
    let questions = ctx
        .get_typed::<Arc<UserQuestionService>>("userQuestions", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-ask-user requires the userQuestions service".to_string())?;
    let definition = ToolDefinition {
        name: "ask_user_question".to_string(),
        description: DESCRIPTION.to_string(),
        parameters: parameters_schema(),
        output: ToolOutputDefinition {
            schema: output_schema(),
            render: Arc::new(|_args, value| {
                Ok(vec![dsh_llm::ContentBlock::Text {
                    text: serde_json::to_string(value).expect("answer"),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args: &serde_json::Value, exec: &ToolRunContext| {
            let questions = questions.clone();
            let args = args.clone();
            let agent = exec.agent.clone();
            let signal = exec.signal.lock().clone();
            Box::pin(async move {
                let violations =
                    validate_json_schema_value(&parameters_schema(), &args, "arguments");
                if !violations.is_empty() {
                    return Err(ToolBodyError::plain(violations.join("; ")));
                }
                let raw_questions = args["questions"].as_array().cloned().unwrap_or_default();
                let mut items: Vec<AskUserQuestionItem> = Vec::new();
                for question in &raw_questions {
                    items.push(AskUserQuestionItem {
                        id: question["id"].as_str().unwrap_or_default().to_string(),
                        question: question["question"].as_str().unwrap_or_default().to_string(),
                        detail: None,
                        header: question
                            .get("header")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                        options: question
                            .get("options")
                            .and_then(|value| value.as_array())
                            .map(|options| {
                                options
                                    .iter()
                                    .map(|option| AskUserQuestionOption {
                                        label: option["label"].as_str().unwrap_or_default().to_string(),
                                        description: option
                                            .get("description")
                                            .and_then(|value| value.as_str())
                                            .map(str::to_string),
                                    })
                                    .collect()
                            }),
                        multi_select: question
                            .get("multi_select")
                            .and_then(|value| value.as_bool()),
                        intent: None,
                    });
                }
                let request = AskUserQuestionRequest {
                    questions: items,
                    agent,
                    signal: Some(signal),
                };
                let answer = questions
                    .ask(&request)
                    .await
                    .map_err(|error| ToolBodyError::coded(error.message, "UserQuestionError", &error.code))?;
                Ok(serde_json::json!({
                    "answers": answer.answers.iter().map(|item| {
                        let mut value = serde_json::json!({
                            "id": item.id,
                            "selected": item.selected,
                        });
                        if let Some(custom) = &item.custom {
                            value["custom"] = serde_json::json!(custom);
                        }
                        value
                    }).collect::<Vec<_>>()
                }))
            })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    };
    tools.register(ctx, definition)
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `apply`).
pub struct ToolAskUserPlugin;

#[async_trait::async_trait]
impl Plugin for ToolAskUserPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let disposer =
            apply(ctx).map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        let _ = ctx.effect(
            "tool-ask-user",
            Box::pin(async move { Some(disposer) }),
        );
        Ok(())
    }
}
