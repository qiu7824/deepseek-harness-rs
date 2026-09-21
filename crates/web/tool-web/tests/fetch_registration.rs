use std::sync::Arc;

use async_trait::async_trait;
use cordis::Context;
use dsh_tool_web::{Config, apply};
use dsh_web::{WebError, WebFetch, WebFetchBody, WebFetchRequest, WebFetchResult};

struct StubWeb;

#[async_trait]
impl dsh_web::WebSearch for StubWeb {
    async fn search(
        &self,
        _request: dsh_web::WebSearchRequest,
        _cancelled: dsh_web::Cancelled,
    ) -> Result<dsh_web::WebSearchResult, WebError> {
        Err(WebError::new(
            "NATIVE_TOOL_UNSUPPORTED",
            "selected route has no native search",
        ))
    }
}

#[tokio::test]
async fn search_keeps_capability_error_codes_for_single_and_batched_queries() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).unwrap();
    let tools = dsh_tools::ToolRuntime::install(&ctx, dsh_tools::Config::default()).unwrap();
    let search: Arc<dyn dsh_web::WebSearch> = Arc::new(StubWeb);
    ctx.provide("web", Some(cordis::arc(search)));
    let _disposer = apply(
        &ctx,
        &Config {
            fetch: false,
            ..Config::default()
        },
    )
    .unwrap();
    for queries in [
        serde_json::json!(["one"]),
        serde_json::json!(["one", "two"]),
    ] {
        let result = tools
            .execute(dsh_tools::ToolExecutionInput {
                call_id: dsh_llm::call_id("search-capability"),
                root_call_id: None,
                name: "web_search".into(),
                arguments: serde_json::json!({"queries":queries}),
                agent: None,
                parent: None,
                signal: Arc::new(|| false),
            })
            .await;
        assert!(result.is_error);
        assert_eq!(
            result.error.as_ref().unwrap().info.as_ref().unwrap().code,
            "NATIVE_TOOL_UNSUPPORTED"
        );
    }
}

impl cordis::Service for StubWeb {
    fn service_name(&self) -> &'static str {
        "web"
    }
}

#[async_trait]
impl WebFetch for StubWeb {
    async fn fetch(
        &self,
        request: WebFetchRequest,
        _cancelled: dsh_web::Cancelled,
    ) -> Result<WebFetchResult, WebError> {
        Ok(WebFetchResult {
            url: request.url,
            status_code: 200,
            body: WebFetchBody::Html {
                content: "<h1>Hello</h1><script>ignore()</script><p>World</p>".into(),
            },
            truncated: false,
        })
    }
}

#[tokio::test]
async fn registers_web_fetch_when_enabled() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("system prompt");
    let tools = dsh_tools::ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let fetch: Arc<dyn WebFetch> = Arc::new(StubWeb);
    ctx.provide("webFetch", Some(cordis::arc(fetch)));

    let _disposer = apply(
        &ctx,
        &Config {
            search: false,
            fetch: true,
            ..Config::default()
        },
    )
    .expect("web_fetch should register");

    let schema = tools
        .schemas(None)
        .into_iter()
        .find(|schema| schema.name == "web_fetch")
        .expect("web_fetch schema");
    assert_eq!(schema.parameters["required"], serde_json::json!(["url"]));
    assert_eq!(
        schema.parameters["properties"].as_object().unwrap().len(),
        1
    );
    let definition = tools.get("web_fetch", None).expect("web_fetch definition");
    assert_eq!(definition.timeout_ms, Some(30_000));
}
