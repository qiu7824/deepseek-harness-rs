// Tests live beside the private serializer so protocol-specific reasoning
// fields cannot regress while the generic OpenAI route reuses this adapter.

use dsh_llm::{GenerateOptions, ReasoningEffortId};

use crate::{ReasoningWireFormat, RequestDefaults};

#[test]
fn responses_native_screenshots_keep_exact_call_ownership_and_do_not_leak_internal_markers() {
    use dsh_llm::*;
    use serde_json::{Value, json};
    let mut options = request("none");
    let image = |name: &str| ContentBlock::Image {
        attachment: ImageAttachmentRef {
            attachment_id: name.into(),
            media_type: Some("image/png".into()),
            bytes: Some(1),
            width: Some(10),
            height: Some(10),
            name: None,
        },
        offloaded: None,
    };
    let calls = vec![
        ContentBlock::ToolCall {
            id: call_id("native"),
            name: computer_protocol::TOOL_NAME.into(),
            arguments: json!({"actions":[{"type":"screenshot"}],"pendingSafetyChecks":[]})
                .to_string(),
        },
        ContentBlock::ToolCall {
            id: call_id("ordinary"),
            name: "inspect_image".into(),
            arguments: "{}".into(),
        },
    ];
    options.messages = vec![
        create_message(
            Role::Assistant,
            calls,
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ),
        create_tool_result_message(ToolResultMessageInput {
            call_id: call_id("ordinary"),
            is_error: false,
            content: vec![image("other-image")],
        }),
        create_tool_result_message(ToolResultMessageInput {
            call_id: call_id("native"),
            is_error: false,
            content: vec![image("native-image")],
        }),
    ];
    let urls = std::collections::HashMap::from([
        (
            "other-image".into(),
            "data:image/png;base64,T1RIRVI=".into(),
        ),
        (
            "native-image".into(),
            "data:image/png;base64,TkFUSVZF".into(),
        ),
    ]);
    for legacy in [false, true] {
        let mut options = options.clone();
        if legacy {
            options.messages[2] = create_message(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_call_id: call_id("native"),
                    content: vec![image("native-image")],
                    is_error: None,
                }],
                MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
            );
        }
        let chat = super::serialize_responses_request(
            &options,
            &RequestDefaults::default(),
            ReasoningWireFormat::OpenAi,
            Some(&urls),
            None,
            None,
        )
        .unwrap();
        let body =
            crate::responses::request_for_endpoint(&chat, "https://example.test/v1").unwrap();
        let rows = body["input"].as_array().unwrap();
        let output = rows
            .iter()
            .find(|row| row["type"] == "computer_call_output")
            .unwrap();
        assert_eq!(output["call_id"], "native");
        assert_eq!(output["output"]["image_url"], urls["native-image"]);
        assert_eq!(output["output"]["detail"], "original");
        assert!(
            rows.iter()
                .any(|row| row["role"] == "user" && row.to_string().contains(&urls["other-image"]))
        );
        assert!(!rows.iter().any(|row|row["role"]=="user" && row.to_string().contains(&urls["native-image"])));
        assert!(!body.to_string().contains("_dsh_native"));
        let generic = super::serialize_request_with_prepared_images(
            &options,
            &RequestDefaults::default(),
            ReasoningWireFormat::OpenAi,
            Some(&urls),
            None,
            None,
        )
        .unwrap();
        assert!(!generic.to_string().contains("_dsh_native"));
        for replacement in [
            json!([]),
            json!([{"type":"text","text":"no frame"}]),
            json!([{"type":"image_url","image_url":{"url":urls["native-image"]}},{"type":"image_url","image_url":{"url":urls["other-image"]}}]),
        ] {
            let mut broken = chat.clone();
            let tool = broken["messages"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|row| row["tool_call_id"] == "native")
                .unwrap();
            tool["_dsh_native_output"] = replacement;
            assert_eq!(
                crate::responses::request_for_endpoint(&broken, "https://example.test/v1")
                    .unwrap_err()
                    .code,
                "NATIVE_COMPUTER_SCREENSHOT_REQUIRED"
            );
        }
        let mut failed = chat.clone();
        failed["messages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["tool_call_id"] == "native")
            .unwrap()["_dsh_native_error"] = Value::Bool(true);
        let recovered =
            crate::responses::request_for_endpoint(&failed, "https://example.test/v1").unwrap();
        assert!(
            !recovered["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["type"] == "computer_call" || row["type"] == "computer_call_output")
        );
        assert!(recovered.to_string().contains("effects may be partial"));
        assert!(!recovered.to_string().contains(&urls["native-image"]));
    }
    let mut offloaded = options.clone();
    offloaded.messages[2] = create_tool_result_message(ToolResultMessageInput {
        call_id: call_id("native"),
        is_error: false,
        content: vec![ContentBlock::Image {
            attachment: ImageAttachmentRef {
                attachment_id: "native-image".into(),
                media_type: Some("image/png".into()),
                bytes: Some(1),
                width: Some(10),
                height: Some(10),
                name: None,
            },
            offloaded: Some(true),
        }],
    });
    let before = serde_json::to_value(&offloaded.messages).unwrap();
    let chat = super::serialize_responses_request(
        &offloaded,
        &RequestDefaults::default(),
        ReasoningWireFormat::OpenAi,
        Some(&urls),
        None,
        None,
    )
    .unwrap();
    let body = crate::responses::request_for_endpoint(&chat, "https://example.test/v1").unwrap();
    assert!(body.to_string().contains("screenshot offloaded"));
    assert!(!body.to_string().contains(&urls["native-image"]));
    assert_eq!(serde_json::to_value(&offloaded.messages).unwrap(), before);
}

fn request(effort: &str) -> GenerateOptions {
    GenerateOptions {
        provider: "provider".to_string(),
        model: "model".to_string(),
        reasoning_effort: Some(ReasoningEffortId::new(effort)),
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
        telemetry: None,
    }
}

#[test]
fn openai_xhigh_uses_only_reasoning_effort() {
    let value = super::serialize_request_with_prepared_images(
        &request("xhigh"),
        &RequestDefaults::default(),
        ReasoningWireFormat::OpenAi,
        None,
        None,
        None,
    )
    .expect("serialize OpenAI request");
    assert_eq!(value["reasoning_effort"], "xhigh");
    assert!(value.get("thinking").is_none());
}

#[test]
fn deepseek_max_keeps_native_thinking_fields() {
    let value = super::serialize_request_with_prepared_images(
        &request("max"),
        &RequestDefaults::default(),
        ReasoningWireFormat::DeepSeek,
        None,
        None,
        None,
    )
    .expect("serialize DeepSeek request");
    assert_eq!(value["reasoning_effort"], "max");
    assert_eq!(value["thinking"]["type"], "enabled");
}

#[test]
fn deepseek_rejects_xhigh() {
    let failure = super::serialize_request_with_prepared_images(
        &request("xhigh"),
        &RequestDefaults::default(),
        ReasoningWireFormat::DeepSeek,
        None,
        None,
        None,
    )
    .expect_err("DeepSeek must reject an OpenAI-only effort");
    assert_eq!(failure.code, "UNSUPPORTED_REASONING_EFFORT");
}
