//! `settings` domain contract: the web face of the user-settings seam.
//! Every payload that leaves this domain is redacted by the seam. Rust port
//! of `packages/host/apiproxy/src/api/settings.ts`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::api::rpc::{RpcRequest, RpcResponse};
use crate::fetch::handler::AbortSignal;

/// One schema-declared secret slot inside a redacted namespace value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsSecretView {
    /// Path from the section root to the removed field.
    pub path: Vec<String>,
    /// Whether the slot currently holds a value (the value itself never
    /// rides).
    pub set: bool,
}

/// How the owner applies changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsApplies {
    Live,
    Restart,
}

/// Wire view of one registered settings namespace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsNamespaceView {
    /// Namespace key (`llm-deepseek`, `llm-pi-ai`, …).
    pub ns: String,
    /// Serialized schemastery schema envelope.
    pub schema: serde_json::Value,
    /// Redacted resolved value.
    pub value: serde_json::Value,
    /// Redacted composition base layer, when the registrant declared one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<serde_json::Value>,
    /// Redacted raw user section, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<serde_json::Value>,
    /// When the owner applies changes.
    pub applies: SettingsApplies,
    /// Every schema-declared secret slot with its configured state.
    pub secrets: Vec<SettingsSecretView>,
    /// Monotonic revision of the raw user section this view was read at.
    pub revision: i64,
}

/// One path-addressed edit carried by `settings.mutate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum SettingsPathOpView {
    Set {
        path: Vec<String>,
        value: serde_json::Value,
    },
    Unset {
        path: Vec<String>,
    },
}

/// `settings.describe` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDescribeResult {
    pub writable: bool,
    pub has_document: bool,
    pub namespaces: Vec<SettingsNamespaceView>,
}

/// `settings.openDocument` response value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsOpenDocumentResult {
    pub opened: bool,
}

/// `settings.update` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdateRequest {
    pub ns: String,
    pub patch: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<i64>,
}

/// `settings.replace` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsReplaceRequest {
    pub ns: String,
    pub section: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<i64>,
}

/// `settings.mutate` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsMutateRequest {
    pub ns: String,
    pub ops: Vec<SettingsPathOpView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<i64>,
}

/// Settings-domain unary methods (the map keys `settings.*`).
#[async_trait]
pub trait SettingsApi: Send + Sync {
    /// Describe every registered namespace: redacted layered values plus the
    /// serialized schema a client renders its form from.
    async fn describe(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<SettingsDescribeResult>;

    /// Materialize the configured local document when absent and ask the
    /// Host to hand it to the platform text-document opener.
    async fn open_document(
        &self,
        request: RpcRequest<serde_json::Value>,
        signal: AbortSignal,
    ) -> RpcResponse<SettingsOpenDocumentResult>;

    /// Merge a patch into one namespace's user layer (validate → persist →
    /// commit).
    async fn update(
        &self,
        request: RpcRequest<SettingsUpdateRequest>,
    ) -> RpcResponse<SettingsNamespaceView>;

    /// Replace one namespace's user section wholesale.
    async fn replace(
        &self,
        request: RpcRequest<SettingsReplaceRequest>,
    ) -> RpcResponse<SettingsNamespaceView>;

    /// Apply path-addressed edits to one namespace's user section.
    async fn mutate(
        &self,
        request: RpcRequest<SettingsMutateRequest>,
    ) -> RpcResponse<SettingsNamespaceView>;
}
