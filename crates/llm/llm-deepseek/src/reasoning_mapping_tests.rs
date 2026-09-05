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
            reasoning_default: None,
            api: None,
            supports_reasoning_summaries: None,
            supported_parameters: None,
            id: "gpt-5.6-sol".to_string(),
            enabled: None,
            name: None,
            description: None,
            context_window: None,
            max_tokens: None,
            reasoning_efforts: Some(vec![CatalogReasoningEffort {
                description: None,
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

#[tokio::test]
async fn catalog_protocol_and_parameter_capabilities_reach_the_actual_http_body() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        let (header_end, length) = loop {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
            assert!(bytes.len() < 128 * 1024);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                break (end + 4, length);
            }
        };
        while bytes.len() < header_end + length {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
        }
        assert!(
            String::from_utf8_lossy(&bytes[..header_end])
                .starts_with("POST /v1/responses HTTP/1.1")
        );
        let body: serde_json::Value =
            serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
        let response =
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).as_bytes()).await.unwrap();
        body
    });
    let mut connection = connection();
    connection.base_url = format!("http://{address}/v1");
    connection.keyless = true;
    let model = &mut connection.models[0];
    model.api = Some("openai-responses".into());
    model.supported_parameters = Some(vec!["reasoning".into(), "max_output_tokens".into()]);
    model.supports_reasoning_summaries = Some(false);
    model.reasoning_efforts = Some(
        ["xhigh", "max"]
            .into_iter()
            .map(|id| CatalogReasoningEffort {
                id: id.into(),
                wire: id.into(),
                name: id.into(),
                description: None,
            })
            .collect(),
    );
    let mut options = request("max");
    options.temperature = Some(0.7);
    options.stop = Some(vec!["STOP".into()]);
    options.tools = Some(vec![dsh_llm::ToolSchema {
        name: "glob".into(),
        description: "List files".into(),
        parameters: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
    }]);
    let (sender, _receiver) = tokio::sync::mpsc::channel(8);
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::request_chunks(
            options,
            connection,
            String::new(),
            "Fixture",
            ReasoningWireFormat::OpenAi,
            None,
            &sender,
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let body = server.await.unwrap();
    assert_eq!(body["reasoning"]["effort"], "max");
    assert!(body["reasoning"].get("summary").is_none());
    assert!(body.get("temperature").is_none());
    assert!(body.get("stop").is_none());
    assert_eq!(body["tools"][0]["name"], "glob");
    assert_eq!(body["stream"], true);
}
