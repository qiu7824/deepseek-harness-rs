//! Anthropic Messages conversion and lossless thinking/tool streaming.
use dsh_llm::{
    ContentBlock, FinishReason, GenerateOptions, LlmFailure, MessageSource, Role, StreamChunk,
    TokenUsage, call_id,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn failure(message: impl Into<String>, code: &str) -> LlmFailure {
    LlmFailure {
        message: message.into(),
        code: code.into(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

/// The generic chat serializer does not carry provider-private replay state.
/// Attach it only for the Anthropic route before converting the wire body.
pub(crate) fn attach_replay(chat: &mut Value, options: &GenerateOptions) {
    let Some(wire) = chat.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for (message, source) in wire.iter_mut().filter(|m| m["role"] == "assistant").zip(
        options
            .messages
            .iter()
            .filter(|m| m.role == Role::Assistant),
    ) {
        if let MessageSource::Model {
            replay_state: Some(state),
            ..
        } = &source.source
        {
            if state["protocol"] == "anthropic-messages" && state["content"].is_array() {
                message["anthropic_content"] = state["content"].clone();
            }
        }
    }
}

pub(crate) fn request_from_chat(chat: &Value) -> Result<Value, LlmFailure> {
    let model = chat
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| failure("Anthropic model is required", "INVALID_REQUEST"))?;
    let mut messages: Vec<Value> = Vec::new();
    let mut system = Vec::new();
    for message in chat
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| failure("Anthropic messages are required", "INVALID_REQUEST"))?
    {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        if matches!(role, "system" | "developer") {
            system.extend(content(message.get("content"))?);
            continue;
        }
        let mut parts = if role == "assistant" && message["anthropic_content"].is_array() {
            message["anthropic_content"].as_array().unwrap().clone()
        } else {
            content(message.get("content"))?
        };
        if role == "tool" {
            let id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| failure("Tool result is missing its call id", "INVALID_REQUEST"))?;
            parts = vec![
                json!({"type":"tool_result","tool_use_id":id,"content":parts,"is_error":message.get("is_error").and_then(Value::as_bool).unwrap_or(false)}),
            ];
        } else if role == "assistant" && !message["anthropic_content"].is_array() {
            // Unsigned reasoning from another provider is not replayable as an
            // Anthropic thinking block. Preserve normal text and tool calls.
            for call in message
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| failure("Tool call is missing its id", "INVALID_REQUEST"))?;
                let name = call
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| failure("Tool call is missing its name", "INVALID_REQUEST"))?;
                let input: Value = serde_json::from_str(
                    call.pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}"),
                )
                .map_err(|_| {
                    failure("Tool call arguments are not valid JSON", "INVALID_REQUEST")
                })?;
                if !input.is_object() {
                    return Err(failure(
                        "Anthropic tool arguments must be an object",
                        "INVALID_REQUEST",
                    ));
                }
                parts.push(json!({"type":"tool_use","id":id,"name":name,"input":input}));
            }
        }
        if parts.is_empty() {
            continue;
        }
        let wire_role = if role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        if let Some(previous) = messages.last_mut().filter(|p| p["role"] == wire_role) {
            let existing = previous["content"].as_array_mut().unwrap();
            if role == "tool" {
                let end = existing
                    .iter()
                    .take_while(|p| p["type"] == "tool_result")
                    .count();
                existing.splice(end..end, parts);
            } else {
                existing.extend(parts)
            }
        } else {
            messages.push(json!({"role":wire_role,"content":parts}));
        }
    }
    let max_tokens = chat
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(16384);
    if max_tokens == 0 {
        return Err(failure(
            "Anthropic max_tokens must be positive",
            "INVALID_REQUEST",
        ));
    }
    let mut body = json!({"model":model,"messages":messages,"max_tokens":max_tokens,"stream":true});
    if !system.is_empty() {
        body["system"] = json!(system)
    }
    if let Some(tools) = chat
        .get("tools")
        .and_then(Value::as_array)
        .filter(|t| !t.is_empty())
    {
        body["tools"]=Value::Array(tools.iter().map(|tool|{let f=&tool["function"];json!({"name":f["name"],"description":f["description"],"input_schema":f["parameters"]})}).collect());
    }
    if let Some(stop) = chat.get("stop") {
        body["stop_sequences"] = if stop.is_string() {
            json!([stop])
        } else {
            stop.clone()
        };
    }
    if let Some(choice) = chat.get("tool_choice") {
        body["tool_choice"] = match choice.as_str() {
            Some("auto") => json!({"type":"auto"}),
            Some("required") => json!({"type":"any"}),
            Some("none") => json!({"type":"none"}),
            _ if choice.pointer("/function/name").is_some() => {
                json!({"type":"tool","name":choice["function"]["name"]})
            }
            _ => {
                return Err(failure(
                    "Unsupported Anthropic tool choice",
                    "INVALID_REQUEST",
                ));
            }
        };
    }
    let effort = chat
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .or_else(|| chat.pointer("/thinking/effort").and_then(Value::as_str));
    if effort == Some("off")
        || chat.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled")
    {
        body["thinking"] = json!({"type":"disabled"});
    } else if let Some(effort) = effort {
        let model = model.to_lowercase().replace('.', "-");
        let adaptive = [
            "opus-4-6",
            "opus-4-7",
            "opus-4-8",
            "sonnet-4-6",
            "sonnet-5",
            "opus-5",
            "fable-5",
            "mythos",
        ]
        .iter()
        .any(|known| model.contains(known));
        if adaptive {
            let xhigh = [
                "opus-4-7", "opus-4-8", "sonnet-5", "opus-5", "fable-5", "mythos-5",
            ]
            .iter()
            .any(|known| model.contains(known));
            if !matches!(effort, "low" | "medium" | "high" | "max") && !(effort == "xhigh" && xhigh)
            {
                return Err(failure(
                    format!("Effort {effort:?} is not supported by this Anthropic model"),
                    "UNSUPPORTED_REASONING",
                ));
            }
            body["thinking"] = json!({"type":"adaptive"});
            body["output_config"] = json!({"effort":effort});
        } else {
            let extended = ["3-7-sonnet", "sonnet-4", "opus-4", "haiku-4-5"]
                .iter()
                .any(|known| model.contains(known));
            if !extended || !matches!(effort, "low" | "medium" | "high" | "max") {
                return Err(failure(
                    "This Anthropic model does not support the requested reasoning level",
                    "UNSUPPORTED_REASONING",
                ));
            }
            if max_tokens <= 1024 {
                return Err(failure(
                    "Extended thinking needs max_tokens greater than 1024",
                    "INVALID_REQUEST",
                ));
            }
            let budget = match effort {
                "low" => 1024,
                "medium" => 4096,
                "high" => 8192,
                _ => 16384,
            }
            .min(max_tokens - 1);
            body["thinking"] = json!({"type":"enabled","budget_tokens":budget});
            if model.contains("opus-4-5") && effort != "max" {
                body["output_config"] = json!({"effort":effort});
            }
        }
    }
    if body.get("thinking").is_none()
        || body.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled")
    {
        if let Some(temperature) = chat.get("temperature") {
            body["temperature"] = temperature.clone();
        }
    }
    Ok(body)
}

fn content(value: Option<&Value>) -> Result<Vec<Value>, LlmFailure> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(if text.is_empty() {
            vec![]
        } else {
            vec![json!({"type":"text","text":text})]
        }),
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => Ok(json!({"type":"text","text":part["text"]})),
                Some("image_url") => {
                    let url = part
                        .pointer("/image_url/url")
                        .and_then(Value::as_str)
                        .ok_or_else(|| failure("Missing image URL", "UNSUPPORTED_CONTENT"))?;
                    let source = if let Some(data) = url.strip_prefix("data:") {
                        let (kind, encoded) = data.split_once(";base64,").ok_or_else(|| {
                            failure(
                                "Anthropic requires a base64 image data URL",
                                "UNSUPPORTED_CONTENT",
                            )
                        })?;
                        if !matches!(
                            kind,
                            "image/jpeg" | "image/png" | "image/gif" | "image/webp"
                        ) {
                            return Err(failure(
                                "Unsupported Anthropic image media type",
                                "UNSUPPORTED_CONTENT",
                            ));
                        }
                        json!({"type":"base64","media_type":kind,"data":encoded})
                    } else if url.starts_with("https://") || url.starts_with("http://") {
                        json!({"type":"url","url":url})
                    } else {
                        return Err(failure(
                            "Unsupported Anthropic image URL",
                            "UNSUPPORTED_CONTENT",
                        ));
                    };
                    Ok(json!({"type":"image","source":source}))
                }
                _ => Err(failure(
                    "Unsupported Anthropic message content",
                    "UNSUPPORTED_CONTENT",
                )),
            })
            .collect(),
        _ => Err(failure(
            "Invalid Anthropic content value",
            "INVALID_REQUEST",
        )),
    }
}

#[derive(Default)]
pub(crate) struct AnthropicTranslator {
    blocks: BTreeMap<u64, Value>,
    finished: BTreeMap<u64, Value>,
    args: BTreeMap<u64, String>,
    usage: TokenUsage,
    stop: Option<String>,
    completed: bool,
}
impl AnthropicTranslator {
    pub(crate) fn consume(&mut self, payload: &str) -> Result<Vec<StreamChunk>, LlmFailure> {
        let event: Value = serde_json::from_str(payload).map_err(|e| {
            failure(
                format!("Malformed Anthropic SSE: {e}"),
                "MALFORMED_RESPONSE",
            )
        })?;
        let mut out = Vec::new();
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "ping" => (),
            "error" => {
                return Err(failure(
                    event
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("Anthropic stream error"),
                    event
                        .pointer("/error/type")
                        .and_then(Value::as_str)
                        .unwrap_or("PROVIDER_ERROR"),
                ));
            }
            "message_start" => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.merge_usage(usage);
                }
            }
            "content_block_start" => {
                let index = index(&event)?;
                let block = event
                    .get("content_block")
                    .cloned()
                    .ok_or_else(|| failure("Missing content block", "MALFORMED_RESPONSE"))?;
                if self.blocks.contains_key(&index) || self.finished.contains_key(&index) {
                    return Err(failure("Duplicate content block", "MALFORMED_RESPONSE"));
                }
                let kind = block["type"].as_str().unwrap_or("");
                let block_type = match kind {
                    "text" => "text",
                    "thinking" => "reasoning",
                    "tool_use" => "tool-call",
                    "redacted_thinking" => {
                        self.blocks.insert(index, block);
                        return Ok(out);
                    }
                    _ => {
                        return Err(failure(
                            format!("Unsupported Anthropic content block {kind}"),
                            "UNSUPPORTED_CONTENT",
                        ));
                    }
                };
                out.push(StreamChunk::BlockStart {
                    index,
                    block_type: block_type.into(),
                });
                match kind {
                    "text" => {
                        let text = block["text"].as_str().unwrap_or("");
                        if !text.is_empty() {
                            out.push(StreamChunk::TextDelta {
                                index,
                                text: text.into(),
                            })
                        }
                    }
                    "thinking" => {
                        let text = block["thinking"].as_str().unwrap_or("");
                        if !text.is_empty() {
                            out.push(StreamChunk::ReasoningDelta {
                                index,
                                text: text.into(),
                            })
                        }
                    }
                    "tool_use" => out.push(StreamChunk::ToolCallDelta {
                        index,
                        id: call_id(required(&block, "id")?),
                        name: Some(required(&block, "name")?),
                        arguments_delta: String::new(),
                    }),
                    _ => (),
                }
                self.blocks.insert(index, block);
            }
            "content_block_delta" => {
                let index = index(&event)?;
                let block = self.blocks.get_mut(&index).ok_or_else(|| {
                    failure("Delta before content block start", "MALFORMED_RESPONSE")
                })?;
                let delta = &event["delta"];
                match delta["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let text = required(delta, "text")?;
                        append(block, "text", &text);
                        out.push(StreamChunk::TextDelta { index, text });
                    }
                    "thinking_delta" => {
                        let text = required(delta, "thinking")?;
                        append(block, "thinking", &text);
                        out.push(StreamChunk::ReasoningDelta { index, text });
                    }
                    "signature_delta" => {
                        let signature = required(delta, "signature")?;
                        append(block, "signature", &signature);
                    }
                    "input_json_delta" => {
                        let partial = required(delta, "partial_json")?;
                        self.args.entry(index).or_default().push_str(&partial);
                        out.push(StreamChunk::ToolCallDelta {
                            index,
                            id: call_id(required(block, "id")?),
                            name: None,
                            arguments_delta: partial,
                        });
                    }
                    "citations_delta" => (),
                    _ => {
                        return Err(failure(
                            "Unsupported Anthropic content delta",
                            "UNSUPPORTED_CONTENT",
                        ));
                    }
                }
            }
            "content_block_stop" => {
                let index = index(&event)?;
                let mut block = self.blocks.remove(&index).ok_or_else(|| {
                    failure("Content block stop without start", "MALFORMED_RESPONSE")
                })?;
                let result = match block["type"].as_str().unwrap_or("") {
                    "text" => Some(ContentBlock::Text {
                        text: block["text"].as_str().unwrap_or("").into(),
                    }),
                    "thinking" => Some(ContentBlock::Reasoning {
                        text: block["thinking"].as_str().unwrap_or("").into(),
                    }),
                    "tool_use" => {
                        if let Some(args) = self.args.remove(&index) {
                            block["input"] = serde_json::from_str(&args).map_err(|_| {
                                failure(
                                    "Anthropic streamed invalid tool arguments",
                                    "MALFORMED_RESPONSE",
                                )
                            })?;
                        }
                        if !block["input"].is_object() {
                            return Err(failure(
                                "Anthropic tool input is not an object",
                                "MALFORMED_RESPONSE",
                            ));
                        }
                        Some(ContentBlock::ToolCall {
                            id: call_id(required(&block, "id")?),
                            name: required(&block, "name")?,
                            arguments: block["input"].to_string(),
                        })
                    }
                    "redacted_thinking" => None,
                    _ => return Err(failure("Unknown content block", "UNSUPPORTED_CONTENT")),
                };
                if let Some(block) = result {
                    out.push(StreamChunk::BlockEnd { index, block });
                }
                self.finished.insert(index, block);
            }
            "message_delta" => {
                if let Some(usage) = event.get("usage") {
                    self.merge_usage(usage);
                }
                if let Some(stop) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(stop.into());
                }
            }
            "message_stop" => {
                if self.completed {
                    return Ok(out);
                }
                if !self.blocks.is_empty() {
                    return Err(failure(
                        "Anthropic stopped with incomplete content",
                        "MALFORMED_RESPONSE",
                    ));
                }
                let reason = match self.stop.as_deref() {
                    Some("tool_use") => FinishReason::ToolCalls,
                    Some("max_tokens" | "model_context_window_exceeded") => FinishReason::MaxTokens,
                    Some("end_turn" | "stop_sequence" | "refusal") => FinishReason::Stop,
                    Some("pause_turn") => {
                        return Err(failure(
                            "Anthropic paused a server-tool turn; server tools are not configured",
                            "UNSUPPORTED_CONTENT",
                        ));
                    }
                    _ => {
                        return Err(failure(
                            "Anthropic stopped without a recognized stop reason",
                            "MALFORMED_RESPONSE",
                        ));
                    }
                };
                self.completed = true;
                out.push(StreamChunk::Usage {
                    usage: self.usage.clone(),
                });
                out.push(StreamChunk::Finish{reason,replay_state:Some(json!({"protocol":"anthropic-messages","content":self.finished.values().collect::<Vec<_>>()}))});
            }
            _ => (),
        }
        Ok(out)
    }
    fn merge_usage(&mut self, value: &Value) {
        if let Some(n) = value.get("input_tokens").and_then(Value::as_u64) {
            self.usage.input_tokens = n;
        }
        if let Some(n) = value.get("output_tokens").and_then(Value::as_u64) {
            self.usage.output_tokens = n;
        }
        if let Some(n) = value.get("cache_read_input_tokens").and_then(Value::as_u64) {
            self.usage.cache_read_tokens = Some(n);
        }
        if let Some(n) = value
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
        {
            self.usage.cache_write_tokens = Some(n);
        }
    }
    pub(crate) fn finish(&self) -> Result<(), LlmFailure> {
        if self.completed {
            Ok(())
        } else {
            Err(failure(
                "Anthropic stream ended before message_stop",
                "STREAM_CLOSED",
            ))
        }
    }
}
fn index(value: &Value) -> Result<u64, LlmFailure> {
    value
        .get("index")
        .and_then(Value::as_u64)
        .ok_or_else(|| failure("Missing Anthropic block index", "MALFORMED_RESPONSE"))
}
fn required(value: &Value, key: &str) -> Result<String, LlmFailure> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| failure(format!("Missing Anthropic {key}"), "MALFORMED_RESPONSE"))
}
fn append(value: &mut Value, key: &str, delta: &str) {
    let mut text = value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    text.push_str(delta);
    value[key] = json!(text);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_system_tools_images_and_parallel_results() {
        let body=request_from_chat(&json!({"model":"claude-sonnet-4-6","reasoning_effort":"medium","messages":[{"role":"system","content":"Be precise"},{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}]},{"role":"assistant","content":"","tool_calls":[{"id":"a","function":{"name":"read","arguments":"{\"path\":\"x\"}"}},{"id":"b","function":{"name":"read","arguments":"{}"}}]},{"role":"tool","tool_call_id":"a","content":"one"},{"role":"tool","tool_call_id":"b","content":"two"}],"tools":[{"type":"function","function":{"name":"read","description":"Read","parameters":{"type":"object"}}}]})).unwrap();
        assert_eq!(body["system"][0]["text"], "Be precise");
        assert_eq!(body["messages"].as_array().unwrap().len(), 3);
        assert_eq!(
            body["messages"][0]["content"][0]["source"]["media_type"],
            "image/png"
        );
        assert_eq!(body["messages"][2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][2]["content"][1]["tool_use_id"], "b");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "medium");
    }
    #[test]
    fn reasoning_levels_and_budgets_follow_model_support() {
        assert!(
            request_from_chat(
                &json!({"model":"claude-sonnet-4-6","messages":[],"reasoning_effort":"xhigh"})
            )
            .is_err()
        );
        let body=request_from_chat(&json!({"model":"claude-3-7-sonnet","messages":[],"reasoning_effort":"high","max_tokens":4096})).unwrap();
        assert_eq!(body["thinking"]["budget_tokens"], 4095);
        assert!(request_from_chat(&json!({"model":"claude-3-7-sonnet","messages":[],"reasoning_effort":"high","max_tokens":1024})).is_err());
    }
    #[test]
    fn preserves_thinking_signatures_streamed_arguments_and_cumulative_usage() {
        let mut stream = AnthropicTranslator::default();
        let mut chunks = Vec::new();
        for event in [
            json!({"type":"message_start","message":{"usage":{"input_tokens":20,"output_tokens":1,"cache_read_input_tokens":50}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Check first"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed-data"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_1","name":"read","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"x\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":12}}),
            json!({"type":"message_stop"}),
        ] {
            chunks.extend(stream.consume(&event.to_string()).unwrap());
        }
        stream.finish().unwrap();
        assert!(chunks.iter().any(|c|matches!(c,StreamChunk::BlockEnd{block:ContentBlock::ToolCall{arguments,..},..} if arguments=="{\"path\":\"x\"}")));
        let Some(StreamChunk::Finish {
            reason: FinishReason::ToolCalls,
            replay_state: Some(state),
        }) = chunks.last()
        else {
            panic!("missing tool finish")
        };
        assert_eq!(state["content"][0]["signature"], "signed-data");
        let body=request_from_chat(&json!({"model":"claude-sonnet-4-6","messages":[{"role":"assistant","anthropic_content":state["content"]},{"role":"tool","tool_call_id":"tool_1","content":"done"}]})).unwrap();
        assert_eq!(
            body["messages"][0]["content"][0]["signature"],
            "signed-data"
        );
        assert!(chunks.iter().any(|c|matches!(c,StreamChunk::Usage{usage} if usage.input_tokens==20&&usage.output_tokens==12&&usage.cache_read_tokens==Some(50))));
    }
    #[test]
    fn truncated_streams_and_provider_errors_fail_loudly() {
        assert!(AnthropicTranslator::default().finish().is_err());
        let failure = AnthropicTranslator::default()
            .consume(r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#)
            .unwrap_err();
        assert_eq!(failure.code, "overloaded_error");
    }
}
