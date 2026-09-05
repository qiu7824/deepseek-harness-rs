//! `llm` domain contract: host-scoped provider topology for configuration
//! surfaces. Rust port of `packages/host/apiproxy/src/api/llm.ts`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::api::rpc::{RpcRequest, RpcResponse};
use crate::api::sessions::{ModelCatalogFailure, ModelProviderGroup};
use crate::fetch::handler::AbortSignal;

/// Wire view of one configurable provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurableProviderView {
    /// Provider route key (`deepseek-official`, `openai`, …).
    pub provider: String,
    /// Human-readable name for configuration surfaces.
    pub display_name: String,
    /// Settings namespace whose section configures this provider.
    pub settings_ns: String,
    /// Path from that section's root to the provider's profile object
    /// (empty = whole section).
    pub settings_path: Vec<String>,
    /// Whether the route is currently registered (its models are
    /// requestable).
    pub active: bool,
    /// Whether the owning adapter knows this route only because
    /// configuration declared it. Absent = "unknown", not "shipped".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<bool>,
}

/// `llm.providers` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmProvidersResult {
    pub providers: Vec<ConfigurableProviderView>,
}

/// `llm.models` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelsResult {
    pub groups: Vec<ModelProviderGroup>,
    pub failures: Vec<ModelCatalogFailure>,
}

/// `llm.discoverModels` request payload (the draft, not a stored route).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmDiscoverModelsRequest {
    pub settings_ns: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(rename = "baseURL", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// Accepted here but never stored or returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// Wire view of one model an interrogated endpoint advertises.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModelView {
    /// Model id the endpoint accepts.
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_descriptions: Option<std::collections::BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_summaries: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_parameters: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<dsh_llm::ModelModality>>,
    /// Human-readable name when the endpoint supplies one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Maximum combined request and response context, when disclosed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Maximum output tokens, when disclosed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

/// `llm.discoverModels` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmDiscoverModelsResult {
    pub models: Vec<DiscoveredModelView>,
}

/// Llm-domain unary methods (the map keys `llm.*`).
#[async_trait]
pub trait LlmApi: Send + Sync {
    /// List every configurable provider with its live/dormant state, in
    /// directory declaration order.
    async fn providers(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<LlmProvidersResult>;

    /// Host-scoped model catalog over every registered provider route.
    async fn models(&self, request: RpcRequest<serde_json::Value>) -> RpcResponse<LlmModelsResult>;

    /// Interrogate a provider endpoint the configuration surface is still
    /// drafting, and return the models it advertises for the user to adopt.
    async fn discover_models(
        &self,
        request: RpcRequest<LlmDiscoverModelsRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<LlmDiscoverModelsResult>;
}
