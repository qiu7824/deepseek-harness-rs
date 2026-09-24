//! DeepSeek Messages request projection. Persistent message values are never edited.
use std::collections::{HashMap, HashSet};

use dsh_llm::{ContentBlock, GenerateOptions, LlmFailure, Message, MessageSource, Role};
use serde_json::{Value, json};

use crate::serialize::PreparedImageMeta;
use crate::{DeepSeekReasoningEffort, ResolvedDeepSeekOptions, ThinkingMode, failure};

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;

pub const API: &str = "deepseek-messages";

pub(crate) fn is_official_base(base: &str) -> bool {
    reqwest::Url::parse(base).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("api.deepseek.com")
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn input_content(
    blocks: &[ContentBlock],
    urls: Option<&HashMap<String, String>>,
    files: Option<&HashMap<String, String>>,
    metadata: Option<&HashMap<String, PreparedImageMeta>>,
) -> Result<Vec<Value>, LlmFailure> {
    let mut result = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Image {
                offloaded: Some(true),
                ..
            } => {
                result.push(json!({"type":"text","text":dsh_llm::OFFLOADED_IMAGE_TEXT}));
            }
            ContentBlock::Text { text } if !text.is_empty() => {
                result.push(json!({"type":"text","text":text}))
            }
            ContentBlock::Image { attachment, .. } => {
                let id = &attachment.attachment_id;
                if let Some(meta) = metadata.and_then(|items| items.get(id)) {
                    result.push(json!({"type":"text","text":format!("Image {}; request image {}x{}px.", id, meta.width, meta.height)}));
                }
                let source = if let Some(files) = files {
                    let file = files.get(id).ok_or_else(|| {
                        failure("Missing Messages image file id", "ATTACHMENT_NOT_FOUND")
                    })?;
                    json!({"type":"file","file_id":file})
                } else {
                    let url = urls.and_then(|items| items.get(id)).ok_or_else(|| {
                        failure("Missing Messages image bytes", "ATTACHMENT_NOT_FOUND")
                    })?;
                    let (media, data) = url
                        .strip_prefix("data:")
                        .and_then(|value| value.split_once(";base64,"))
                        .ok_or_else(|| {
                            failure(
                                "Messages image must use prepared inline bytes",
                                "INVALID_REQUEST",
                            )
                        })?;
                    json!({"type":"base64","media_type":media,"data":data})
                };
                result.push(json!({"type":"image","source":source}));
            }
            ContentBlock::ToolResult {
                tool_call_id: id,
                content,
                is_error,
            } => {
                if content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
                {
                    return Err(failure(
                        "Messages tool results cannot contain nested tool results",
                        "UNSUPPORTED_CONTENT",
                    ));
                }
                result.push(json!({"type":"tool_result","tool_use_id":id.as_str(),
                    "content":input_content(content, urls, files, metadata)?,"is_error":is_error.unwrap_or(false)}));
            }
            ContentBlock::Text { .. }
            | ContentBlock::Reasoning { .. }
            | ContentBlock::ToolCall { .. } => {}
            other => {
                return Err(failure(
                    format!("Unsupported Messages content {}", other.type_tag()),
                    "UNSUPPORTED_CONTENT",
                ));
            }
        }
    }
    Ok(result)
}

fn assistant_content(
    message: &Message,
    options: &GenerateOptions,
    connection: &ResolvedDeepSeekOptions,
) -> Result<Vec<Value>, LlmFailure> {
    // Replay only signatures whose provider, model and endpoint still match.
    // Content always comes from the durable message, not an opaque wire copy.
    let replay = match &message.source {
        MessageSource::Model {
            provider,
            model,
            replay_state: Some(state),
        } if provider == &options.provider
            && (model == &options.model || state["requestedModel"] == options.model)
            && state["protocol"] == API
            && state["endpointHash"]
                == crate::anthropic::replay_endpoint_hash(&connection.base_url) =>
        {
            state.get("content").and_then(Value::as_array)
        }
        _ => None,
    };
    message
        .content
        .iter()
        .enumerate()
        .map(|(index, block)| match block {
            ContentBlock::Text { text } => Ok(json!({"type":"text","text":text})),
            ContentBlock::Reasoning { text } => {
                let mut value = json!({"type":"thinking","thinking":text});
                if let Some(signature) = replay
                    .and_then(|blocks| blocks.get(index))
                    .filter(|block| block["type"] == "thinking" && block["thinking"] == *text)
                    .and_then(|block| block.get("signature"))
                    .and_then(Value::as_str)
                {
                    value["signature"] = json!(signature);
                }
                Ok(value)
            }
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
            } => Ok(json!({"type":"tool_use","id":id.as_str(),"name":name,
            "input":crate::anthropic::historical_tool_input(arguments)})),
            _ => Err(failure(
                "Unsupported DeepSeek Messages assistant content",
                "UNSUPPORTED_CONTENT",
            )),
        })
        .collect()
}

fn flush_system(messages: &mut Vec<Value>, updates: &mut Vec<Value>) -> Result<(), LlmFailure> {
    if !updates.is_empty() {
        if !messages.last().is_some_and(|value| value["role"] == "user") {
            return Err(failure(
                "In-history system update requires preceding user or tool results",
                "INVALID_REQUEST",
            ));
        }
        messages.append(updates);
    }
    Ok(())
}

pub(crate) fn serialize(
    options: &GenerateOptions,
    connection: &ResolvedDeepSeekOptions,
    urls: Option<&HashMap<String, String>>,
    files: Option<&HashMap<String, String>>,
    metadata: Option<&HashMap<String, PreparedImageMeta>>,
) -> Result<Value, LlmFailure> {
    crate::serialize::validate_projected_content(options)?;
    let model = connection
        .models
        .iter()
        .find(|model| model.id == options.model);
    let in_history = model.is_some_and(|model| {
        model.system_prompt_update == Some(dsh_llm::SystemPromptUpdate::InHistory)
    });
    let mut messages: Vec<Value> = Vec::new();
    let mut updates = Vec::new();
    let mut history_system = None;
    for message in &options.messages {
        if message.role == Role::Developer {
            return Err(failure(
                "Messages does not support developer history on this route",
                "UNSUPPORTED_CONTENT",
            ));
        }
        if message.role == Role::System {
            let mut text = String::new();
            for block in &message.content {
                let ContentBlock::Text { text: part } = block else {
                    return Err(failure(
                        "Messages system content must be text",
                        "UNSUPPORTED_CONTENT",
                    ));
                };
                text.push_str(part);
            }
            if in_history && !messages.is_empty() {
                if text.is_empty() {
                    return Err(failure(
                        "Empty in-history system update",
                        "UNSUPPORTED_CONTENT",
                    ));
                }
                updates.push(json!({"role":"system","content":[{"type":"text","text":text}]}));
            } else {
                history_system = Some(text);
            }
            continue;
        }
        let assistant = message.role == Role::Assistant;
        if assistant {
            flush_system(&mut messages, &mut updates)?;
        }
        let content = if assistant {
            assistant_content(message, options, connection)?
        } else if message.role == Role::Tool {
            let (id, content, error) = message
                .as_tool_result()
                .ok_or_else(|| failure("Tool message is missing its call id", "INVALID_REQUEST"))?;
            if content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
            {
                return Err(failure(
                    "Tool message content cannot contain result wrappers",
                    "INVALID_REQUEST",
                ));
            }
            let mut result = json!({"type":"tool_result","tool_use_id":id.as_str(),"content":input_content(content, urls, files, metadata)?});
            if let Some(error) = error {
                result["is_error"] = json!(error);
            }
            vec![result]
        } else {
            input_content(&message.content, urls, files, metadata)?
        };
        if !assistant && content.is_empty() {
            continue;
        }
        let role = if assistant { "assistant" } else { "user" };
        if let Some(previous) = messages
            .last_mut()
            .filter(|previous| previous["role"] == role)
        {
            previous["content"].as_array_mut().unwrap().extend(content);
        } else {
            messages.push(json!({"role":role,"content":content}));
        }
    }
    flush_system(&mut messages, &mut updates)?;
    let mut pending = HashSet::new();
    for message in &mut messages {
        let content = message["content"].as_array().unwrap();
        if message["role"] == "assistant" {
            if !pending.is_empty() {
                return Err(failure(
                    "Messages tool calls need immediate results",
                    "INVALID_REQUEST",
                ));
            }
            for block in content.iter().filter(|block| block["type"] == "tool_use") {
                let id = block["id"].as_str().unwrap_or("");
                if id.is_empty() || !pending.insert(id.to_owned()) {
                    return Err(failure(
                        "Messages duplicate or empty tool id",
                        "INVALID_REQUEST",
                    ));
                }
            }
        } else if message["role"] == "user" {
            for block in content
                .iter()
                .filter(|block| block["type"] == "tool_result")
            {
                if !pending.remove(block["tool_use_id"].as_str().unwrap_or("")) {
                    return Err(failure(
                        "Messages tool result has no matching call",
                        "INVALID_REQUEST",
                    ));
                }
            }
            if !pending.is_empty() {
                return Err(failure(
                    "Messages tool calls need immediate results",
                    "INVALID_REQUEST",
                ));
            }
            message["content"]
                .as_array_mut()
                .unwrap()
                .sort_by_key(|block| block["type"] != "tool_result");
        }
    }
    if !pending.is_empty() {
        return Err(failure(
            "Messages history ends with unresolved tools",
            "INVALID_REQUEST",
        ));
    }
    let default_effort = match connection.defaults.reasoning_effort {
        Some(DeepSeekReasoningEffort::Off) => "off",
        Some(DeepSeekReasoningEffort::Low) => "low",
        Some(DeepSeekReasoningEffort::High) => "high",
        Some(DeepSeekReasoningEffort::Max) => "max",
        None if connection.defaults.thinking == Some(ThinkingMode::Disabled) => "off",
        None => "high",
    };
    let effort = if options.purpose.as_deref() == Some("session-title") {
        "off"
    } else {
        options
            .reasoning_effort
            .as_ref()
            .map(|value| value.as_str())
            .unwrap_or(default_effort)
    };
    if !matches!(effort, "off" | "low" | "high" | "max")
        || (connection.defaults.thinking == Some(ThinkingMode::Disabled) && effort != "off")
    {
        return Err(failure(
            "Unsupported DeepSeek Messages reasoning effort",
            "UNSUPPORTED_REASONING_EFFORT",
        ));
    }
    let mut body = json!({"model":options.model,"stream":true,"messages":messages,
        "max_tokens":options.max_tokens.or_else(||model.and_then(|model| model.max_tokens)).unwrap_or(connection.max_tokens),
        "thinking":{"type":if effort == "off" {"disabled"} else {"enabled"}}});
    if effort != "off" {
        body["output_config"] = json!({"effort":effort});
    }
    let system = [options.system.as_deref(), history_system.as_deref()]
        .into_iter()
        .flatten()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if let Some(value) = options.temperature {
        body["temperature"] = json!(value);
    }
    if let Some(value) = &options.stop {
        body["stop_sequences"] = json!(value);
    }
    if let Some(tools) = &options.tools {
        body["tools"] = json!(tools.iter().map(|tool| json!({"name":tool.name,"description":tool.description,"input_schema":tool.parameters})).collect::<Vec<_>>());
    }
    Ok(body)
}
