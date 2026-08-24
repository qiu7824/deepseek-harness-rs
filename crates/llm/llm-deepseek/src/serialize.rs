use std::collections::HashMap;

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
) -> Result<(Option<ThinkingMode>, Option<&'static str>), LlmFailure> {
    if options.purpose.as_deref() == Some("session-title") {
        return Ok((Some(ThinkingMode::Disabled), None));
    }
    let effort = if let Some(effort) = options.reasoning_effort.as_ref() {
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
        Some(DeepSeekReasoningEffort::Low) => (Some(ThinkingMode::Enabled), Some("low")),
        Some(DeepSeekReasoningEffort::High) => (Some(ThinkingMode::Enabled), Some("high")),
        Some(DeepSeekReasoningEffort::Max) => (Some(ThinkingMode::Enabled), Some("max")),
        None => (defaults.thinking, None),
    })
}

#[cfg(test)]
pub(crate) fn serialize_request(
    options: &GenerateOptions,
    defaults: &RequestDefaults,
    include_thinking_fields: bool,
    image_urls: Option<&HashMap<String, String>>,
) -> Result<Value, LlmFailure> {
    serialize_request_with_files(options, defaults, include_thinking_fields, image_urls, None)
}

#[cfg(test)]
pub(crate) fn serialize_request_with_files(
    options: &GenerateOptions,
    defaults: &RequestDefaults,
    include_thinking_fields: bool,
    image_urls: Option<&HashMap<String, String>>,
    image_file_ids: Option<&HashMap<String, String>>,
) -> Result<Value, LlmFailure> {
    serialize_request_with_prepared_images(
        options,
        defaults,
        include_thinking_fields,
        image_urls,
        image_file_ids,
        None,
    )
}

pub(crate) fn serialize_request_with_prepared_images(
    options: &GenerateOptions,
    defaults: &RequestDefaults,
    include_thinking_fields: bool,
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

    let (thinking, reasoning_effort) = if include_thinking_fields {
        resolve_thinking(options, defaults)?
    } else {
        (None, None)
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replays_reasoning_content_on_a_tool_call_free_assistant_turn() {
        let assistant = dsh_llm::create_message(
            dsh_llm::Role::Assistant,
            vec![
                dsh_llm::ContentBlock::Reasoning {
                    text: "private chain".to_string(),
                },
                dsh_llm::ContentBlock::Text {
                    text: "visible answer".to_string(),
                },
            ],
            dsh_llm::MessageSource::Model {
                provider: "deepseek-official".to_string(),
                model: "deepseek-reasoner".to_string(),
                replay_state: None,
            },
        );
        let options = GenerateOptions {
            provider: "deepseek-official".to_string(),
            model: "deepseek-reasoner".to_string(),
            messages: vec![assistant],
            system: None,
            tools: None,
            temperature: None,
            max_tokens: None,
            stop: None,
            signal: None,
            session_id: None,
            purpose: None,
            reasoning_effort: None,
            agent_loop_request: false,
        };
        let body = serialize_request(&options, &RequestDefaults::default(), true, None)
            .expect("reasoned replay");
        assert_eq!(body["messages"][0]["content"], "visible answer");
        assert_eq!(body["messages"][0]["reasoning_content"], "private chain");
        assert!(body["messages"][0].get("tool_calls").is_none());
    }

    #[test]
    fn serializes_user_text_and_image_in_original_order() {
        let message = dsh_llm::create_user_message(
            vec![
                dsh_llm::ContentBlock::Text {
                    text: "inspect".to_string(),
                },
                dsh_llm::ContentBlock::Image {
                    attachment: dsh_llm::ImageAttachmentRef {
                        attachment_id: "sha256-image".to_string(),
                        media_type: Some("image/png".to_string()),
                        bytes: Some(68),
                        width: Some(1),
                        height: Some(1),
                        name: Some("pixel.png".to_string()),
                    },
                },
            ],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        let options = GenerateOptions {
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4-flash".to_string(),
            messages: vec![message],
            system: None,
            tools: None,
            temperature: None,
            max_tokens: None,
            stop: None,
            signal: None,
            session_id: None,
            purpose: None,
            reasoning_effort: None,
            agent_loop_request: false,
        };
        let image_urls = HashMap::from([(
            "sha256-image".to_string(),
            "data:image/png;base64,AAAA".to_string(),
        )]);
        let body = serialize_request(
            &options,
            &RequestDefaults::default(),
            true,
            Some(&image_urls),
        )
        .expect("multimodal request");
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][0]["text"], "inspect");
        assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,AAAA"
        );
        let file_ids = HashMap::from([("sha256-image".to_string(), "file-123".to_string())]);
        let file_body = serialize_request_with_files(
            &options,
            &RequestDefaults::default(),
            true,
            None,
            Some(&file_ids),
        )
        .expect("file request");
        assert_eq!(file_body["messages"][0]["content"][1]["type"], "file");
        assert_eq!(
            file_body["messages"][0]["content"][1]["file_id"],
            "file-123"
        );
    }

    fn options_with_messages(messages: Vec<dsh_llm::Message>) -> GenerateOptions {
        GenerateOptions {
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4-flash".to_string(),
            messages,
            system: Some("stable system".to_string()),
            tools: None,
            temperature: None,
            max_tokens: None,
            stop: None,
            signal: None,
            session_id: None,
            purpose: None,
            reasoning_effort: Some(dsh_llm::reasoning_effort_id("high")),
            agent_loop_request: true,
        }
    }

    #[test]
    fn later_turns_append_without_rewriting_the_existing_wire_prefix() {
        let first_user = dsh_llm::create_user_message(
            vec![ContentBlock::Text {
                text: "first".to_string(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        let assistant_tool = dsh_llm::create_message(
            Role::Assistant,
            vec![
                ContentBlock::Reasoning {
                    text: "reason one".to_string(),
                },
                ContentBlock::ToolCall {
                    id: dsh_llm::call_id("call-stable"),
                    name: "read".to_string(),
                    arguments: "{\"path\":\"a.txt\"}".to_string(),
                },
            ],
            dsh_llm::MessageSource::Model {
                provider: "deepseek-official".to_string(),
                model: "deepseek-v4-flash".to_string(),
                replay_state: None,
            },
        );
        let tool_result = dsh_llm::create_user_message(
            vec![ContentBlock::ToolResult {
                tool_call_id: dsh_llm::call_id("call-stable"),
                content: vec![ContentBlock::Text {
                    text: "result one".to_string(),
                }],
                is_error: Some(false),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        let first_history = vec![first_user, assistant_tool, tool_result];
        let first = serialize_request(
            &options_with_messages(first_history.clone()),
            &RequestDefaults::default(),
            true,
            None,
        )
        .expect("first wire");

        let mut second_history = first_history;
        second_history.push(dsh_llm::create_user_message(
            vec![ContentBlock::Text {
                text: "second".to_string(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ));
        let second = serialize_request(
            &options_with_messages(second_history.clone()),
            &RequestDefaults::default(),
            true,
            None,
        )
        .expect("second wire");
        let first_messages = first["messages"].as_array().expect("first messages");
        let second_messages = second["messages"].as_array().expect("second messages");
        assert_eq!(&second_messages[..first_messages.len()], first_messages);

        second_history.push(dsh_llm::create_message(
            Role::Assistant,
            vec![
                ContentBlock::Reasoning {
                    text: "reason two".to_string(),
                },
                ContentBlock::Text {
                    text: "answer two".to_string(),
                },
            ],
            dsh_llm::MessageSource::Model {
                provider: "deepseek-official".to_string(),
                model: "deepseek-v4-flash".to_string(),
                replay_state: None,
            },
        ));
        second_history.push(dsh_llm::create_user_message(
            vec![ContentBlock::Text {
                text: "third".to_string(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ));
        let third = serialize_request(
            &options_with_messages(second_history),
            &RequestDefaults::default(),
            true,
            None,
        )
        .expect("third wire");
        let third_messages = third["messages"].as_array().expect("third messages");
        assert_eq!(&third_messages[..second_messages.len()], second_messages);
    }

    #[test]
    fn serializes_tool_result_images_after_the_tool_message() {
        let image = dsh_llm::ContentBlock::Image {
            attachment: dsh_llm::ImageAttachmentRef {
                attachment_id: "tool-image".to_string(),
                media_type: Some("image/png".to_string()),
                bytes: Some(68),
                width: Some(1),
                height: Some(1),
                name: Some("tool.png".to_string()),
            },
        };
        let message = dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::ToolResult {
                tool_call_id: dsh_llm::call_id("call-tool-image"),
                content: vec![
                    dsh_llm::ContentBlock::Text {
                        text: "tool text".to_string(),
                    },
                    image,
                ],
                is_error: Some(false),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        let options = GenerateOptions {
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4-flash".to_string(),
            messages: vec![message],
            system: None,
            tools: None,
            temperature: None,
            max_tokens: None,
            stop: None,
            signal: None,
            session_id: None,
            purpose: None,
            reasoning_effort: None,
            agent_loop_request: false,
        };
        let file_ids = HashMap::from([("tool-image".to_string(), "file-tool".to_string())]);
        let image_meta = HashMap::from([(
            "tool-image".to_string(),
            PreparedImageMeta {
                attachment_id: "tool-image".to_string(),
                bytes: 3,
                width: 1,
                height: 1,
            },
        )]);
        let body = serialize_request_with_prepared_images(
            &options,
            &RequestDefaults::default(),
            true,
            None,
            Some(&file_ids),
            Some(&image_meta),
        )
        .expect("tool-result image request");
        assert_eq!(body["messages"][0]["role"], "tool");
        assert_eq!(
            body["messages"][0]["content"],
            "tool textImage tool-image; request image 1x1px."
        );
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(
            body["messages"][1]["content"][0]["text"],
            TOOL_RESULT_IMAGE_TEXT
        );
        assert_eq!(body["messages"][1]["content"][1]["file_id"], "file-tool");
    }

    #[test]
    fn serializes_low_reasoning_effort() {
        let options = GenerateOptions {
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4-flash".to_string(),
            reasoning_effort: Some(dsh_llm::reasoning_effort_id("low")),
            messages: Vec::new(),
            system: None,
            tools: None,
            temperature: None,
            max_tokens: None,
            stop: None,
            signal: None,
            session_id: None,
            purpose: None,
            agent_loop_request: false,
        };
        let body = serialize_request(&options, &RequestDefaults::default(), true, None)
            .expect("low reasoning effort");
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "low");
    }
}
