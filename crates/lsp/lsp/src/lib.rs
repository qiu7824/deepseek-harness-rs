use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Weak};

pub use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LspProviderId(String);

impl LspProviderId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LspOperation {
    GoToDefinition,
    FindReferences,
    GoToImplementation,
    Hover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspPosition {
    pub line: u64,
    pub character: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspHover {
    pub contents: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<LspRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspQueryRequest {
    pub operation: LspOperation,
    pub file_path: String,
    pub position: LspPosition,
    pub workspace_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspProviderQuery {
    pub operation: LspOperation,
    pub file_path: String,
    pub position: LspPosition,
    pub workspace_root: String,
    pub language_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LspQueryResult {
    Locations {
        locations: Vec<LspLocation>,
        #[serde(rename = "resolvedWorkspaceUri")]
        resolved_workspace_uri: String,
    },
    Hover {
        hover: Option<LspHover>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspError {
    message: String,
    code: String,
}

impl LspError {
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

impl fmt::Display for LspError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for LspError {}

#[async_trait]
pub trait LspProvider: Send + Sync {
    fn id(&self) -> &LspProviderId;
    fn extension_to_language(&self) -> &BTreeMap<String, String>;
    async fn query(&self, request: LspProviderQuery) -> Result<LspQueryResult, LspError>;
}

#[derive(Clone)]
struct Route {
    provider: Arc<dyn LspProvider>,
    language_id: String,
}

#[derive(Default)]
struct Registry {
    provider_ids: HashSet<LspProviderId>,
    routes: HashMap<String, Route>,
}

pub struct LspRegistration {
    registry: Weak<parking_lot::Mutex<Registry>>,
    provider_id: LspProviderId,
    extensions: Vec<String>,
}

impl Drop for LspRegistration {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let mut registry = registry.lock();
        registry.provider_ids.remove(&self.provider_id);
        for extension in &self.extensions {
            registry.routes.remove(extension);
        }
    }
}

pub struct Lsp {
    registry: Arc<parking_lot::Mutex<Registry>>,
}

impl Lsp {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(parking_lot::Mutex::new(Registry::default())),
        }
    }

    pub fn register_provider(
        &self,
        provider: Arc<dyn LspProvider>,
    ) -> Result<LspRegistration, LspError> {
        let provider_id = provider.id().clone();
        let mut pending = Vec::new();
        for (raw_extension, language_id) in provider.extension_to_language() {
            let extension = normalize_extension(raw_extension);
            pending.push((extension, language_id.clone()));
        }
        let mut registry = self.registry.lock();
        if registry.provider_ids.contains(&provider_id) {
            return Err(LspError::new(
                format!(
                    "an LSP provider with id {:?} is already registered",
                    provider_id.as_str()
                ),
                "LSP_CONFLICT",
            ));
        }
        if let Some((extension, _)) = pending
            .iter()
            .find(|(extension, _)| registry.routes.contains_key(extension))
        {
            return Err(LspError::new(
                format!("extension {extension:?} is already handled by another LSP provider"),
                "LSP_CONFLICT",
            ));
        }
        registry.provider_ids.insert(provider_id.clone());
        for (extension, language_id) in &pending {
            registry.routes.insert(
                extension.clone(),
                Route {
                    provider: provider.clone(),
                    language_id: language_id.clone(),
                },
            );
        }
        Ok(LspRegistration {
            registry: Arc::downgrade(&self.registry),
            provider_id,
            extensions: pending
                .into_iter()
                .map(|(extension, _)| extension)
                .collect(),
        })
    }

    pub async fn query(&self, request: LspQueryRequest) -> Result<LspQueryResult, LspError> {
        let route = self
            .registry
            .lock()
            .routes
            .get(&final_extension(&request.file_path))
            .cloned();
        let Some(route) = route else {
            return Err(LspError::new(
                format!("no LSP provider handles {:?}", request.file_path),
                "LSP_UNAVAILABLE",
            ));
        };
        route
            .provider
            .query(LspProviderQuery {
                operation: request.operation,
                file_path: request.file_path,
                position: request.position,
                workspace_root: request.workspace_root,
                language_id: route.language_id,
            })
            .await
    }
}

pub fn final_extension(file_path: &str) -> String {
    let base = file_path.rsplit(['/', '\\']).next().unwrap_or(file_path);
    let Some(dot) = base.rfind('.') else {
        return String::new();
    };
    if dot == 0 {
        return String::new();
    }
    base[dot..].to_ascii_lowercase()
}

fn normalize_extension(extension: &str) -> String {
    let lower = extension.to_ascii_lowercase();
    if lower.starts_with('.') {
        lower
    } else {
        format!(".{lower}")
    }
}

impl Default for Lsp {
    fn default() -> Self {
        Self::new()
    }
}
