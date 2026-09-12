use super::*;
use crate::{DeepSeekConfig, ReasoningWireFormat, resolve_adapter_options};
use dsh_llm::{GenerateOptions, reasoning_effort_id};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn endpoint(response: &'static str) -> (String, tokio::task::JoinHandle<Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        let (start, len) = loop {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
            assert!(bytes.len() < 256 * 1024);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]);
                let len = header
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                break (end + 4, len);
            }
        };
        while bytes.len() < start + len {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
        }
        let mut body: Value = serde_json::from_slice(&bytes[start..start + len]).unwrap();
        body["_wireHeaders"] = Value::String(String::from_utf8_lossy(&bytes[..start]).into_owned());
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
        body
    });
    (format!("http://{address}/v1"), task)
}

fn options() -> GenerateOptions {
    GenerateOptions {
        provider: "fixture".into(),
        model: "custom".into(),
        reasoning_effort: Some(reasoning_effort_id("high")),
        max_tokens: Some(4096),
        messages: vec![],
        system: None,
        tools: None,
        temperature: None,
        stop: None,
        signal: None,
        session_id: None,
        purpose: None,
        agent_loop_request: false,
    }
}

#[tokio::test]
async fn all_budget_spellings_and_priority_reach_chat_http_body() {
    for spelling in [
        "thinking_token_budget",
        "thinking_budget",
        "thinking_budget_tokens",
    ] {
        let (url,server)=endpoint("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").await;
        let compat:ProviderCompatibility=serde_json::from_value(json!({"thinkingTokenBudgetField":spelling,"thinkingBudgets":{"high":2048},"supportsReasoningEffort":false,"vllmPriority":-2})).unwrap();
        let connection = resolve_adapter_options(&DeepSeekConfig {
            compat: Some(compat),
            base_url: Some(url),
            keyless: true,
            max_tokens: Some(4096),
            ..Default::default()
        })
        .unwrap();
        let (sender, _receiver) = tokio::sync::mpsc::channel(32);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::request_chunks(
                options(),
                connection,
                String::new(),
                "fixture",
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
        assert_eq!(body[spelling], 2048);
        assert_eq!(body["priority"], -2);
        assert!(body.get("reasoning_effort").is_none());
        for other in [
            "thinking_token_budget",
            "thinking_budget",
            "thinking_budget_tokens",
        ] {
            if other != spelling {
                assert!(body.get(other).is_none());
            }
        }
    }
}

#[tokio::test]
async fn responses_cap_opt_out_reaches_http_body() {
    let (url, server) = endpoint(
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
    )
    .await;
    let connection = resolve_adapter_options(&DeepSeekConfig {
        api: Some("openai-responses".into()),
        compat: Some(ProviderCompatibility {
            supports_max_output_tokens: Some(false),
            ..Default::default()
        }),
        base_url: Some(url),
        keyless: true,
        ..Default::default()
    })
    .unwrap();
    let (sender, _receiver) = tokio::sync::mpsc::channel(32);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::request_chunks(
            options(),
            connection,
            String::new(),
            "fixture",
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
    assert!(body.get("max_output_tokens").is_none());
    assert!(body.get("max_tokens").is_none());
}

#[test]
fn compatibility_rejects_unknown_fields_wrong_protocol_and_non_integer_priority() {
    assert!(
        ProviderCompatibility {
            supports_max_output_tokens: Some(true),
            ..Default::default()
        }
        .validate_endpoint("openai-responses", "https://chatgpt.com/backend-api/codex")
        .is_err()
    );
    assert!(serde_json::from_value::<ProviderCompatibility>(json!({"vllmPriority":null})).is_err());
    assert!(serde_json::from_value::<ProviderCompatibility>(json!({"invented":true})).is_err());
    assert!(serde_json::from_value::<ProviderCompatibility>(json!({"vllmPriority":0.25})).is_err());
    assert!(
        serde_json::from_value::<ProviderCompatibility>(
            json!({"thinkingTokenBudgetField":"arbitrary"})
        )
        .is_err()
    );
    for api in ["anthropic-messages", "openai-responses"] {
        assert!(
            resolve_adapter_options(&DeepSeekConfig {
                api: Some(api.into()),
                compat: Some(ProviderCompatibility {
                    vllm_priority: Some(1),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .is_err()
        );
    }
    assert!(
        ProviderCompatibility {
            supports_max_output_tokens: Some(false),
            ..Default::default()
        }
        .validate("openai-completions")
        .is_err()
    );
}

#[test]
fn explicit_field_overrides_legacy_alias_and_model_overrides_provider() {
    let provider = ProviderCompatibility {
        supports_thinking_token_budget: Some(true),
        vllm_priority: Some(4),
        ..Default::default()
    };
    let model = ProviderCompatibility {
        thinking_token_budget_field: Some(ThinkingTokenBudgetField::ThinkingBudgetTokens),
        vllm_priority: Some(0),
        ..Default::default()
    };
    let merged = provider.merged(Some(&model));
    assert_eq!(
        merged.budget_field(),
        Some(ThinkingTokenBudgetField::ThinkingBudgetTokens)
    );
    assert_eq!(merged.vllm_priority, Some(0));
    let connection = resolve_adapter_options(&DeepSeekConfig {
        compat: Some(merged),
        ..Default::default()
    })
    .unwrap();
    let mut title = json!({"model":"custom","messages":[],"max_tokens":32});
    apply_chat(&mut title, &connection, Some("off")).unwrap();
    assert!(title.get("thinking_budget_tokens").is_none());
}

#[tokio::test]
async fn lite_is_opt_in_and_reaches_the_actual_http_transport() {
    for enabled in [false, true] {
        let (url, server) = endpoint(
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        )
        .await;
        let connection = resolve_adapter_options(&DeepSeekConfig {
            api: Some("openai-responses".into()),
            base_url: Some(url),
            keyless: true,
            compat: Some(ProviderCompatibility {
                use_responses_lite: Some(enabled),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
        let mut request = options();
        request.system = Some("fixture policy".into());
        let (sender, _receiver) = tokio::sync::mpsc::channel(32);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::request_chunks(
                request,
                connection,
                String::new(),
                "fixture",
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
        let headers = body["_wireHeaders"].as_str().unwrap().to_ascii_lowercase();
        assert_eq!(
            headers.contains("x-openai-internal-codex-responses-lite: true"),
            enabled
        );
        if enabled {
            assert!(body.get("instructions").is_none());
            assert!(body.get("tools").is_none());
            assert_eq!(body["input"][0]["type"], "additional_tools");
            assert_eq!(body["input"][1]["role"], "developer");
            assert_eq!(body["input"][1]["content"][0]["text"], "fixture policy");
            assert_eq!(body["reasoning"]["context"], "all_turns");
            assert_eq!(body["parallel_tool_calls"], false);
        } else {
            assert_eq!(body["instructions"], "fixture policy");
            assert!(body["reasoning"].get("context").is_none());
        }
    }
}

#[test]
fn lite_model_override_can_disable_provider_default() {
    let provider = ProviderCompatibility {
        use_responses_lite: Some(true),
        ..Default::default()
    };
    let model = ProviderCompatibility {
        use_responses_lite: Some(false),
        ..Default::default()
    };
    assert_eq!(
        provider.merged(Some(&model)).use_responses_lite,
        Some(false)
    );
    assert!(provider.validate("openai-completions").is_err());
    assert!(
        serde_json::from_value::<ProviderCompatibility>(json!({"useResponsesLite":null})).is_err()
    );
}
