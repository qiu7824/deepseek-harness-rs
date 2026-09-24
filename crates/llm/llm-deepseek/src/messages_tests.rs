use super::*;
use crate::{DeepSeekConfig, resolve_adapter_options};
use dsh_llm::{call_id, create_message, reasoning_effort_id};

fn options() -> GenerateOptions {
    GenerateOptions {
        provider: crate::PROVIDER.into(),
        model: "deepseek-flash".into(),
        reasoning_effort: None,
        messages: vec![],
        system: None,
        tools: None,
        temperature: None,
        max_tokens: Some(128),
        stop: None,
        signal: None,
        session_id: Some("messages-test".into()),
        purpose: None,
        agent_loop_request: false,
        telemetry: None,
    }
}
fn message(role: Role, content: Vec<ContentBlock>) -> Message {
    create_message(
        role,
        content,
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    )
}
fn text(role: Role, text: &str) -> Message {
    message(role, vec![ContentBlock::Text { text: text.into() }])
}
fn connection() -> ResolvedDeepSeekOptions {
    resolve_adapter_options(&DeepSeekConfig::default()).unwrap()
}
fn body(options: &GenerateOptions) -> Value {
    serialize(options, &connection(), None, None, None).unwrap()
}

#[test]
fn native_tool_results_keep_correlation_errors_and_offloaded_images() {
    let mut request = options();
    request.messages = vec![
        text(Role::User, "inspect"),
        message(Role::Assistant, vec![ContentBlock::ToolCall { id: call_id("native-1"), name: "read_image".into(), arguments: "{}".into() }]),
        dsh_llm::create_tool_result_message(dsh_llm::ToolResultMessageInput {
            call_id: call_id("native-1"), is_error: true,
            content: vec![serde_json::from_value(json!({"type":"image","attachment":{"attachmentId":"old-image"},"offloaded":true})).unwrap()],
        }),
    ];
    let native = body(&request);
    let result = &native["messages"][2]["content"][0];
    assert_eq!(result["tool_use_id"], "native-1");
    assert_eq!(result["is_error"], true);
    assert_eq!(result["content"][0]["type"], "text");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("image omitted")
    );
    let chat = crate::serialize::serialize_request_with_prepared_images(
        &request,
        &crate::RequestDefaults::default(),
        crate::ReasoningWireFormat::OpenAi,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(chat["messages"][2]["role"], "tool");
    assert_eq!(chat["messages"][2]["tool_call_id"], "native-1");
    assert!(
        chat["messages"][2]["content"]
            .as_str()
            .unwrap()
            .contains("image omitted")
    );
    assert!(
        crate::request_image_attachments(&request).is_empty(),
        "offloaded images must not trigger file reads or uploads"
    );
}

#[test]
fn reserved_and_unprojected_blocks_fail_explicitly_on_both_provider_routes() {
    for block in [
        json!({"type":"tool-addition","toolName":"read"}),
        json!({"type":"plugin:future","payload":"retain"}),
        json!({"type":"file","attachment":{"attachmentId":"file-1","name":"report.pdf","bytes":5}}),
    ] {
        let mut request = options();
        request.messages = vec![message(
            Role::User,
            vec![serde_json::from_value(block.clone()).unwrap()],
        )];
        assert_eq!(
            serialize(&request, &connection(), None, None, None)
                .unwrap_err()
                .code,
            "UNSUPPORTED_CONTENT"
        );
        assert_eq!(
            crate::serialize::serialize_request_with_prepared_images(
                &request,
                &crate::RequestDefaults::default(),
                crate::ReasoningWireFormat::OpenAi,
                None,
                None,
                None
            )
            .unwrap_err()
            .code,
            "UNSUPPORTED_CONTENT"
        );
        assert_eq!(
            serde_json::to_value(&request.messages[0].content[0]).unwrap(),
            block
        );
    }
}

#[test]
fn official_defaults_move_to_messages_and_explicit_custom_routes_survive() {
    for base in [
        "https://api.deepseek.com?token=invalid",
        "https://api.deepseek.com/#fragment",
        "https://user:password@api.deepseek.com",
    ] {
        assert!(
            resolve_adapter_options(&DeepSeekConfig {
                api: Some(API.into()),
                base_url: Some(base.into()),
                ..Default::default()
            })
            .is_err()
        );
    }
    let defaults = connection();
    assert_eq!(defaults.api, API);
    assert_eq!(
        crate::anthropic_transport::endpoint(&defaults.base_url, "messages"),
        "https://api.deepseek.com/anthropic/v1/messages"
    );
    for base in [
        "https://api.deepseek.com",
        "https://api.deepseek.com/v1/chat/completions",
    ] {
        let resolved = resolve_adapter_options(&DeepSeekConfig {
            base_url: Some(base.into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(resolved.api, API);
        assert_eq!(resolved.base_url, crate::PUBLIC_BASE_URL);
        let explicit = resolve_adapter_options(&DeepSeekConfig {
            base_url: Some(base.into()),
            api: Some("openai-completions".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(explicit.api, "openai-completions");
        assert_eq!(explicit.base_url, base);
    }
    for base in [
        "http://127.0.0.1:1234/v1",
        "https://api.deepseek.com.attacker.test",
        "https://gateway.test/v1",
    ] {
        let resolved = resolve_adapter_options(&DeepSeekConfig {
            base_url: Some(base.into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(resolved.api, "openai-completions");
        assert_eq!(resolved.base_url, base);
    }
}

#[test]
fn historical_bad_tool_inputs_degrade_without_mutating_messages_or_losing_results() {
    for arguments in [
        "{broken",
        "null",
        "[]",
        "7",
        "\"string\"",
        "{\"path\":\"x\"}",
    ] {
        let mut request = options();
        request.messages = vec![
            text(Role::User, "read"),
            message(
                Role::Assistant,
                vec![ContentBlock::ToolCall {
                    id: call_id("c1"),
                    name: "read".into(),
                    arguments: arguments.into(),
                }],
            ),
            message(
                Role::User,
                vec![
                    ContentBlock::Text {
                        text: "continue".into(),
                    },
                    ContentBlock::ToolResult {
                        tool_call_id: call_id("c1"),
                        content: vec![],
                        is_error: Some(true),
                    },
                ],
            ),
        ];
        let before = serde_json::to_value(&request.messages).unwrap();
        let projected = body(&request);
        let expected = if arguments.starts_with("{\"path") {
            json!({"path":"x"})
        } else {
            json!({})
        };
        assert_eq!(projected["messages"][1]["content"][0]["input"], expected);
        assert_eq!(
            projected["messages"][2]["content"][0]["type"],
            "tool_result"
        );
        assert_eq!(projected["messages"][2]["content"][0]["is_error"], true);
        assert_eq!(serde_json::to_value(&request.messages).unwrap(), before);
        // The generic Messages route uses the same history-only policy.
        let chat = json!({"model":"claude","messages":[{"role":"assistant","tool_calls":[{"id":"c1","function":{"name":"read","arguments":arguments}}]}]});
        assert_eq!(
            crate::anthropic::request_from_chat(&chat).unwrap()["messages"][0]["content"][0]["input"],
            expected
        );
    }
}

#[test]
fn system_updates_are_positioned_after_user_input_and_effort_uses_messages_fields() {
    let mut request = options();
    request.system = Some("base".into());
    request.messages = vec![
        text(Role::System, "initial"),
        text(Role::User, "one"),
        text(Role::Assistant, "ok"),
        text(Role::System, "updated"),
        text(Role::User, "two"),
    ];
    let projected = body(&request);
    assert_eq!(projected["system"], "base\n\ninitial");
    assert_eq!(projected["messages"][2]["role"], "user");
    assert_eq!(
        projected["messages"][3],
        json!({"role":"system","content":[{"type":"text","text":"updated"}]})
    );
    assert_eq!(projected["output_config"]["effort"], "high");
    assert!(projected.get("reasoning_effort").is_none());
    for effort in ["off", "low", "high", "max"] {
        request.reasoning_effort = Some(reasoning_effort_id(effort));
        assert_eq!(
            body(&request)["thinking"]["type"],
            if effort == "off" {
                "disabled"
            } else {
                "enabled"
            }
        );
    }
    request.reasoning_effort = Some(reasoning_effort_id("xhigh"));
    assert!(serialize(&request, &connection(), None, None, None).is_err());
    request.purpose = Some("session-title".into());
    assert_eq!(body(&request)["thinking"]["type"], "disabled");
}

#[test]
fn images_stay_inside_their_tool_results_for_inline_and_files_requests() {
    let image: ContentBlock = serde_json::from_value(json!({"type":"image","attachment":{"type":"image","attachmentId":"image-one","mediaType":"image/png","width":1,"height":1}})).unwrap();
    let mut request = options();
    request.messages = vec![
        message(
            Role::Assistant,
            vec![ContentBlock::ToolCall {
                id: call_id("c1"),
                name: "render".into(),
                arguments: "{}".into(),
            }],
        ),
        message(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_call_id: call_id("c1"),
                content: vec![image],
                is_error: None,
            }],
        ),
    ];
    let urls = HashMap::from([("image-one".into(), "data:image/png;base64,AQID".into())]);
    let files = HashMap::from([("image-one".into(), "file_one".into())]);
    let inline = serialize(&request, &connection(), Some(&urls), None, None).unwrap();
    assert_eq!(
        inline["messages"][1]["content"][0]["content"][0]["source"],
        json!({"type":"base64","media_type":"image/png","data":"AQID"})
    );
    let uploaded = serialize(&request, &connection(), None, Some(&files), None).unwrap();
    assert_eq!(
        uploaded["messages"][1]["content"][0]["content"][0]["source"],
        json!({"type":"file","file_id":"file_one"})
    );
    request.messages.pop();
    assert_eq!(
        serialize(&request, &connection(), None, None, None)
            .unwrap_err()
            .code,
        "INVALID_REQUEST"
    );
}

#[test]
fn live_streamed_tool_inputs_remain_strict() {
    for input in [json!([]), json!(null), json!(7)] {
        let mut translator =
            crate::anthropic::AnthropicTranslator::new("deepseek-flash", crate::PUBLIC_BASE_URL);
        translator.set_protocol(API);
        translator
            .consume(&json!({"type":"message_start","message":{"usage":{}}}).to_string())
            .unwrap();
        translator.consume(&json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c1","name":"read","input":input}}).to_string()).unwrap();
        translator
            .consume(&json!({"type":"content_block_stop","index":0}).to_string())
            .unwrap();
        translator
            .consume(
                &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}).to_string(),
            )
            .unwrap();
        assert_eq!(
            translator
                .consume(&json!({"type":"message_stop"}).to_string())
                .unwrap_err()
                .code,
            "MALFORMED_RESPONSE"
        );
    }
}

#[test]
fn token_limited_partial_tool_input_is_pruned_before_execution() {
    let mut translator =
        crate::anthropic::AnthropicTranslator::new("deepseek-flash", crate::PUBLIC_BASE_URL);
    translator.set_protocol(API);
    let mut assembler = dsh_llm::BlockAssembler::new();
    for event in [
        json!({"type":"message_start","message":{"usage":{}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Saved prefix"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"c1","name":"write","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}}),
        json!({"type":"message_stop"}),
    ] {
        for chunk in translator.consume(&event.to_string()).unwrap() {
            assembler.push(&chunk);
        }
    }
    assert_eq!(assembler.finish(), dsh_llm::FinishReason::MaxTokens);
    assert_eq!(
        assembler.blocks(),
        vec![ContentBlock::Text {
            text: "Saved prefix".into()
        }]
    );
}

#[test]
fn messages_reject_out_of_order_frames_and_invalid_usage() {
    for event in [
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"x"}}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_start","message":{"usage":{"input_tokens":-1}}}),
    ] {
        let mut translator =
            crate::anthropic::AnthropicTranslator::new("deepseek-flash", crate::PUBLIC_BASE_URL);
        translator.set_protocol(API);
        assert_eq!(
            translator.consume(&event.to_string()).unwrap_err().code,
            "MALFORMED_RESPONSE"
        );
    }
}
