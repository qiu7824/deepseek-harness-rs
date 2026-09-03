use std::sync::Arc;

use async_trait::async_trait;
use cordis::Context;
use dsh_tool_web::{Config, apply};
use dsh_web::{WebError, WebFetch, WebFetchBody, WebFetchRequest, WebFetchResult};

struct StubWeb;

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
