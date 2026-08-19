use dsh_llm::{ContentBlock, GenerateOptions, LlmFailure, Role, content_has_image};
use serde_json::{Map, Value, json};

use crate::{DeepSeekReasoningEffort, RequestDefaults, ThinkingMode};

fn failure(message: impl Into<String>, code: &str) -> LlmFailure {
    LlmFailure {
        message: message.into(),
        code: code.to_string(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

fn flatten_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn serialize_messages(options: &GenerateOptions) -> Result<Vec<Value>, LlmFailure> {
    let mut messages = Vec::new();
    if let Some(system) = &options.system {
        messages.push(json!({"role": "system", "content": system}));
    }

    for message in &options.messages {
        if content_has_image(&message.content) {
            return Err(failure(
                "The DeepSeek chat-completions adapter does not support image content.",
                "UNSUPPORTED_CONTENT",
            ));
        }
        match message.role {
            Role::System => messages.push(json!({
                "role": "system",
                "content": flatten_text(&message.content),
            })),
            Role::Assistant => {
                let text = flatten_text(&message.content);
                let reasoning: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Reasoning { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                let tool_calls: Vec<Value> = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } => Some(json!({
                            "id": id.as_str(),
                            "type": "function",
                            "function": {"name": name, "arguments": arguments},
                        })),
                        _ => None,
                    })
                    .collect();
                let mut wire = Map::new();
                wire.insert("role".to_string(), json!("assistant"));
                wire.insert("content".to_string(), json!(text));
                if !tool_calls.is_empty() {
                    if !reasoning.is_empty() {
                        wire.insert("reasoning_content".to_string(), json!(reasoning));
                    }
                    wire.insert("tool_calls".to_string(), Value::Array(tool_calls));
                }
                messages.push(Value::Object(wire));
            }
            Role::User => {
                let tool_results: Vec<_> = message
                    .content
                    .iter()
                    .filter_map(ContentBlock::as_tool_result)
                    .collect();
                let text = flatten_text(&message.content);
                if !text.is_empty() || tool_results.is_empty() {
                    messages.push(json!({"role": "user", "content": text}));
                }
                for (id, content, _) in tool_results {
                    let output = flatten_text(content);
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": id.as_str(),
                        "content": if output.is_empty() { "(no output)" } else { output.as_str() },
                    }));
                }
            }
        }
    }
    Ok(messages)
}

fn resolve_thinking(
    options: &GenerateOptions,
    defaults: &RequestDefaults,
) -> Result<(Option<ThinkingMode>, Option<&'static str>), LlmFailure> {
    if options.purpose.as_deref() == Some("session-title") {
        return Ok((Some(ThinkingMode::Disabled), None));
    }
    let effort = if let Some(effort) = options.reasoning_effort.as_ref() {
        match effort.as_str() {
            "off" => Some(DeepSeekReasoningEffort::Off),
            "high" => Some(DeepSeekReasoningEffort::High),
            "max" => Some(DeepSeekReasoningEffort::Max),
            other => {
                return Err(failure(
                    format!("DeepSeek does not support reasoning effort \"{other}\""),
                    "UNSUPPORTED_REASONING_EFFORT",
                ));
            }
        }
    } else {
        defaults.reasoning_effort
    };
    if defaults.thinking == Some(ThinkingMode::Disabled)
        && matches!(
            effort,
            Some(DeepSeekReasoningEffort::High | DeepSeekReasoningEffort::Max)
        )
    {
        return Err(failure(
            "DeepSeek deployment does not support the requested reasoning effort",
            "UNSUPPORTED_REASONING_EFFORT",
        ));
    }
    Ok(match effort {
        Some(DeepSeekReasoningEffort::Off) => (Some(ThinkingMode::Disabled), None),
        Some(DeepSeekReasoningEffort::High) => (Some(ThinkingMode::Enabled), Some("high")),
        Some(DeepSeekReasoningEffort::Max) => (Some(ThinkingMode::Enabled), Some("max")),
        None => (defaults.thinking, None),
    })
}

pub(crate) fn serialize_request(
    options: &GenerateOptions,
    defaults: &RequestDefaults,
) -> Result<Value, LlmFailure> {
    let mut body = Map::new();
    body.insert("model".to_string(), json!(options.model));
    body.insert(
        "messages".to_string(),
        Value::Array(serialize_messages(options)?),
    );
    body.insert("stream".to_string(), json!(true));
    body.insert("stream_options".to_string(), json!({"include_usage": true}));

    let (thinking, reasoning_effort) = resolve_thinking(options, defaults)?;
    if let Some(thinking) = thinking {
        body.insert(
            "thinking".to_string(),
            json!({
                "type": match thinking {
                    ThinkingMode::Enabled => "enabled",
                    ThinkingMode::Disabled => "disabled",
                }
            }),
        );
    }
    if let Some(effort) = reasoning_effort {
        body.insert("reasoning_effort".to_string(), json!(effort));
    }
    if let Some(tools) = &options.tools
        && !tools.is_empty()
    {
        body.insert(
            "tools".to_string(),
            Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.parameters,
                            },
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(temperature) = options.temperature {
        body.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_tokens) = options.max_tokens {
        body.insert("max_tokens".to_string(), json!(max_tokens));
    }
    if let Some(stop) = &options.stop {
        body.insert("stop".to_string(), json!(stop));
    }
    Ok(Value::Object(body))
}
