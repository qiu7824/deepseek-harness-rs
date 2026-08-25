// Tests live beside the private serializer so protocol-specific reasoning
// fields cannot regress while the generic OpenAI route reuses this adapter.

use dsh_llm::{GenerateOptions, ReasoningEffortId};

use crate::{ReasoningWireFormat, RequestDefaults};

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
