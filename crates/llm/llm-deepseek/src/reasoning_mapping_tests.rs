use dsh_llm::{GenerateOptions, reasoning_effort_id};

use crate::{
    CatalogReasoningEffort, DeepSeekCatalogModel, DeepSeekConfig, ReasoningWireFormat,
    apply_model_max_tokens, map_reasoning_effort_for_request, resolve_adapter_options,
};

fn request(effort: &str) -> GenerateOptions {
    GenerateOptions {
        provider: "gpt".to_string(),
        model: "gpt-5.6-sol".to_string(),
        reasoning_effort: Some(reasoning_effort_id(effort)),
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

fn connection() -> crate::ResolvedDeepSeekOptions {
    resolve_adapter_options(&DeepSeekConfig {
        models: Some(vec![DeepSeekCatalogModel {
            id: "gpt-5.6-sol".to_string(),
            name: None,
            description: None,
            context_window: None,
            max_tokens: None,
            reasoning_efforts: Some(vec![CatalogReasoningEffort {
                id: "max".to_string(),
                name: "Extra High".to_string(),
                wire: "xhigh".to_string(),
            }]),
            image_input: Some(true),
        }]),
        ..DeepSeekConfig::default()
    })
    .expect("valid adapter options")
}

#[test]
fn stable_max_maps_to_openai_xhigh() {
    let mut options = request("max");
    map_reasoning_effort_for_request(&mut options, &connection(), ReasoningWireFormat::OpenAi)
        .expect("map effort");
    assert_eq!(
        options.reasoning_effort.as_ref().map(|id| id.as_str()),
        Some("xhigh")
    );
}

#[test]
fn deepseek_wire_keeps_stable_id() {
    let mut options = request("max");
    map_reasoning_effort_for_request(&mut options, &connection(), ReasoningWireFormat::DeepSeek)
        .expect("keep native effort");
    assert_eq!(
        options.reasoning_effort.as_ref().map(|id| id.as_str()),
        Some("max")
    );
}

#[test]
fn glm_flash_uses_model_limit_for_legacy_request_value() {
    let mut options = request("max");
    options.model = "glm-5.3-flash".to_string();
    options.max_tokens = Some(256_000);
    apply_model_max_tokens(&mut options, &connection());
    assert_eq!(options.max_tokens, Some(131_072));
}

#[test]
fn unknown_model_keeps_its_request_value() {
    let mut options = request("max");
    options.model = "private-preview".to_string();
    options.max_tokens = Some(256_000);
    apply_model_max_tokens(&mut options, &connection());
    assert_eq!(options.max_tokens, Some(256_000));
}

#[test]
fn unknown_catalog_effort_is_rejected_before_transport() {
    let mut options = request("high");
    let failure =
        map_reasoning_effort_for_request(&mut options, &connection(), ReasoningWireFormat::OpenAi)
            .expect_err("unknown effort must reject");
    assert_eq!(failure.code, "UNSUPPORTED_REASONING_EFFORT");
}
