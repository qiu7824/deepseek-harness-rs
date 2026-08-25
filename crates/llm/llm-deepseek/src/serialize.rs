use std::collections::HashMap;

use dsh_llm::{ContentBlock, GenerateOptions, LlmFailure, Role, content_has_image};
use serde_json::{Map, Value, json};

#[cfg(test)]
#[path = "serialize_tests.rs"]
mod tests;

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

const TOOL_RESULT_IMAGE_TEXT: &str =
    "The following image(s) are from the preceding tool result(s).";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedImageMeta {
    pub attachment_id: String,
    pub bytes: u64,
    pub width: u64,
    pub height: u64,
}

fn request_image_handle_text(meta: &PreparedImageMeta) -> String {
    format!(
        "Image {}; request image {}x{}px.",
        meta.attachment_id, meta.width, meta.height
    )
}

fn content_parts(
    blocks: &[ContentBlock],
    image_urls: Option<&HashMap<String, String>>,
    image_file_ids: Option<&HashMap<String, String>>,
    image_meta: Option<&HashMap<String, PreparedImageMeta>>,
) -> Result<Vec<Value>, LlmFailure> {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } if !text.is_empty() => {
                parts.push(json!({"type": "text", "text": text}));
            }
            ContentBlock::Image { attachment } => {
                if let Some(meta) =
                    image_meta.and_then(|items| items.get(&attachment.attachment_id))
                {
                    parts.push(json!({"type": "text", "text": request_image_handle_text(meta)}));
                }
                if let Some(file_id) =
                    image_file_ids.and_then(|ids| ids.get(&attachment.attachment_id))
                {
                    parts.push(json!({"type": "file", "file_id": file_id}));
                    continue;
                }
                let Some(url) = image_urls.and_then(|urls| urls.get(&attachment.attachment_id))
                else {
                    return Err(failure(
                        format!(
                            "DeepSeek image attachment {:?} was not resolved",
                            attachment.attachment_id
                        ),
                        "ATTACHMENT_NOT_FOUND",
                    ));
                };
                parts.push(json!({
                    "type": "image_url",
                    "image_url": { "url": url },
                }));
            }
            ContentBlock::ToolResult { content, .. } => {
                parts.extend(content_parts(
                    content,
                    image_urls,
                    image_file_ids,
                    image_meta,
                )?);
            }
            _ => {}
        }
    }
    Ok(parts)
}

fn flush_tool_images(messages: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    let mut content = vec![json!({"type": "text", "text": TOOL_RESULT_IMAGE_TEXT})];
    content.append(pending);
    messages.push(json!({"role": "user", "content": content}));
}

fn serialize_messages(
    options: &GenerateOptions,
    image_urls: Option<&HashMap<String, String>>,
    image_file_ids: Option<&HashMap<String, String>>,
    image_meta: Option<&HashMap<String, PreparedImageMeta>>,
) -> Result<Vec<Value>, LlmFailure> {
    let mut messages = Vec::new();
    let mut pending_tool_images = Vec::new();
    if let Some(system) = &options.system {
        messages.push(json!({"role": "system", "content": system}));
    }

    for message in &options.messages {
        if message.role != Role::User && content_has_image(&message.content) {
            return Err(failure(
                format!(
                    "The DeepSeek chat-completions adapter cannot represent image content in a {} message.",
                    message.role.as_str()
                ),
                "UNSUPPORTED_CONTENT",
            ));
        }
        match message.role {
            Role::System => {
                flush_tool_images(&mut messages, &mut pending_tool_images);
                messages.push(json!({
                    "role": "system",
                    "content": flatten_text(&message.content),
                }));
            }
            Role::Assistant => {
                flush_tool_images(&mut messages, &mut pending_tool_images);
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
                if !reasoning.is_empty() {
                    wire.insert("reasoning_content".to_string(), json!(reasoning));
                }
                if !tool_calls.is_empty() {
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
                let regular: Vec<_> = message
                    .content
                    .iter()
                    .filter(|block| !matches!(block, ContentBlock::ToolResult { .. }))
                    .cloned()
                    .collect();
                let parts = content_parts(&regular, image_urls, image_file_ids, image_meta)?;
                if !parts.is_empty() || tool_results.is_empty() {
                    flush_tool_images(&mut messages, &mut pending_tool_images);
                    let has_image = parts
                        .iter()
                        .any(|part| part["type"] == "image_url" || part["type"] == "file");
                    let content = if has_image {
                        Value::Array(parts)
                    } else {
                        Value::String(
                            parts
                                .iter()
                                .filter_map(|part| part.get("text").and_then(Value::as_str))
                                .collect(),
                        )
                    };
                    messages.push(json!({"role": "user", "content": content}));
                }
                for (id, content, _) in tool_results {
                    let result_parts =
                        content_parts(content, image_urls, image_file_ids, image_meta)?;
                    let output: String = result_parts
                        .iter()
                        .filter(|part| part["type"] == "text")
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect();
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": id.as_str(),
                        "content": if output.is_empty() { "(no output)" } else { output.as_str() },
                    }));
                    pending_tool_images.extend(
                        result_parts
                            .into_iter()
                            .filter(|part| part["type"] == "image_url" || part["type"] == "file"),
                    );
                }
            }
        }
    }
    flush_tool_images(&mut messages, &mut pending_tool_images);
    Ok(messages)
}

fn resolve_thinking(
    options: &GenerateOptions,
    defaults: &RequestDefaults,
    reasoning_wire_format: crate::ReasoningWireFormat,
) -> Result<(Option<ThinkingMode>, Option<String>), LlmFailure> {
    if options.purpose.as_deref() == Some("session-title") {
        return Ok((Some(ThinkingMode::Disabled), None));
    }
    let effort = if let Some(effort) = options.reasoning_effort.as_ref() {
        if reasoning_wire_format == crate::ReasoningWireFormat::OpenAi {
            if effort.as_str() == "off" {
                return Ok((Some(ThinkingMode::Disabled), None));
            }
            return Ok((Some(ThinkingMode::Enabled), Some(effort.to_string())));
        }
        match effort.as_str() {
            "off" => Some(DeepSeekReasoningEffort::Off),
            "low" => Some(DeepSeekReasoningEffort::Low),
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
            Some(
                DeepSeekReasoningEffort::Low
                    | DeepSeekReasoningEffort::High
                    | DeepSeekReasoningEffort::Max
            )
        )
    {
        return Err(failure(
            "DeepSeek deployment does not support the requested reasoning effort",
            "UNSUPPORTED_REASONING_EFFORT",
        ));
    }
    Ok(match effort {
        Some(DeepSeekReasoningEffort::Off) => (Some(ThinkingMode::Disabled), None),
        Some(DeepSeekReasoningEffort::Low) => {
            (Some(ThinkingMode::Enabled), Some("low".to_string()))
        }
        Some(DeepSeekReasoningEffort::High) => {
            (Some(ThinkingMode::Enabled), Some("high".to_string()))
        }
        Some(DeepSeekReasoningEffort::Max) => {
            (Some(ThinkingMode::Enabled), Some("max".to_string()))
        }
        None => (defaults.thinking, None),
    })
}

pub(crate) fn serialize_request_with_prepared_images(
    options: &GenerateOptions,
    defaults: &RequestDefaults,
    reasoning_wire_format: crate::ReasoningWireFormat,
    image_urls: Option<&HashMap<String, String>>,
    image_file_ids: Option<&HashMap<String, String>>,
    image_meta: Option<&HashMap<String, PreparedImageMeta>>,
) -> Result<Value, LlmFailure> {
    let mut body = Map::new();
    body.insert("model".to_string(), json!(options.model));
    body.insert(
        "messages".to_string(),
        Value::Array(serialize_messages(
            options,
            image_urls,
            image_file_ids,
            image_meta,
        )?),
    );
    body.insert("stream".to_string(), json!(true));
    body.insert("stream_options".to_string(), json!({"include_usage": true}));

    let (thinking, reasoning_effort) = resolve_thinking(options, defaults, reasoning_wire_format)?;
    if reasoning_wire_format == crate::ReasoningWireFormat::DeepSeek
        && let Some(thinking) = thinking
    {
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
