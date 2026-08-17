//! Composition-layer `llm.*` over the real fetch carrier with a stub
//! adapter: provider directory merge, host-scoped catalog, and the
//! discovery failure vocabulary.

use std::sync::Arc;

use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_llm::{
    ChunkStream, GenerateOptions, LlmAdapter, LlmConfigurableProvider, LlmModelInfo, LlmRuntime,
};
use futures::stream;

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

/// A provider route the stub advertises without any directory declaration.
struct StubAdapter;

#[async_trait::async_trait]
impl LlmAdapter for StubAdapter {
    fn provider_info(&self, provider: &str) -> dsh_llm::LlmProviderInfo {
        dsh_llm::LlmProviderInfo {
            id: provider.to_string(),
            name: "Stub Provider".to_string(),
        }
    }

    async fn list_models(&self, _provider: &str) -> Vec<LlmModelInfo> {
        vec![LlmModelInfo {
            provider: "openai".to_string(),
            id: "gpt-x".to_string(),
            name: "GPT X".to_string(),
            description: Some("the stub model".to_string()),
            input_modalities: None,
        }]
    }

    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        Box::pin(stream::empty())
    }
}

struct Harness {
    _ctx: cordis::Context,
    handler: dsh_host_apiproxy::FetchHandler,
}

impl Harness {
    fn new() -> Self {
        let ctx = cordis::Context::root();
        let runtime = LlmRuntime::install(&ctx);
        runtime
            .register_adapter(&ctx, vec!["openai".to_string()], Arc::new(StubAdapter))
            .expect("adapter");
        runtime
            .register_configurable_providers(
                &ctx,
                vec![
                    LlmConfigurableProvider {
                        provider: "openai".to_string(),
                        display_name: "OpenAI".to_string(),
                        settings_ns: "llm-openai".to_string(),
                        settings_path: vec!["providers".to_string(), "openai".to_string()],
                        declared: Some(true),
                    },
                    LlmConfigurableProvider {
                        provider: "dormant".to_string(),
                        display_name: "Dormant".to_string(),
                        settings_ns: "llm-dormant".to_string(),
                        settings_path: vec![],
                        declared: None,
                    },
                ],
            )
            .expect("directory");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self { _ctx: ctx, handler }
    }

    async fn post(&self, method: &str, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": method,
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: format!("/api/{method}"),
                query: vec![],
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Some(body.into_bytes()),
            })
            .await;
        assert_eq!(response.status(), http::StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("unary answers are byte bodies");
        };
        serde_json::from_slice(&bytes).expect("json")
    }
}

#[test]
fn providers_merges_the_directory_with_live_routes() {
    run(async {
        let harness = Harness::new();
        let response = harness.post("llm.providers", serde_json::json!({})).await;
        assert_eq!(response["result"]["ok"], true);
        let providers = response["result"]["value"]["providers"]
            .as_array()
            .expect("providers");
        let by_id = |id: &str| {
            providers
                .iter()
                .find(|entry| entry["provider"] == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        let openai = by_id("openai");
        assert_eq!(openai["displayName"], "OpenAI");
        assert_eq!(openai["settingsNs"], "llm-openai");
        assert_eq!(openai["settingsPath"][0], "providers");
        assert_eq!(openai["active"], true);
        assert_eq!(openai["declared"], true);

        let dormant = by_id("dormant");
        assert_eq!(dormant["active"], false);
    });
}

#[test]
fn models_builds_the_host_scoped_catalog() {
    run(async {
        let harness = Harness::new();
        let response = harness.post("llm.models", serde_json::json!({})).await;
        assert_eq!(response["result"]["ok"], true);
        let groups = response["result"]["value"]["groups"]
            .as_array()
            .expect("groups");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["id"], "openai");
        assert_eq!(groups[0]["name"], "Stub Provider");
        let models = groups[0]["models"].as_array().expect("models");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["id"], "gpt-x");
        assert_eq!(models[0]["description"], "the stub model");
        let failures = response["result"]["value"]["failures"]
            .as_array()
            .expect("failures");
        assert_eq!(failures.len(), 0);
    });
}

#[test]
fn discover_models_without_a_registered_discovery_is_model_discovery_failed() {
    run(async {
        let harness = Harness::new();
        let response = harness
            .post(
                "llm.discoverModels",
                serde_json::json!({ "settingsNs": "llm-openai", "baseURL": "http://x" }),
            )
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(
            response["result"]["error"]["code"],
            "model-discovery-failed"
        );
        assert_eq!(
            response["result"]["error"]["details"]["settingsNs"],
            "llm-openai"
        );
        assert_eq!(
            response["result"]["error"]["details"]["baseURL"],
            "http://x"
        );
    });
}

#[test]
fn a_missing_llm_service_is_internal() {
    run(async {
        let ctx = cordis::Context::root();
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "llm.providers",
            "payload": {},
        }))
        .expect("envelope");
        let response = handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/llm.providers".to_string(),
                query: vec![],
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Some(body.into_bytes()),
            })
            .await;
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(parsed["result"]["ok"], false);
        assert_eq!(parsed["result"]["error"]["code"], "internal");
    });
}
