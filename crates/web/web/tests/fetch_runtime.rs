use std::sync::Arc;

use async_trait::async_trait;
use dsh_web::{
    Config, WebFetch, WebFetchBody, WebFetchProvider, WebFetchRequest, WebFetchResult, WebRuntime,
};

struct StubFetch;

#[async_trait]
impl WebFetchProvider for StubFetch {
    fn id(&self) -> &str {
        "stub"
    }

    fn available(&self) -> bool {
        true
    }

    async fn fetch(
        &self,
        request: WebFetchRequest,
        _cancelled: dsh_web::Cancelled,
    ) -> Result<WebFetchResult, dsh_web::WebError> {
        Ok(WebFetchResult {
            url: request.url,
            status_code: 200,
            body: WebFetchBody::Text {
                content: "ok".into(),
            },
            truncated: false,
        })
    }
}

#[tokio::test]
async fn fetch_provider_is_selected_independently() {
    let runtime = WebRuntime::new(Config {
        search_provider: None,
        fetch_provider: Some("stub".into()),
    });
    runtime
        .register_fetch_provider(Arc::new(StubFetch))
        .expect("register fetch provider");
    let seam: Arc<dyn WebFetch> = runtime;
    let result = seam
        .fetch(
            WebFetchRequest {
                url: "https://example.com/".into(),
            },
            Arc::new(|| false),
        )
        .await
        .expect("fetch");
    assert_eq!(result.body.content(), "ok");
}
