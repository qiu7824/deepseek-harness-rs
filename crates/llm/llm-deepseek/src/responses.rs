use dsh_llm::{LlmFailure, StreamChunk};
use serde_json::{Value, json};

#[cfg(test)]
mod tests {
    use super::request_from_chat;
    use serde_json::json;
    #[test]
    fn only_explicit_commentary_without_tools_requires_a_followup() {
        use dsh_llm::StreamChunk;
        for (phases, expected) in [
            (vec![Some("commentary")], true),
            (vec![Some("commentary"), Some("commentary")], true),
            (vec![Some("final_answer")], false),
            (vec![Some("commentary"), Some("final_answer")], false),
            (vec![Some("commentary"), None], false),
            (vec![None], false),
            (vec![Some("unknown")], false),
            (vec![], false),
        ] {
            let output = phases.iter().enumerate().map(|(index, phase)| {
                let mut item = json!({"id":format!("message-{index}"),"type":"message","role":"assistant","content":[{"type":"output_text","text":"content"}]});
                if let Some(phase) = phase { item["phase"] = json!(phase); }
                item
            }).collect::<Vec<_>>();
            let mut translator = super::ResponsesTranslator::default();
            let chunks = translator
                .consume(
                    &json!({"type":"response.completed","response":{"output":output}}).to_string(),
                )
                .unwrap();
            let state = chunks
                .iter()
                .find_map(|chunk| match chunk {
                    StreamChunk::Finish { replay_state, .. } => replay_state.as_ref(),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                state["continuation"] == "commentary",
                expected,
                "{phases:?}"
            );
            assert_eq!(state["items"], json!(output));
        }
        let mut translator = super::ResponsesTranslator::default();
        let chunks = translator.consume(&json!({"type":"response.completed","response":{"output":[
            {"type":"message","role":"assistant","phase":"commentary","content":[]},
            {"type":"function_call","id":"call-item","call_id":"call","name":"read","arguments":"{}"}
        ]}}).to_string()).unwrap();
        assert!(chunks.iter().any(|chunk| matches!(chunk, StreamChunk::Finish { reason:dsh_llm::FinishReason::ToolCalls, replay_state:Some(state) } if state.get("continuation").is_none())));
    }
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

    #[test]
    fn completion_metadata_stays_local_and_never_changes_request_token_limits() {
        use dsh_llm::{ContentBlock, ModelMessageSource, create_assistant_message};
        for endpoint in [
            "https://api.openai.com/v1",
            "https://chatgpt.com/backend-api/codex",
        ] {
            let source = create_assistant_message(
                vec![ContentBlock::Text {
                    text: "Saved prefix".into(),
                }],
                ModelMessageSource {
                    provider: "openai".into(),
                    model: "test".into(),
                    replay_state: Some(
                        json!({"format":"openai-responses-v1","items":[],"responseStatus":"incomplete","incompleteReason":"max_output_tokens","hasVisibleFinal":true,"truncatedToolCalls":false}),
                    ),
                },
            );
            let chat = json!({"model":"test","max_tokens":32,"messages":[{"role":"assistant","content":"Saved prefix"},{"role":"user","content":"Continue"}]});
            let body =
                super::request_for_endpoint_with_history(&chat, endpoint, &[source], "openai")
                    .unwrap();
            assert_eq!(body["input"][0]["content"][0]["text"], "Saved prefix");
            for key in [
                "responseStatus",
                "incompleteReason",
                "hasVisibleFinal",
                "truncatedToolCalls",
            ] {
                assert!(!body.to_string().contains(key));
            }
            if endpoint.contains("chatgpt.com") {
                assert!(body.get("max_output_tokens").is_none());
            } else {
                assert_eq!(body["max_output_tokens"], 32);
            }
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

/// Mark the stable prefix boundary understood by the Responses API.  The
/// marker is deliberately placed on the first input content block; user
/// turns remain after the boundary, so a changing request cannot invalidate
/// the reusable system/tool prefix.
pub(crate) fn apply_cache_breakpoint(body: &mut Value, endpoint: &str) {
    let Ok(url) = reqwest::Url::parse(endpoint) else { return };
    let official = url.scheme() == "https"
        && (url.host_str() == Some("api.openai.com")
            || (url.host_str() == Some("chatgpt.com")
                && url.path().contains("/backend-api/codex")));
    if !official { return; }
    let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) else { return; };
    let item_index = items.iter().position(|item| item["role"].as_str() == Some("system") || item["role"].as_str() == Some("developer")).or_else(|| items.iter().position(|item| item["role"].is_string()));
    let Some(item) = item_index.and_then(|index| items.get_mut(index)) else { return; };
    if let Some(content) = item.get_mut("content").and_then(Value::as_array_mut)
        && let Some(first) = content.first_mut().and_then(Value::as_object_mut)
    { first.insert("prompt_cache_breakpoint".into(), json!(true)); }
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

#[path = "responses_stream.rs"]
mod stream;
pub(crate) use stream::ResponsesTranslator;
