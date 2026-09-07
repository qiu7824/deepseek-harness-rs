use std::collections::BTreeMap;

use dsh_llm::{ContentBlock, FinishReason, LlmFailure, StreamChunk, TokenUsage, call_id};
use serde_json::{Value, json};

#[cfg(test)]
mod tests {
    use super::request_from_chat;
    use serde_json::json;
    #[test]
    fn response_usage_cache_buckets_do_not_double_count_prompt_tokens() {
        let mut translator = super::ResponsesTranslator::default();
        let chunks=translator.consume(&json!({"type":"response.completed","response":{"usage":{"input_tokens":1000,"output_tokens":20,"input_tokens_details":{"cached_tokens":800,"cache_write_tokens":50}}}}).to_string()).unwrap();
        let usage = chunks
            .into_iter()
            .find_map(|chunk| match chunk {
                dsh_llm::StreamChunk::Usage { usage } => Some(usage),
                _ => None,
            })
            .unwrap();
        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.cache_read_tokens, Some(800));
        assert_eq!(usage.cache_write_tokens, Some(50));
        assert_eq!(
            usage.input_tokens
                + usage.cache_read_tokens.unwrap()
                + usage.cache_write_tokens.unwrap(),
            1000
        );
    }
    #[test]
    fn encrypted_reasoning_is_replayed_only_on_its_provider_model_and_endpoint() {
        use dsh_llm::{ContentBlock, ModelMessageSource, StreamChunk, create_assistant_message};
        let endpoint = "https://chatgpt.com/backend-api/codex";
        let item = json!({"type":"reasoning","id":"rs_fixture","summary":[],"encrypted_content":"opaque-fixture"});
        let mut translator = super::ResponsesTranslator::default();
        let output_message = json!({"id":"msg_fixture","type":"message","role":"assistant","phase":"final_answer","status":"completed","content":[{"type":"output_text","text":"done","annotations":[]}]});
        let mut finish=translator.consume(&json!({"type":"response.completed","response":{"model":"gpt-test-snapshot","output":[item,output_message]}}).to_string()).unwrap().into_iter().find(|chunk|matches!(chunk,StreamChunk::Finish{..})).unwrap();
        super::bind_replay_metadata(&mut finish, endpoint, "gpt-test");
        let StreamChunk::Finish { replay_state, .. } = finish else {
            unreachable!()
        };
        let source = create_assistant_message(
            vec![ContentBlock::Text {
                text: "done".into(),
            }],
            ModelMessageSource {
                provider: "configured-openai".into(),
                model: "gpt-test-snapshot".into(),
                replay_state,
            },
        );
        let chat = json!({"model":"gpt-test","messages":[{"role":"assistant","content":"done"},{"role":"user","content":"continue"}]});
        let body = super::request_for_endpoint_with_history(
            &chat,
            endpoint,
            std::slice::from_ref(&source),
            "configured-openai",
        )
        .unwrap();
        assert_eq!(body["input"][0]["encrypted_content"], "opaque-fixture");
        assert_eq!(body["input"][1]["role"], "assistant");
        assert_eq!(body["input"][2]["role"], "user");
        assert_eq!(body["input"][1]["id"], "msg_fixture");
        assert_eq!(body["input"][1]["phase"], "final_answer");
        let mut edited = chat.clone();
        edited["messages"][0]["content"] = json!("edited visible history");
        let edited = super::request_for_endpoint_with_history(
            &edited,
            endpoint,
            std::slice::from_ref(&source),
            "configured-openai",
        )
        .unwrap();
        assert_eq!(
            edited["input"].as_array().unwrap().len(),
            2,
            "edited history must not replay stale opaque content"
        );
        for (endpoint, provider, model) in [
            ("https://api.other.test/v1", "configured-openai", "gpt-test"),
            (endpoint, "other-provider", "gpt-test"),
            (endpoint, "configured-openai", "other-model"),
        ] {
            let mut chat = chat.clone();
            chat["model"] = json!(model);
            let body = super::request_for_endpoint_with_history(
                &chat,
                endpoint,
                std::slice::from_ref(&source),
                provider,
            )
            .unwrap();
            assert_eq!(body["input"].as_array().unwrap().len(), 2);
        }
    }
    #[test]
    fn official_response_cache_routing_is_stable_and_session_scoped() {
        let mut first = json!({});
        let mut retry = json!({});
        let mut other = json!({});
        super::apply_session_cache_key(
            &mut first,
            Some("session-1"),
            "https://chatgpt.com/backend-api/codex",
        );
        super::apply_session_cache_key(
            &mut retry,
            Some("session-1"),
            "https://chatgpt.com/backend-api/codex",
        );
        super::apply_session_cache_key(
            &mut other,
            Some("session-2"),
            "https://chatgpt.com/backend-api/codex",
        );
        assert_eq!(first, retry);
        assert_ne!(first, other);
        assert!(first["prompt_cache_key"].as_str().unwrap().len() < 64);
        let mut custom = json!({});
        super::apply_session_cache_key(&mut custom, Some("session-1"), "https://custom.example/v1");
        assert!(custom.get("prompt_cache_key").is_none());
    }
    #[test]
    fn terminal_output_recovers_coalesced_text_and_calls_and_preserves_phase() {
        use dsh_llm::{ContentBlock, ModelMessageSource, StreamChunk, create_assistant_message};
        let endpoint = "https://api.openai.com/v1";
        let output = json!([
            {"id":"msg_commentary","type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"Checking the file.","annotations":[]}]},
            {"id":"reason","type":"reasoning","summary":[{"type":"summary_text","text":"First summary"},{"type":"summary_text","text":"Second summary"}],"encrypted_content":"opaque-fixture"},
            {"id":"fn_item","type":"function_call","call_id":"call_1","name":"read","arguments":"{\"path\":\"notes.txt\"}"}
        ]);
        let mut translator = super::ResponsesTranslator::default();
        let mut chunks=translator.consume(&json!({"type":"response.completed","response":{"model":"gpt-test","output":output}}).to_string()).unwrap();
        assert!(chunks.iter().any(|chunk|matches!(chunk,StreamChunk::BlockEnd{block:ContentBlock::Text{text},..}if text=="Checking the file.")));
        assert!(chunks.iter().any(|chunk|matches!(chunk,StreamChunk::BlockEnd{block:ContentBlock::Reasoning{text},..}if text=="First summary\n\nSecond summary")));
        assert!(chunks.iter().any(|chunk|matches!(chunk,StreamChunk::BlockEnd{block:ContentBlock::ToolCall{name,..},..}if name=="read")));
        let finish = chunks
            .iter_mut()
            .find(|chunk| matches!(chunk, StreamChunk::Finish { .. }))
            .unwrap();
        super::bind_replay_metadata(finish, endpoint, "gpt-test");
        let StreamChunk::Finish { replay_state, .. } = finish else {
            unreachable!()
        };
        let source = create_assistant_message(
            vec![],
            ModelMessageSource {
                provider: "openai".into(),
                model: "gpt-test".into(),
                replay_state: replay_state.clone(),
            },
        );
        let chat = json!({"model":"gpt-test","messages":[{"role":"assistant","content":"Checking the file.","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"notes.txt\"}"}}]},{"role":"tool","tool_call_id":"call_1","content":"file contents"}]});
        let body =
            super::request_for_endpoint_with_history(&chat, endpoint, &[source], "openai").unwrap();
        let items = body["input"].as_array().unwrap();
        assert_eq!(&items[..3], output.as_array().unwrap().as_slice());
        assert_eq!(items.len(), 4);
        assert_eq!(items[3]["type"], "function_call_output");
    }
    #[test]
    fn codex_rejects_unresolved_ultra_without_changing_custom_endpoints() {
        let request = json!({"model":"gpt-6-astra","messages":[],"reasoning_effort":"ultra"});
        assert!(
            super::request_for_endpoint(&request, "https://chatgpt.com/backend-api/codex").is_err()
        );
        assert_eq!(
            super::request_for_endpoint(&request, "https://custom.example/v1").unwrap()["reasoning"]
                ["effort"],
            "ultra"
        );
        let request = json!({"model":"gpt-6-astra","messages":[],"reasoning_effort":"max"});
        let body =
            super::request_for_endpoint(&request, "https://chatgpt.com/backend-api/codex").unwrap();
        assert_eq!(body["reasoning"]["effort"], "max");
        assert!(body.get("executionMode").is_none());
    }

    #[test]
    fn maps_openai_reasoning_effort_to_responses_shape() {
        let request = request_from_chat(&json!({
            "model": "gpt-test",
            "messages": [],
            "reasoning_effort": "xhigh"
        }))
        .expect("convert chat request");
        assert_eq!(request["reasoning"]["effort"], "xhigh");
        assert_eq!(request["reasoning"]["summary"], "auto");
        assert!(request.get("reasoning_effort").is_none());
    }

    #[test]
    fn codex_subscription_body_is_stateless_and_keeps_public_api_contract_separate() {
        let chat = json!({"model":"gpt-test","messages":[],"max_tokens":8192});
        let codex =
            super::request_for_endpoint(&chat, "https://chatgpt.com/backend-api/codex/").unwrap();
        assert_eq!(codex["store"], false);
        assert_eq!(codex["instructions"], "");
        assert!(codex.get("max_output_tokens").is_none());
        assert_eq!(codex["stream"], true);
        for endpoint in [
            "https://api.openai.com/v1",
            "https://chatgpt.com.attacker.invalid/backend-api/codex",
            "https://chatgpt.com/v1",
        ] {
            let body = super::request_for_endpoint(&chat, endpoint).unwrap();
            assert!(body.get("store").is_none());
            assert_eq!(body["max_output_tokens"], 8192);
        }
    }
}

fn failure(message: impl Into<String>, code: &str) -> LlmFailure {
    LlmFailure {
        message: message.into(),
        code: code.to_string(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

pub(crate) fn apply_session_cache_key(body: &mut Value, session_id: Option<&str>, endpoint: &str) {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return;
    };
    let official = url.scheme() == "https"
        && (url.host_str() == Some("api.openai.com")
            || (url.host_str() == Some("chatgpt.com")
                && url
                    .path()
                    .trim_end_matches('/')
                    .ends_with("/backend-api/codex")));
    if !official || body.get("prompt_cache_key").is_some() {
        return;
    }
    let Some(session) = session_id.filter(|id| !id.is_empty()) else {
        return;
    };
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(session.as_bytes()));
    body["prompt_cache_key"] = json!(format!("dsh-{}", &hash[..48]));
}

pub(crate) fn request_from_chat(chat: &Value) -> Result<Value, LlmFailure> {
    request_from_chat_with_history(chat, &[], "", "")
}

fn endpoint_key(endpoint: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(endpoint.trim_end_matches('/').as_bytes())
    )
}
fn replay_matches_chat(items: &[Value], message: &Value) -> bool {
    if items.iter().any(|item| {
        !matches!(
            item["type"].as_str(),
            Some("reasoning" | "message" | "function_call")
        ) || (item["type"] == "message" && item["role"] != "assistant")
    }) {
        return false;
    }
    let output_text = items
        .iter()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter_map(|part| match part["type"].as_str() {
            Some("output_text") => part["text"].as_str(),
            Some("refusal") => part["refusal"].as_str(),
            _ => None,
        })
        .flat_map(str::bytes);
    let chat_text = message["content"]
        .as_str()
        .into_iter()
        .chain(
            message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|part| part["type"] == "text")
                .filter_map(|part| part["text"].as_str()),
        )
        .flat_map(str::bytes);
    if !output_text.eq(chat_text) {
        return false;
    }
    let output_calls = items
        .iter()
        .filter(|item| item["type"] == "function_call")
        .map(|item| {
            (
                item["call_id"].as_str(),
                item["name"].as_str(),
                item["arguments"].as_str(),
            )
        });
    let chat_calls = message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|call| {
            (
                call["id"].as_str(),
                call["function"]["name"].as_str(),
                call["function"]["arguments"].as_str(),
            )
        });
    output_calls.eq(chat_calls)
}
pub(crate) fn bind_replay_metadata(chunk: &mut StreamChunk, endpoint: &str, model: &str) {
    if let StreamChunk::Finish {
        replay_state: Some(state),
        ..
    } = chunk
    {
        if state["format"] == "openai-responses-v1" {
            state["endpointHash"] = json!(endpoint_key(endpoint));
            state["requestedModel"] = json!(model);
        }
    }
}
fn request_from_chat_with_history(
    chat: &Value,
    history: &[dsh_llm::Message],
    provider: &str,
    endpoint: &str,
) -> Result<Value, LlmFailure> {
    let model = chat.get("model").cloned().unwrap_or(Value::Null);
    let mut input = Vec::new();
    let mut instructions = Vec::new();
    let mut assistants = history
        .iter()
        .filter(|message| message.role == dsh_llm::Role::Assistant);
    let endpoint_hash = endpoint_key(endpoint);
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
        if role == "assistant" {
            if let Some(source) = assistants.next() {
                if let dsh_llm::MessageSource::Model {
                    provider: source_provider,
                    model: source_model,
                    replay_state: Some(state),
                } = &source.source
                {
                    if source_provider == provider
                        && state["format"] == "openai-responses-v1"
                        && state["endpointHash"] == endpoint_hash
                        && (Some(source_model.as_str()) == model.as_str()
                            || state["requestedModel"] == model)
                    {
                        if let Some(items) = state["items"].as_array().filter(|items| {
                            !items.is_empty() && replay_matches_chat(items, message)
                        }) {
                            input.extend(items.iter().cloned());
                            continue;
                        }
                    }
                }
            }
        }
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
    if let Some(effort) = chat
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .or_else(|| chat.pointer("/thinking/effort").and_then(Value::as_str))
    {
        body["reasoning"] = json!({"effort":effort, "summary":"auto"});
    }
    Ok(body)
}

/// Codex subscription requests have a stricter contract than the public API.
pub(crate) fn request_for_endpoint(chat: &Value, base_url: &str) -> Result<Value, LlmFailure> {
    request_for_endpoint_with_history(chat, base_url, &[], "")
}
pub(crate) fn request_for_endpoint_with_history(
    chat: &Value,
    base_url: &str,
    history: &[dsh_llm::Message],
    provider: &str,
) -> Result<Value, LlmFailure> {
    let mut body = request_from_chat_with_history(chat, history, provider, base_url)?;
    if reqwest::Url::parse(base_url).ok().is_some_and(|url| {
        url.scheme() == "https" && matches!(url.host_str(), Some("api.openai.com" | "chatgpt.com"))
    }) {
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    if reqwest::Url::parse(base_url).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("chatgpt.com")
            && url.path().trim_end_matches('/') == "/backend-api/codex"
    }) {
        if let Some(effort) = body.pointer("/reasoning/effort").and_then(Value::as_str) {
            if !["none", "minimal", "low", "medium", "high", "xhigh", "max"].contains(&effort) {
                return Err(failure(
                    "Codex execution modes must be resolved before sending a reasoning effort",
                    "UNSUPPORTED_REASONING_EFFORT",
                ));
            }
        }
        body["store"] = json!(false);
        if body.get("instructions").is_none() {
            body["instructions"] = json!("");
        }
        body.as_object_mut()
            .expect("request object")
            .remove("max_output_tokens");
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
    replay_items: Vec<Value>,
}

impl ResponsesTranslator {
    pub(crate) fn completed(&self) -> bool {
        self.completed
    }

    pub(crate) fn consume(&mut self, payload: &str) -> Result<Vec<StreamChunk>, LlmFailure> {
        if self.completed {
            return Ok(Vec::new());
        }
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
            "response.output_item.done" => {
                if let Some(item) = event.get("item").filter(|item| item["type"].is_string()) {
                    self.replay_items.push(item.clone());
                }
            }
            "response.completed" => {
                let mut items = std::mem::take(&mut self.replay_items);
                if let Some(output) = event
                    .pointer("/response/output")
                    .and_then(Value::as_array)
                    .filter(|items| !items.is_empty())
                {
                    items = output.clone();
                }
                // Terminal output is authoritative even if an intermediary coalesces
                // deltas. Keep assistant phases and complete tool arguments in replay.
                let text_parts = items
                    .iter()
                    .filter(|item| item["type"] == "message" && item["role"] == "assistant")
                    .flat_map(|item| item["content"].as_array().into_iter().flatten())
                    .filter_map(|part| match part["type"].as_str() {
                        Some("output_text") => part["text"].as_str(),
                        Some("refusal") => part["refusal"].as_str(),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if !text_parts.is_empty() {
                    let text = text_parts.concat();
                    if let Some((_, old)) = &mut self.text {
                        *old = text
                    } else {
                        let index = self.next_index;
                        self.next_index += 1;
                        out.push(StreamChunk::BlockStart {
                            index,
                            block_type: "text".into(),
                        });
                        self.text = Some((index, text));
                    }
                }
                let summaries = items
                    .iter()
                    .filter(|item| item["type"] == "reasoning")
                    .flat_map(|item| item["summary"].as_array().into_iter().flatten())
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>();
                if !summaries.is_empty() {
                    let text = summaries.join("\n\n");
                    if let Some((_, old)) = &mut self.reasoning {
                        *old = text
                    } else {
                        let index = self.next_index;
                        self.next_index += 1;
                        out.push(StreamChunk::BlockStart {
                            index,
                            block_type: "reasoning".into(),
                        });
                        self.reasoning = Some((index, text));
                    }
                }
                for item in items.iter().filter(|item| item["type"] == "function_call") {
                    let (Some(call), Some(name), Some(arguments)) = (
                        item["call_id"].as_str(),
                        item["name"].as_str(),
                        item["arguments"].as_str(),
                    ) else {
                        continue;
                    };
                    if let Some((_, _, old_name, old_arguments)) =
                        self.tools.values_mut().find(|(_, id, _, _)| id == call)
                    {
                        *old_name = name.into();
                        *old_arguments = arguments.into();
                    } else {
                        let index = self.next_index;
                        self.next_index += 1;
                        out.push(StreamChunk::BlockStart {
                            index,
                            block_type: "tool-call".into(),
                        });
                        self.tools.insert(
                            item["id"].as_str().unwrap_or(call).into(),
                            (index, call.into(), name.into(), arguments.into()),
                        );
                    }
                }
                let has_tool_calls = !self.tools.is_empty();
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
                    let cache_read = usage
                        .pointer("/input_tokens_details/cached_tokens")
                        .and_then(Value::as_u64);
                    let cache_write = usage
                        .pointer("/input_tokens_details/cache_write_tokens")
                        .and_then(Value::as_u64);
                    out.push(StreamChunk::Usage {
                        usage: TokenUsage {
                            input_tokens: usage
                                .get("input_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0)
                                .saturating_sub(cache_read.unwrap_or(0))
                                .saturating_sub(cache_write.unwrap_or(0)),
                            output_tokens: usage
                                .get("output_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                            cache_read_tokens: cache_read,
                            cache_write_tokens: cache_write,
                            reasoning_tokens: usage
                                .pointer("/output_tokens_details/reasoning_tokens")
                                .and_then(Value::as_u64),
                        },
                    });
                }
                self.completed = true;
                let mut replay_state = json!({"format":"openai-responses-v1","items":items,"usageAccounting":"disjoint"});
                if let Some(model) = event
                    .pointer("/response/model")
                    .and_then(Value::as_str)
                    .filter(|model| !model.is_empty())
                {
                    replay_state["responseModel"] = json!(model);
                }
                out.push(StreamChunk::Finish {
                    reason: if has_tool_calls {
                        FinishReason::ToolCalls
                    } else {
                        FinishReason::Stop
                    },
                    replay_state: Some(replay_state),
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
