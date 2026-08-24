//! Provider-neutral web capability registry and execution seam.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use cordis::{Context, Service};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

pub type Cancelled = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
pub struct Disposer(Arc<dyn Fn() + Send + Sync>);

impl std::fmt::Debug for Disposer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Disposer(..)")
    }
}

impl std::ops::Deref for Disposer {
    type Target = dyn Fn() + Send + Sync;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub search_provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchRequest {
    pub query: String,
    pub max_results: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchSource {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub sources: Vec<WebSearchSource>,
    pub truncated: bool,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{message}")]
pub struct WebError {
    code: String,
    message: String,
}

impl WebError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

#[async_trait]
pub trait WebSearch: Service {
    async fn search(
        &self,
        request: WebSearchRequest,
        cancelled: Cancelled,
    ) -> Result<WebSearchResult, WebError>;
}

#[async_trait]
pub trait WebSearchProvider: Send + Sync {
    fn id(&self) -> &str;
    fn available(&self) -> bool;
    async fn search(
        &self,
        request: WebSearchRequest,
        cancelled: Cancelled,
    ) -> Result<WebSearchResult, WebError>;
}

pub struct WebRuntime {
    config: Config,
    providers: Arc<Mutex<BTreeMap<String, Arc<dyn WebSearchProvider>>>>,
}

impl Service for WebRuntime {
    fn service_name(&self) -> &'static str {
        "web"
    }
}

impl WebRuntime {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self {
            config,
            providers: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let service = Self::new(config);
        let seam: Arc<dyn WebSearch> = service.clone();
        ctx.provide("web", Some(cordis::arc(seam)));
        service
    }

    pub fn register_search_provider(
        &self,
        provider: Arc<dyn WebSearchProvider>,
    ) -> Result<Disposer, WebError> {
        let id = provider.id().to_string();
        let mut providers = self.providers.lock();
        if providers.contains_key(&id) {
            return Err(WebError::new(
                "WEB_DUPLICATE_PROVIDER",
                format!("duplicate web search provider {id}"),
            ));
        }
        providers.insert(id.clone(), provider);
        let providers = self.providers.clone();
        Ok(Disposer(Arc::new(move || {
            providers.lock().remove(&id);
        })))
    }

    fn selected(&self) -> Result<Arc<dyn WebSearchProvider>, WebError> {
        let providers = self.providers.lock();
        if let Some(id) = &self.config.search_provider {
            return providers
                .get(id)
                .filter(|provider| provider.available())
                .cloned()
                .ok_or_else(|| {
                    WebError::new(
                        "WEB_PROVIDER_CONFIGURED_MISSING",
                        format!("configured web search provider {id} is unavailable"),
                    )
                });
        }
        let available: Vec<_> = providers
            .values()
            .filter(|provider| provider.available())
            .cloned()
            .collect();
        match available.as_slice() {
            [] => Err(WebError::new(
                "WEB_PROVIDER_UNAVAILABLE",
                "no usable web search provider",
            )),
            [provider] => Ok(provider.clone()),
            _ => Err(WebError::new(
                "WEB_PROVIDER_AMBIGUOUS",
                "multiple usable web search providers",
            )),
        }
    }

    async fn search_inner(
        &self,
        request: WebSearchRequest,
        cancelled: Cancelled,
    ) -> Result<WebSearchResult, WebError> {
        if cancelled() {
            return Err(WebError::new("WEB_ABORTED", "web search aborted"));
        }
        let cap = request.max_results;
        let mut result = self.selected()?.search(request, cancelled).await?;
        if let Some(cap) = cap
            && result.sources.len() > cap
        {
            result.sources.truncate(cap);
            result.truncated = true;
        }
        Ok(result)
    }

    pub async fn search(
        &self,
        request: WebSearchRequest,
        cancelled: Cancelled,
    ) -> Result<WebSearchResult, WebError> {
        self.search_inner(request, cancelled).await
    }
}

#[async_trait]
impl WebSearch for WebRuntime {
    async fn search(
        &self,
        request: WebSearchRequest,
        cancelled: Cancelled,
    ) -> Result<WebSearchResult, WebError> {
        self.search_inner(request, cancelled).await
    }
}
