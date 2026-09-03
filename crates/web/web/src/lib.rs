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
    pub fetch_provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchRequest {
    pub query: String,
    pub max_results: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebFetchRequest {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WebFetchBody {
    Html { content: String },
    Text { content: String },
}

impl WebFetchBody {
    pub fn content(&self) -> &str {
        match self {
            Self::Html { content } | Self::Text { content } => content,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebFetchResult {
    pub url: String,
    pub status_code: u16,
    pub body: WebFetchBody,
    pub truncated: bool,
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

#[async_trait]
pub trait WebFetch: Service {
    async fn fetch(
        &self,
        request: WebFetchRequest,
        cancelled: Cancelled,
    ) -> Result<WebFetchResult, WebError>;
}

#[async_trait]
pub trait WebFetchProvider: Send + Sync {
    fn id(&self) -> &str;
    fn available(&self) -> bool;
    async fn fetch(
        &self,
        request: WebFetchRequest,
        cancelled: Cancelled,
    ) -> Result<WebFetchResult, WebError>;
}

pub struct WebRuntime {
    config: Config,
    search_providers: Arc<Mutex<BTreeMap<String, Arc<dyn WebSearchProvider>>>>,
    fetch_providers: Arc<Mutex<BTreeMap<String, Arc<dyn WebFetchProvider>>>>,
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
            search_providers: Arc::new(Mutex::new(BTreeMap::new())),
            fetch_providers: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let service = Self::new(config);
        let search: Arc<dyn WebSearch> = service.clone();
        let fetch: Arc<dyn WebFetch> = service.clone();
        ctx.provide("web", Some(cordis::arc(search)));
        ctx.provide("webFetch", Some(cordis::arc(fetch)));
        service
    }

    pub fn register_search_provider(
        &self,
        provider: Arc<dyn WebSearchProvider>,
    ) -> Result<Disposer, WebError> {
        register_provider(&self.search_providers, provider, "search")
    }

    pub fn register_fetch_provider(
        &self,
        provider: Arc<dyn WebFetchProvider>,
    ) -> Result<Disposer, WebError> {
        register_provider(&self.fetch_providers, provider, "fetch")
    }

    fn selected_search(&self) -> Result<Arc<dyn WebSearchProvider>, WebError> {
        select_provider(
            &self.search_providers,
            self.config.search_provider.as_deref(),
            "search",
        )
    }

    fn selected_fetch(&self) -> Result<Arc<dyn WebFetchProvider>, WebError> {
        select_provider(
            &self.fetch_providers,
            self.config.fetch_provider.as_deref(),
            "fetch",
        )
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
        let mut result = self.selected_search()?.search(request, cancelled).await?;
        if let Some(cap) = cap
            && result.sources.len() > cap
        {
            result.sources.truncate(cap);
            result.truncated = true;
        }
        Ok(result)
    }

    async fn fetch_inner(
        &self,
        request: WebFetchRequest,
        cancelled: Cancelled,
    ) -> Result<WebFetchResult, WebError> {
        if cancelled() {
            return Err(WebError::new("WEB_ABORTED", "web fetch aborted"));
        }
        self.selected_fetch()?.fetch(request, cancelled).await
    }

    pub async fn search(
        &self,
        request: WebSearchRequest,
        cancelled: Cancelled,
    ) -> Result<WebSearchResult, WebError> {
        self.search_inner(request, cancelled).await
    }

    pub async fn fetch(
        &self,
        request: WebFetchRequest,
        cancelled: Cancelled,
    ) -> Result<WebFetchResult, WebError> {
        self.fetch_inner(request, cancelled).await
    }
}

fn register_provider<P>(
    providers: &Arc<Mutex<BTreeMap<String, Arc<P>>>>,
    provider: Arc<P>,
    kind: &str,
) -> Result<Disposer, WebError>
where
    P: ?Sized + Send + Sync + ProviderIdentity + 'static,
{
    let id = provider.id().to_string();
    let mut store = providers.lock();
    if store.contains_key(&id) {
        return Err(WebError::new(
            "WEB_DUPLICATE_PROVIDER",
            format!("duplicate web {kind} provider {id}"),
        ));
    }
    store.insert(id.clone(), provider);
    let providers = providers.clone();
    Ok(Disposer(Arc::new(move || {
        providers.lock().remove(&id);
    })))
}

trait ProviderIdentity {
    fn id(&self) -> &str;
    fn available(&self) -> bool;
}

impl ProviderIdentity for dyn WebSearchProvider {
    fn id(&self) -> &str {
        WebSearchProvider::id(self)
    }
    fn available(&self) -> bool {
        WebSearchProvider::available(self)
    }
}

impl ProviderIdentity for dyn WebFetchProvider {
    fn id(&self) -> &str {
        WebFetchProvider::id(self)
    }
    fn available(&self) -> bool {
        WebFetchProvider::available(self)
    }
}

fn select_provider<P: ?Sized + ProviderIdentity>(
    providers: &Arc<Mutex<BTreeMap<String, Arc<P>>>>,
    configured: Option<&str>,
    kind: &str,
) -> Result<Arc<P>, WebError> {
    let providers = providers.lock();
    if let Some(id) = configured {
        let provider = providers.get(id).ok_or_else(|| {
            WebError::new(
                "WEB_PROVIDER_CONFIGURED_MISSING",
                format!("configured web {kind} provider {id} is not registered"),
            )
        })?;
        if !provider.available() {
            return Err(WebError::new(
                "WEB_PROVIDER_CONFIGURED_UNAVAILABLE",
                format!("configured web {kind} provider {id} is unavailable"),
            ));
        }
        return Ok(provider.clone());
    }
    let available = providers
        .values()
        .filter(|provider| provider.available())
        .cloned()
        .collect::<Vec<_>>();
    match available.as_slice() {
        [] => Err(WebError::new(
            "WEB_PROVIDER_UNAVAILABLE",
            format!("no usable web {kind} provider"),
        )),
        [provider] => Ok(provider.clone()),
        _ => Err(WebError::new(
            "WEB_PROVIDER_AMBIGUOUS",
            format!("multiple usable web {kind} providers"),
        )),
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

#[async_trait]
impl WebFetch for WebRuntime {
    async fn fetch(
        &self,
        request: WebFetchRequest,
        cancelled: Cancelled,
    ) -> Result<WebFetchResult, WebError> {
        self.fetch_inner(request, cancelled).await
    }
}
