use dsh_llm::{LlmFailure, StreamChunk};
use serde_json::{Value, json};

#[cfg(test)]
mod tests {
    use super::request_from_chat;
    use serde_json::json;
    #[test]
    fn native_computer_replay_matches_actions_and_preserves_original_items() {
        use dsh_llm::{ContentBlock, ModelMessageSource, StreamChunk, create_assistant_message};
        let endpoint = "https://example.test/v1";
        let native = json!({"type":"computer_call","id":"native-item","call_id":"call-1","actions":[{"type":"screenshot"}],"status":"completed"});
        let mut parser = super::ResponsesTranslator::default();
        parser.enable_native_computer();
        let chunks=parser.consume(&json!({"type":"response.completed","response":{"status":"completed","output":[native.clone()]}}).to_string()).unwrap();
        let block = chunks
            .iter()
            .find_map(|chunk| match chunk {
                StreamChunk::BlockEnd {
                    block: ContentBlock::ToolCall { .. },
                    ..
                } => {
                    if let StreamChunk::BlockEnd { block, .. } = chunk {
                        Some(block.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .unwrap();
        let mut finish = chunks
            .into_iter()
            .find(|chunk| matches!(chunk, StreamChunk::Finish { .. }))
            .unwrap();
        super::bind_replay_metadata(&mut finish, endpoint, "model");
        let StreamChunk::Finish { replay_state, .. } = finish else {
            unreachable!()
        };
        let source = create_assistant_message(
            vec![block.clone()],
            ModelMessageSource {
                provider: "p".into(),
                model: "model".into(),
                replay_state,
            },
        );
        let ContentBlock::ToolCall { arguments, .. } = block else {
            unreachable!()
        };
        let mut chat = json!({"model":"model","tools":[{"type":"function","function":{"name":"computer_native","parameters":{"type":"object","title":"dsh-native-computer-v1"}}}],"messages":[{"role":"assistant","content":"","tool_calls":[{"id":"call-1","function":{"name":"computer_native","arguments":arguments}}]},{"role":"tool","tool_call_id":"call-1","_dsh_native_output":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}]});
        let body = super::request_for_endpoint_with_history(
            &chat,
            endpoint,
            std::slice::from_ref(&source),
            "p",
        )
        .unwrap();
        assert_eq!(body["tools"], json!([{"type":"computer"}]));
        assert_eq!(body["input"][0], native);
        assert_eq!(body["input"][1]["type"], "computer_call_output");
        let mut failed = chat.clone();
        failed["messages"][1]["_dsh_native_error"] = json!(true);
        failed["messages"][1]["_dsh_native_output"] =
            json!([{"type":"text","text":"Action cancelled; partial effects possible"}]);
        let recovered = super::request_for_endpoint_with_history(
            &failed,
            endpoint,
            std::slice::from_ref(&source),
            "p",
        )
        .unwrap();
        assert_eq!(recovered["tools"], json!([{"type":"computer"}]));
        assert!(
            !recovered["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "computer_call"
                    || item["type"] == "computer_call_output")
        );
        assert!(recovered.to_string().contains("Action cancelled"));
        assert!(!recovered.to_string().contains("native-item"));
        chat["messages"][0]["tool_calls"][0]["function"]["arguments"] =
            json!(json!({"actions":[{"type":"wait"}],"pendingSafetyChecks":[]}).to_string());
        let edited = super::request_for_endpoint_with_history(
            &chat,
            endpoint,
            std::slice::from_ref(&source),
            "p",
        )
        .unwrap();
        assert!(edited["input"][0].get("id").is_none());
        assert_eq!(edited["input"][0]["actions"], json!([{"type":"wait"}]));
        chat["messages"][0]["tool_calls"][0]["function"]["arguments"] = json!(
            json!({"actions":[{"type":"wait"}],"pendingSafetyChecks":[{"id":"check"}]}).to_string()
        );
        assert_eq!(
            super::request_for_endpoint_with_history(&chat, endpoint, &[source], "p")
                .unwrap_err()
                .code,
            "NATIVE_COMPUTER_SAFETY_CHECK_REQUIRED"
        );
    }
    #[test]
    fn failed_native_history_does_not_remove_the_next_fresh_native_pair() {
        let call=|id:&str|json!({"role":"assistant","tool_calls":[{"id":id,"function":{"name":"computer_native","arguments":json!({"actions":[{"type":"screenshot"}],"pendingSafetyChecks":[]}).to_string()}}]});
        let body=request_from_chat(&json!({"model":"fixture","messages":[call("failed"),{"role":"tool","tool_call_id":"failed","_dsh_native_error":true,"_dsh_native_output":[{"type":"text","text":"Cancelled before screenshot"}]},call("fresh"),{"role":"tool","tool_call_id":"fresh","_dsh_native_output":[{"type":"image_url","image_url":{"url":"data:image/png;base64,TkVX"}}]}]})).unwrap();
        let rows=body["input"].as_array().unwrap();
        assert_eq!(rows.iter().filter(|row|row["type"]=="computer_call").count(),1);
        assert_eq!(rows.iter().find(|row|row["type"]=="computer_call").unwrap()["call_id"],"fresh");
        assert_eq!(rows.iter().find(|row|row["type"]=="computer_call_output").unwrap()["call_id"],"fresh");
        assert!(body.to_string().contains("Cancelled before screenshot"));
        assert!(!body.to_string().contains("_dsh_native"));
    }
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
    fn cache_breakpoint_serializes_as_object_after_complete_stable_instructions() {
        for endpoint in [
            "https://api.openai.com/v1",
            "https://api.openai.com/v1/responses",
        ] {
            let chat = json!({"model":"gpt-6-astra","messages":[
                {"role":"system","content":"Stable instructions"},
                {"role":"developer","content":[{"type":"text","text":"Shared rules"},{"type":"text","text":"More shared rules"}]},
                {"role":"user","content":"First question"}
            ]});
            let mut first = super::request_for_endpoint(&chat, endpoint).unwrap();
            super::apply_session_cache_key(&mut first, Some("session-cache"), endpoint);
            super::apply_cache_breakpoint(&mut first, endpoint);
            let first: serde_json::Value =
                serde_json::from_slice(&serde_json::to_vec(&first).unwrap()).unwrap();
            assert_eq!(first["instructions"], "");
            assert_eq!(
                first["input"][0]["content"][0]["text"],
                "Stable instructions"
            );
            assert!(
                first["input"][0]["content"][0]
                    .get("prompt_cache_breakpoint")
                    .is_none()
            );
            assert!(
                first["input"][1]["content"][0]
                    .get("prompt_cache_breakpoint")
                    .is_none()
            );
            assert_eq!(
                first["input"][1]["content"][1]["prompt_cache_breakpoint"],
                json!({"mode":"explicit"})
            );
            assert!(
                first["input"][2]["content"][0]
                    .get("prompt_cache_breakpoint")
                    .is_none()
            );
            let mut changed = chat.clone();
            changed["messages"][2]["content"] = json!("Different question");
            let mut changed = super::request_for_endpoint(&changed, endpoint).unwrap();
            super::apply_session_cache_key(&mut changed, Some("session-cache"), endpoint);
            super::apply_cache_breakpoint(&mut changed, endpoint);
            assert_eq!(first["prompt_cache_key"], changed["prompt_cache_key"]);
            assert_eq!(
                &first["input"].as_array().unwrap()[..2],
                &changed["input"].as_array().unwrap()[..2]
            );
            let unchanged = changed.clone();
            super::apply_cache_breakpoint(&mut changed, endpoint);
            assert_eq!(
                unchanged, changed,
                "applying cache policy twice must not duplicate instructions"
            );
        }
    }
    #[test]
    fn cache_breakpoints_preserve_legacy_and_third_party_contracts() {
        for (model, endpoint) in [
            ("gpt-5.5", "https://api.openai.com/v1"),
            ("gpt-5.4", "https://chatgpt.com/backend-api/codex"),
            ("gpt-5.3-codex", "https://chatgpt.com/backend-api/codex"),
            ("gpt-5.6-sol", "https://chatgpt.com/backend-api/codex"),
            ("gpt-6-astra", "https://chatgpt.com/backend-api/codex"),
            (
                "gpt-6-astra",
                "https://chatgpt.com/backend-api/codex/responses",
            ),
            ("o3", "https://api.openai.com/v1"),
            ("custom-alias", "https://api.openai.com/v1"),
            ("gpt-6-astra", "https://example.test/v1"),
            ("gpt-6-astra", "https://chatgpt.com/other/backend-api/codex"),
            ("gpt-6-astra", "https://chatgpt.com/backend-api/codex-else"),
        ] {
            let mut body = json!({"model":model,"instructions":"Instructions","input":[{"role":"user","content":[{"type":"input_text","text":"Question"}]}]});
            let before = body.clone();
            super::apply_cache_breakpoint(&mut body, endpoint);
            assert_eq!(before, body, "{model} {endpoint}");
        }
        for model in ["gpt-5.6", "gpt-5.6-sol", "gpt-6-astra"] {
            let mut body = json!({"model":model,"input":[{"role":"user","content":[{"type":"input_text","text":"Changing user input"}]}]});
            let before = body.clone();
            super::apply_cache_breakpoint(&mut body, "https://api.openai.com/v1");
            assert_eq!(before, body, "no user/assistant fallback breakpoint");
        }
    }
    #[test]
    fn account_refresh_keeps_cache_and_replay_but_account_switch_does_not() {
        use dsh_llm::{ContentBlock, ModelMessageSource, StreamChunk, create_assistant_message};
        let endpoint = "https://chatgpt.com/backend-api/codex";
        let headers = |account: &str, token: &str| {
            vec![
                ("ChatGPT-Account-Id".to_string(), account.to_string()),
                ("Authorization".to_string(), token.to_string()),
            ]
        };
        let alice = super::account_scope_hash(
            &headers("shared-workspace", "token-v1"),
            Some("alice-scope"),
        )
        .unwrap();
        let renewed = super::account_scope_hash(
            &headers("shared-workspace", "token-v2"),
            Some("alice-scope"),
        )
        .unwrap();
        let bob =
            super::account_scope_hash(&headers("shared-workspace", "token-v1"), Some("bob-scope"))
                .unwrap();
        assert_eq!(alice, renewed);
        assert_ne!(alice, bob);
        let mut keys = vec![];
        for scope in [&alice, &renewed, &bob] {
            let mut body = json!({});
            super::apply_session_cache_key_for_account(
                &mut body,
                Some("same-session"),
                endpoint,
                Some(scope),
            );
            keys.push(body["prompt_cache_key"].clone());
        }
        assert_eq!(keys[0], keys[1]);
        assert_ne!(keys[0], keys[2]);
        let mut translator = super::ResponsesTranslator::default();
        let mut finish = translator.consume(&json!({"type":"response.completed","response":{"output":[
            {"type":"reasoning","id":"rs-fixture","encrypted_content":"opaque-alice","summary":[]},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}
        ]}}).to_string()).unwrap().into_iter().find(|chunk|matches!(chunk,StreamChunk::Finish{..})).unwrap();
        super::bind_replay_metadata_for_account(&mut finish, endpoint, "gpt-6-astra", Some(&alice));
        let StreamChunk::Finish { replay_state, .. } = finish else {
            unreachable!()
        };
        let history = [create_assistant_message(
            vec![ContentBlock::Text {
                text: "Done".into(),
            }],
            ModelMessageSource {
                provider: "openai-codex".into(),
                model: "gpt-6-astra".into(),
                replay_state,
            },
        )];
        let chat = json!({"model":"gpt-6-astra","messages":[{"role":"assistant","content":"Done"},{"role":"user","content":"Continue"}]});
        for (scope, expected) in [
            (Some(renewed.as_str()), true),
            (Some(bob.as_str()), false),
            (None, false),
        ] {
            let body = super::request_for_endpoint_with_history_for_account(
                &chat,
                endpoint,
                &history,
                "openai-codex",
                scope,
            )
            .unwrap();
            assert_eq!(
                body["input"][0]["encrypted_content"] == "opaque-alice",
                expected
            );
            assert!(
                body.to_string().contains("Done"),
                "visible conversation survives switching"
            );
        }
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
        offload_images: None,
        message: message.into(),
        code: code.to_string(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

#[cfg(test)]
pub(crate) fn apply_session_cache_key(body: &mut Value, session_id: Option<&str>, endpoint: &str) {
    apply_session_cache_key_for_account(body, session_id, endpoint, None);
}

pub(crate) fn account_scope_hash(
    headers: &[(String, String)],
    login_scope: Option<&str>,
) -> Option<String> {
    use sha2::{Digest, Sha256};
    if let Some(scope) = login_scope.filter(|scope| !scope.is_empty()) {
        return Some(format!(
            "{:x}",
            Sha256::digest(format!("dsh-login-v2\0{scope}").as_bytes())
        ));
    }
    headers
        .iter()
        .find(|(name, value)| name.eq_ignore_ascii_case("chatgpt-account-id") && !value.is_empty())
        .map(|(_, value)| format!("{:x}", Sha256::digest(value.as_bytes())))
}

pub(crate) fn apply_session_cache_key_for_account(
    body: &mut Value,
    session_id: Option<&str>,
    endpoint: &str,
    account_scope: Option<&str>,
) {
    if !official_responses_endpoint(endpoint) || body.get("prompt_cache_key").is_some() {
        return;
    }
    let Some(session) = session_id.filter(|id| !id.is_empty()) else {
        return;
    };
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    if let Some(scope) = account_scope {
        digest.update(b"dsh-account-cache-v1\0");
        digest.update(scope.as_bytes());
        digest.update(b"\0");
    }
    digest.update(session.as_bytes());
    let hash = format!("{:x}", digest.finalize());
    body["prompt_cache_key"] = json!(format!("dsh-{}", &hash[..48]));
}

fn official_responses_endpoint(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && match url.host_str() {
                Some("api.openai.com") => matches!(
                    url.path().trim_end_matches('/'),
                    "" | "/v1" | "/v1/responses"
                ),
                Some("chatgpt.com") => matches!(
                    url.path().trim_end_matches('/'),
                    "/backend-api/codex" | "/backend-api/codex/responses"
                ),
                _ => false,
            }
    })
}

/// Explicit cache breakpoints were introduced with GPT-5.6. Unknown aliases
/// and earlier models keep their endpoint's default implicit caching.
fn supports_cache_breakpoints(model: &str) -> bool {
    let Some(version) = model.strip_prefix("gpt-").and_then(|s| s.split('-').next()) else {
        return false;
    };
    let mut parts = version.split('.');
    let (Ok(major), Some(minor)) = (
        parts.next().unwrap_or("").parse::<u32>(),
        parts.next().map(str::parse::<u32>),
    ) else {
        return version.parse::<u32>().is_ok_and(|major| major >= 6);
    };
    parts.next().is_none() && minor.is_ok_and(|minor| major >= 6 || major == 5 && minor >= 6)
}

/// Cache the end of the initial instruction prefix, using the documented
/// object shape. Leave implicit caching enabled for subsequent tool rounds.
pub(crate) fn apply_cache_breakpoint(body: &mut Value, endpoint: &str) {
    if !official_responses_endpoint(endpoint)
        // The subscription service accepts the same wire envelope but does
        // not enable explicit breakpoints, including on GPT-6 Astra. It keeps
        // implicit caching and the stable prompt_cache_key instead.
        || !reqwest::Url::parse(endpoint).is_ok_and(|url| url.host_str() == Some("api.openai.com"))
        || !body.get("model").and_then(Value::as_str).is_some_and(supports_cache_breakpoints)
    {
        return;
    }
    // Top-level instructions cannot carry a content breakpoint. Represent
    // the same instructions once as the leading developer message instead.
    // Do not duplicate the text in both instructions and the input prefix.
    let instructions = body
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    if !body.get("input").is_some_and(Value::is_array) {
        return;
    }
    if let Some(instructions) = instructions {
        body["instructions"] = json!("");
        body["input"].as_array_mut().unwrap().insert(
            0,
            json!({
                "role":"developer", "content":[{"type":"input_text", "text":instructions}]
            }),
        );
    }
    let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    let boundary = items
        .iter()
        .take_while(|item| matches!(item["role"].as_str(), Some("system" | "developer")))
        .enumerate()
        .filter_map(|(index, item)| {
            item["content"].as_array().and_then(|parts| {
                parts
                    .iter()
                    .rposition(|part| part["type"] == "input_text")
                    .map(|part| (index, part))
            })
        })
        .last();
    if let Some((item, part)) = boundary {
        items[item]["content"][part]
            .as_object_mut()
            .unwrap()
            .entry("prompt_cache_breakpoint")
            .or_insert_with(|| json!({"mode":"explicit"}));
    }
}

#[cfg(test)]
pub(crate) fn request_from_chat(chat: &Value) -> Result<Value, LlmFailure> {
    request_from_chat_with_history(chat, &[], "", "", None)
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
            Some("reasoning" | "message" | "function_call" | "computer_call")
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
        .filter(|item| item["type"] == "function_call" || item["type"] == "computer_call")
        .map(|item| {
            if item["type"] == "computer_call" {
                return dsh_llm::computer_protocol::parse_call(item)
                    .ok()
                    .map(|(id, args)| {
                        (
                            Some(id),
                            Some(dsh_llm::computer_protocol::TOOL_NAME.to_string()),
                            Some(args.to_string()),
                        )
                    });
            }
            Some((
                item["call_id"].as_str().map(str::to_owned),
                item["name"].as_str().map(str::to_owned),
                item["arguments"].as_str().map(str::to_owned),
            ))
        });
    let chat_calls = message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|call| {
            Some((
                call["id"].as_str().map(str::to_owned),
                call["function"]["name"].as_str().map(str::to_owned),
                call["function"]["arguments"].as_str().map(str::to_owned),
            ))
        });
    output_calls.eq(chat_calls)
}
#[cfg(test)]
pub(crate) fn bind_replay_metadata(chunk: &mut StreamChunk, endpoint: &str, model: &str) {
    bind_replay_metadata_for_account(chunk, endpoint, model, None);
}

pub(crate) fn bind_replay_metadata_for_account(
    chunk: &mut StreamChunk,
    endpoint: &str,
    model: &str,
    account_scope: Option<&str>,
) {
    if let StreamChunk::Finish {
        replay_state: Some(state),
        ..
    } = chunk
    {
        if state["format"] == "openai-responses-v1" {
            state["endpointHash"] = json!(endpoint_key(endpoint));
            state["requestedModel"] = json!(model);
            if let Some(scope) = account_scope {
                state["accountScopeHash"] = json!(scope);
            }
        }
    }
}
fn request_from_chat_with_history(
    chat: &Value,
    history: &[dsh_llm::Message],
    provider: &str,
    endpoint: &str,
    account_scope: Option<&str>,
) -> Result<Value, LlmFailure> {
    let model = chat.get("model").cloned().unwrap_or(Value::Null);
    let mut input = Vec::new();
    let mut instructions = Vec::new();
    let mut native_calls = std::collections::HashSet::new();
    let mut pending_safety_calls = std::collections::HashSet::new();
    // A completed failed call, or a durably offloaded historical screenshot,
    // is represented as historical data with its paired call removed. Never
    // fabricate a computer_call_output or reuse another call's pixels.
    let retired_outputs = chat["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| {
            message["role"] == "tool"
                && message["_dsh_native_output"]
                    .as_array()
                    .is_some_and(|parts| {
                        let images = parts.iter().filter(|p| p["type"] == "image_url").count();
                        images <= 1
                            && (message["_dsh_native_error"] == true
                                || message["_dsh_native_image_offloaded"] == true && images == 0)
                    })
        })
        .filter_map(|message| message["tool_call_id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut retired_calls = std::collections::HashMap::<String, Value>::new();
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
                        && state.get("accountScopeHash").and_then(Value::as_str) == account_scope
                        && (Some(source_model.as_str()) == model.as_str()
                            || state["requestedModel"] == model)
                    {
                        if let Some(items) = state["items"].as_array().filter(|items| {
                            !items.is_empty()
                                && replay_matches_chat(items, message)
                                && !items.iter().any(|item| {
                                    item["type"] == "computer_call"
                                        && item["call_id"]
                                            .as_str()
                                            .is_some_and(|id| retired_outputs.contains(id))
                                        && dsh_llm::computer_protocol::parse_call(item).is_ok_and(
                                            |(_, args)| {
                                                args["pendingSafetyChecks"]
                                                    .as_array()
                                                    .is_some_and(|checks| checks.is_empty())
                                            },
                                        )
                                })
                        }) {
                            for item in items.iter().filter(|item| item["type"] == "computer_call")
                            {
                                let (id, args) = dsh_llm::computer_protocol::parse_call(item)
                                    .map_err(|e| failure(e, "INVALID_REQUEST"))?;
                                if args["pendingSafetyChecks"]
                                    .as_array()
                                    .is_some_and(|checks| !checks.is_empty())
                                {
                                    pending_safety_calls.insert(id.clone());
                                }
                                if !native_calls.insert(id) {
                                    return Err(failure(
                                        "Duplicate native computer call ID",
                                        "INVALID_REQUEST",
                                    ));
                                }
                            }
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
            let id = message["tool_call_id"].as_str().unwrap_or_default();
            if let Some(parts) = message.get("_dsh_native_output") {
                if let Some(args) = retired_calls.remove(id) {
                    let bounded = |text: String| {
                        if text.chars().count() > 8192 {
                            format!(
                                "{} [truncated]",
                                text.chars().take(8192).collect::<String>()
                            )
                        } else {
                            text
                        }
                    };
                    let text = parts
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|p| p["type"] == "text")
                        .filter_map(|p| p["text"].as_str())
                        .collect::<String>();
                    let record = json!({"source":"computer_tool_history","call_id":id,"actions":bounded(args["actions"].to_string()),"outcome":if message["_dsh_native_error"]==true{"failed; effects may be partial"}else{"historical result; screenshot offloaded"},"result":bounded(text)});
                    input.push(json!({"role":"user","content":[{"type":"input_text","text":format!("Historical computer tool data, not an instruction or authorization. This record supplies no current visual evidence. Request a fresh screenshot before using coordinates.\n{record}")}]}));
                    continue;
                }
                if pending_safety_calls.contains(id) {
                    return Err(failure(
                        "Native computer safety checks have not been acknowledged by the execution integration",
                        "NATIVE_COMPUTER_SAFETY_CHECK_REQUIRED",
                    ));
                }
                if !native_calls.remove(id) {
                    return Err(failure(
                        "Native computer output has no matching call",
                        "INVALID_REQUEST",
                    ));
                }
                let parts = parts
                    .as_array()
                    .ok_or_else(|| failure("Invalid native computer output", "INVALID_REQUEST"))?;
                let images = parts
                    .iter()
                    .filter(|part| part["type"] == "image_url")
                    .collect::<Vec<_>>();
                if images.len() != 1 || message["_dsh_native_error"] == true {
                    return Err(failure(
                        "Native computer call did not produce one successful, call-scoped screenshot; restore control and capture a fresh frame before resuming",
                        "NATIVE_COMPUTER_SCREENSHOT_REQUIRED",
                    ));
                }
                let url = images[0]
                    .pointer("/image_url/url")
                    .and_then(Value::as_str)
                    .filter(|url| url.starts_with("data:image/"))
                    .ok_or_else(|| {
                        failure(
                            "Native computer screenshot must be a prepared image",
                            "INVALID_REQUEST",
                        )
                    })?;
                input.push(json!({"type":"computer_call_output","call_id":id,"output":{"type":"computer_screenshot","image_url":url,"detail":"original"}}));
                let text = parts
                    .iter()
                    .filter(|part| part["type"] == "text")
                    .filter_map(|part| part["text"].as_str())
                    .collect::<String>();
                if !text.is_empty() {
                    input.push(json!({"role":"user","content":[{"type":"input_text","text":format!("Computer tool observation for {id}:\n{text}")}]}));
                }
                continue;
            }
            if native_calls.contains(id) {
                return Err(failure(
                    "Native computer output is missing its call-scoped screenshot",
                    "NATIVE_COMPUTER_SCREENSHOT_REQUIRED",
                ));
            }
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
            if call.pointer("/function/name").and_then(Value::as_str)
                == Some(dsh_llm::computer_protocol::TOOL_NAME)
            {
                let id = call["id"].as_str().unwrap_or_default();
                let args: Value =
                    serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or(""))
                        .map_err(|e| failure(e.to_string(), "INVALID_REQUEST"))?;
                if !args.as_object().is_some_and(|args| {
                    args.keys()
                        .all(|key| matches!(key.as_str(), "actions" | "pendingSafetyChecks"))
                }) {
                    return Err(failure(
                        "Invalid native computer arguments",
                        "INVALID_REQUEST",
                    ));
                }
                let item = json!({"type":"computer_call","call_id":id,"actions":args["actions"],"pending_safety_checks":args.get("pendingSafetyChecks").cloned().unwrap_or_else(||json!([])),"status":"completed"});
                dsh_llm::computer_protocol::parse_call(&item)
                    .map_err(|e| failure(e, "INVALID_REQUEST"))?;
                if retired_outputs.contains(id)
                    && item["pending_safety_checks"]
                        .as_array()
                        .is_some_and(|checks| checks.is_empty())
                {
                    retired_calls.insert(id.to_string(), args);
                    continue;
                }
                if item["pending_safety_checks"]
                    .as_array()
                    .is_some_and(|checks| !checks.is_empty())
                {
                    pending_safety_calls.insert(id.to_string());
                }
                if !native_calls.insert(id.to_string()) {
                    return Err(failure(
                        "Duplicate native computer call ID",
                        "INVALID_REQUEST",
                    ));
                }
                input.push(item);
                continue;
            }
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
        if function["name"]==dsh_llm::computer_protocol::TOOL_NAME && function["parameters"]["title"]=="dsh-native-computer-v1" { return Some(json!({"type":"computer"})); }
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
#[cfg(test)]
pub(crate) fn request_for_endpoint(chat: &Value, base_url: &str) -> Result<Value, LlmFailure> {
    request_for_endpoint_with_history(chat, base_url, &[], "")
}
#[cfg(test)]
pub(crate) fn request_for_endpoint_with_history(
    chat: &Value,
    base_url: &str,
    history: &[dsh_llm::Message],
    provider: &str,
) -> Result<Value, LlmFailure> {
    request_for_endpoint_with_history_for_account(chat, base_url, history, provider, None)
}

pub(crate) fn request_for_endpoint_with_history_for_account(
    chat: &Value,
    base_url: &str,
    history: &[dsh_llm::Message],
    provider: &str,
    account_scope: Option<&str>,
) -> Result<Value, LlmFailure> {
    let mut body =
        request_from_chat_with_history(chat, history, provider, base_url, account_scope)?;
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

/// Responses Lite keeps tools and changing instructions in the input history.
/// It uses the normal Responses stream and the existing scoped replay path.
pub(crate) fn apply_lite(body: &mut Value) -> Result<(), LlmFailure> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| failure("Responses body must be an object", "INVALID_REQUEST"))?;
    let tools = object.remove("tools").unwrap_or_else(|| json!([]));
    let instructions = object.remove("instructions");
    let input = object
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| failure("Responses Lite requires input items", "INVALID_REQUEST"))?;
    if let Some(text) = instructions
        .as_ref()
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        input.insert(0, json!({"type":"message","role":"developer","content":[{"type":"input_text","text":text}]}));
    }
    input.insert(
        0,
        json!({"type":"additional_tools","role":"developer","tools":tools}),
    );
    for item in input {
        for key in ["content", "output"] {
            if let Some(parts) = item.get_mut(key).and_then(Value::as_array_mut) {
                for part in parts {
                    if part["type"] == "input_image" {
                        part.as_object_mut().unwrap().remove("detail");
                    }
                }
            }
        }
    }
    let reasoning = object
        .entry("reasoning")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| failure("Responses reasoning must be an object", "INVALID_REQUEST"))?;
    reasoning.insert("context".into(), json!("all_turns"));
    object.insert("parallel_tool_calls".into(), json!(false));
    object.insert("store".into(), json!(false));
    object.insert("include".into(), json!(["reasoning.encrypted_content"]));
    Ok(())
}

#[cfg(test)]
mod lite_tests {
    use super::*;
    #[test]
    fn tools_instructions_images_and_reasoning_use_lite_shape() {
        let original = json!({"instructions":"policy","tools":[{"type":"function","name":"read"}],
            "input":[{"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,AA==","detail":"high"}]},
                {"type":"reasoning","encrypted_content":"opaque"},{"role":"developer","content":[{"type":"input_text","text":"changed policy"}]}],
            "reasoning":{"effort":"high"},"parallel_tool_calls":true});
        let mut body = original.clone();
        apply_lite(&mut body).unwrap();
        assert!(body.get("tools").is_none());
        assert!(body.get("instructions").is_none());
        assert_eq!(body["input"][0]["type"], "additional_tools");
        assert_eq!(body["input"][1]["content"][0]["text"], "policy");
        assert!(body["input"][2]["content"][0].get("detail").is_none());
        assert_eq!(body["input"][3]["encrypted_content"], "opaque");
        assert_eq!(body["input"][4]["content"][0]["text"], "changed policy");
        assert_eq!(
            body["reasoning"],
            json!({"effort":"high","context":"all_turns"})
        );
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(original["input"][0]["content"][0]["detail"], "high");
    }
}
