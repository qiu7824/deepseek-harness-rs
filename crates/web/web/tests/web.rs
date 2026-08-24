use std::sync::Arc;

use async_trait::async_trait;
use dsh_web::{
    Config, WebError, WebRuntime, WebSearchProvider, WebSearchRequest, WebSearchResult,
    WebSearchSource,
};

struct Provider {
    id: &'static str,
    available: bool,
}

#[async_trait]
impl WebSearchProvider for Provider {
    fn id(&self) -> &str {
        self.id
    }
    fn available(&self) -> bool {
        self.available
    }
    async fn search(
        &self,
        _request: WebSearchRequest,
        _cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<WebSearchResult, WebError> {
        Ok(WebSearchResult {
            content: Some(self.id.into()),
            sources: vec![
                WebSearchSource {
                    url: "https://1".into(),
                    title: None,
                    snippet: None,
                    published_at: None,
                },
                WebSearchSource {
                    url: "https://2".into(),
                    title: None,
                    snippet: None,
                    published_at: None,
                },
            ],
            truncated: false,
        })
    }
}

#[tokio::test]
async fn selects_at_execution_time_and_caps_provider_results() {
    let runtime = WebRuntime::new(Config::default());
    let dispose = runtime
        .register_search_provider(Arc::new(Provider {
            id: "one",
            available: true,
        }))
        .unwrap();
    let result = runtime
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: Some(1),
            },
            Arc::new(|| false),
        )
        .await
        .unwrap();
    assert_eq!(result.content.as_deref(), Some("one"));
    assert_eq!(result.sources.len(), 1);
    assert!(result.truncated);
    dispose();
    assert_eq!(
        runtime
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None
                },
                Arc::new(|| false)
            )
            .await
            .unwrap_err()
            .code(),
        "WEB_PROVIDER_UNAVAILABLE"
    );
}

#[tokio::test]
async fn reports_duplicate_configured_and_ambiguous_providers() {
    let runtime = WebRuntime::new(Config::default());
    runtime
        .register_search_provider(Arc::new(Provider {
            id: "one",
            available: true,
        }))
        .unwrap();
    assert_eq!(
        runtime
            .register_search_provider(Arc::new(Provider {
                id: "one",
                available: true
            }))
            .unwrap_err()
            .code(),
        "WEB_DUPLICATE_PROVIDER"
    );
    runtime
        .register_search_provider(Arc::new(Provider {
            id: "two",
            available: true,
        }))
        .unwrap();
    assert_eq!(
        runtime
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None
                },
                Arc::new(|| false)
            )
            .await
            .unwrap_err()
            .code(),
        "WEB_PROVIDER_AMBIGUOUS"
    );

    let configured = WebRuntime::new(Config {
        search_provider: Some("missing".into()),
    });
    configured
        .register_search_provider(Arc::new(Provider {
            id: "one",
            available: true,
        }))
        .unwrap();
    assert_eq!(
        configured
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None
                },
                Arc::new(|| false)
            )
            .await
            .unwrap_err()
            .code(),
        "WEB_PROVIDER_CONFIGURED_MISSING"
    );
}
