//! `credentials` domain contract: the web face of the credential-reference
//! seam. Reads are structurally value-free, and the value crosses the wire
//! in exactly one direction, inside `credentials.set`. There is no
//! enumeration method by design. Rust port of
//! `packages/host/apiproxy/src/api/credentials.ts`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::api::rpc::{RpcRequest, RpcResponse};

/// Wire view of one credential reference's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialView {
    /// Whether any layer currently supplies a non-empty value.
    pub configured: bool,
    /// Winning layer when configured (`env`, `file`, …); provider
    /// vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Whether `credentials.set`/`credentials.unset` can affect this
    /// reference.
    pub writable: bool,
}

/// `credentials.describe` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CredentialsDescribeRequest {
    #[serde(rename = "refs")]
    pub references: Vec<String>,
}

/// `credentials.describe` response value: name → view, insertion-ordered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CredentialsDescribeResult {
    pub credentials: indexmap::IndexMap<String, CredentialView>,
}

/// `credentials.set` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CredentialsSetRequest {
    #[serde(rename = "ref")]
    pub reference: String,
    pub value: String,
}

/// `credentials.unset` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CredentialsUnsetRequest {
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Credentials-domain unary methods (the map keys `credentials.*`).
#[async_trait]
pub trait CredentialsApi: Send + Sync {
    /// Describe the named references (batch): configured state, winning
    /// source, and writability — never values. An invalid reference name is
    /// a `bad-request`; an unknown-but-valid one describes as unconfigured.
    async fn describe(
        &self,
        request: RpcRequest<CredentialsDescribeRequest>,
    ) -> RpcResponse<CredentialsDescribeResult>;

    /// Store one credential value in the writable layer. Rejected with
    /// `credential-rejected` while a read-only layer shadows the reference.
    async fn set(
        &self,
        request: RpcRequest<CredentialsSetRequest>,
    ) -> RpcResponse<serde_json::Value>;

    /// Remove one credential from the writable layer; same shadowing
    /// rejection as `set`. Unsetting an absent reference succeeds.
    async fn unset(
        &self,
        request: RpcRequest<CredentialsUnsetRequest>,
    ) -> RpcResponse<serde_json::Value>;
}
