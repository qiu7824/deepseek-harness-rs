use std::collections::BTreeMap;

use dsh_llm::{ContentBlock, FinishReason, LlmFailure, StreamChunk, TokenUsage, call_id};
use serde_json::{Value, json};

fn failure(message: impl Into<String>, code: &str) -> LlmFailure {
    LlmFailure {
        message: message.into(),
        code: code.to_string(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

pub(crate) fn request_from_chat(chat: &Value) -> Result<Value, LlmFailure> {
    let model = chat.get("model").cloned().unwrap_or(Value::Null);
    let mut input = Vec::new();
    let mut instructions = Vec::new();
    for message in chat
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = message.get("content");
        if role == "system" {
            if let Some(text) = content.and_then(Value::as_str) {
                instructions.push(text.to_string());
            }
            continue;
        }
        if role == "tool" {
            input.push(json!({
                "type": "function_call_output",
                "call_id": message.get("tool_call_id").and_then(Value::as_str).unwrap_or(""),
                "output": content.and_then(Value::as_str).unwrap_or("")
            }));
            continue;
        }
        let mut parts = Vec::new();
        let text_part_type = if role == "assistant" {
            "output_text"
        } else {
            "input_text"
        };
        match content {
            Some(Value::String(text)) if !text.is_empty() => {
                parts.push(json!({"type":text_part_type, "text":text}));
            }
            Some(Value::Array(items)) => {
                for item in items {
                    match item.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = item.get("text").and_then(Value::as_str) {
                                parts.push(json!({"type":text_part_type, "text":text}));
                            }
                        }
                        Some("image_url") => {
                            if let Some(url) =
                                item.pointer("/image_url/url").and_then(Value::as_str)
                            {
                                parts.push(json!({"type":"input_image", "image_url":url}));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if !parts.is_empty() {
            input.push(json!({"role":role, "content":parts}));
        }
        for call in message
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            input.push(json!({
                "type":"function_call",
                "call_id":call.get("id").and_then(Value::as_str).unwrap_or(""),
                "name":call.pointer("/function/name").and_then(Value::as_str).unwrap_or(""),
                "arguments":call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}")
            }));
        }
    }
    let tools = chat.get("tools").and_then(Value::as_array).map(|items| items.iter().filter_map(|tool| {
        let function = tool.get("function")?;
        Some(json!({
            "type":"function",
            "name":function.get("name")?,
            "description":function.get("description").cloned().unwrap_or(Value::Null),
            "parameters":function.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object"})),
            "strict":function.get("strict").cloned().unwrap_or(Value::Bool(false))
        }))
    }).collect::<Vec<_>>()).unwrap_or_default();
    let mut body = json!({"model":model, "input":input, "stream":true});
    if !instructions.is_empty() {
        body["instructions"] = Value::String(instructions.join("\n\n"));
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(value) = chat.get("max_tokens") {
        body["max_output_tokens"] = value.clone();
    }
    if let Some(effort) = chat.pointer("/thinking/effort").and_then(Value::as_str) {
        body["reasoning"] = json!({"effort":effort, "summary":"auto"});
    }
    Ok(body)
}

#[derive(Default)]
pub(crate) struct ResponsesTranslator {
    next_index: u64,
    text: Option<(u64, String)>,
    reasoning: Option<(u64, String)>,
    tools: BTreeMap<String, (u64, String, String, String)>,
    completed: bool,
}

impl ResponsesTranslator {
    pub(crate) fn consume(&mut self, payload: &str) -> Result<Vec<StreamChunk>, LlmFailure> {
        if payload == "[DONE]" {
            return Ok(Vec::new());
        }
        let event: Value = serde_json::from_str(payload).map_err(|error| {
            failure(
                format!("malformed Responses SSE payload: {error}"),
                "MALFORMED_RESPONSE",
            )
        })?;
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let mut out = Vec::new();
        match kind {
            "response.output_text.delta" => {
                let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                if self.text.is_none() {
                    let index = self.next_index;
                    self.next_index += 1;
                    self.text = Some((index, String::new()));
                    out.push(StreamChunk::BlockStart {
                        index,
                        block_type: "text".to_string(),
                    });
                }
                let (index, text) = self.text.as_mut().expect("text");
                text.push_str(delta);
                out.push(StreamChunk::TextDelta {
                    index: *index,
                    text: delta.to_string(),
                });
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                if self.reasoning.is_none() {
                    let index = self.next_index;
                    self.next_index += 1;
                    self.reasoning = Some((index, String::new()));
                    out.push(StreamChunk::BlockStart {
                        index,
                        block_type: "reasoning".to_string(),
                    });
                }
                let (index, text) = self.reasoning.as_mut().expect("reasoning");
                text.push_str(delta);
                out.push(StreamChunk::ReasoningDelta {
                    index: *index,
                    text: delta.to_string(),
                });
            }
            "response.output_item.added" => {
                if event.pointer("/item/type").and_then(Value::as_str) == Some("function_call") {
                    let item_id = event
                        .pointer("/item/id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let call = event
                        .pointer("/item/call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = event
                        .pointer("/item/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let index = self.next_index;
                    self.next_index += 1;
                    self.tools
                        .insert(item_id, (index, call.clone(), name.clone(), String::new()));
                    out.push(StreamChunk::BlockStart {
                        index,
                        block_type: "tool-call".to_string(),
                    });
                    out.push(StreamChunk::ToolCallDelta {
                        index,
                        id: call_id(call),
                        name: Some(name),
                        arguments_delta: String::new(),
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                let item_id = event.get("item_id").and_then(Value::as_str).unwrap_or("");
                if let Some((index, call, _name, args)) = self.tools.get_mut(item_id) {
                    let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                    args.push_str(delta);
                    out.push(StreamChunk::ToolCallDelta {
                        index: *index,
                        id: call_id(call.clone()),
                        name: None,
                        arguments_delta: delta.to_string(),
                    });
                }
            }
            "response.completed" => {
                if let Some((index, text)) = self.reasoning.take() {
                    out.push(StreamChunk::BlockEnd {
                        index,
                        block: ContentBlock::Reasoning { text },
                    });
                }
                if let Some((index, text)) = self.text.take() {
                    out.push(StreamChunk::BlockEnd {
                        index,
                        block: ContentBlock::Text { text },
                    });
                }
                for (_, (index, call, name, args)) in std::mem::take(&mut self.tools) {
                    out.push(StreamChunk::BlockEnd {
                        index,
                        block: ContentBlock::ToolCall {
                            id: call_id(call),
                            name,
                            arguments: args,
                        },
                    });
                }
                if let Some(usage) = event.pointer("/response/usage") {
                    out.push(StreamChunk::Usage {
                        usage: TokenUsage {
                            input_tokens: usage
                                .get("input_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                            output_tokens: usage
                                .get("output_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                            cache_read_tokens: usage
                                .pointer("/input_tokens_details/cached_tokens")
                                .and_then(Value::as_u64),
                            cache_write_tokens: None,
                            reasoning_tokens: usage
                                .pointer("/output_tokens_details/reasoning_tokens")
                                .and_then(Value::as_u64),
                        },
                    });
                }
                self.completed = true;
                out.push(StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                });
            }
            "response.failed" | "response.incomplete" => {
                return Err(failure(
                    event
                        .pointer("/response/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("Responses request failed"),
                    "PROVIDER_ERROR",
                ));
            }
            _ => {}
        }
        Ok(out)
    }
    pub(crate) fn finish(&self) -> Result<(), LlmFailure> {
        if self.completed {
            Ok(())
        } else {
            Err(failure(
                "Responses stream ended before response.completed",
                "STREAM_CLOSED",
            ))
        }
    }
}
