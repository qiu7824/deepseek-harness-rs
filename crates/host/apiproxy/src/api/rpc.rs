//! Four-quadrant RPC message model. Channels and messages are decoupled:
//! HTTP, WebSocket, and in-process SSE are physical carriers, while logical
//! messages are channel-independent and form a four-member discriminated
//! union. Rust port of `packages/host/apiproxy/src/api/rpc.ts` — the
//! contract layer, zero cordis dependencies (the TS layer is importable
//! from the browser).
//!
//! # Wire notes
//!
//! - The `type` member discriminates the four full forms (`client-request`,
//!   `server-response`, `server-request`, `client-response`).
//! - `RpcResult` discriminates on the boolean `ok` member with literal
//!   `true`/`false` values; hybrids ({`ok: true` without `value`) fail.
//! - A void business result serializes with no `value` field at all; the
//!   wire-wide slot models that with [`WireRpcResult`]'s optional value,
//!   while the typed [`RpcResult<T>`] requires its declared value.

use dsh_brand::Branded;
use dsh_typert_protocol::RemoteError;
use serde::{Deserialize, Serialize};

/// Brand marker for message correlation ids.
#[doc(hidden)]
pub enum RpcIdTag {}

/// Message correlation id: the initiator mints it on a request; a response
/// echoes the matching request's rpcId and never mints a new one.
pub type RpcId = Branded<RpcIdTag>;

/// Brands a string as [`RpcId`] (compile-time cast, zero runtime cost; the
/// TS `RpcId()` precedent).
pub fn rpc_id(id: impl Into<String>) -> RpcId {
    RpcId::new(id)
}

/// The closed error-code union (TS `RpcErrorCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RpcErrorCode {
    BadRequest,
    Cancelled,
    SessionNotFound,
    ModelUnavailable,
    SessionConflict,
    InvalidTimeZone,
    WorkspaceAttachFailed,
    WorkspaceNotFound,
    WorkspaceInvalidPath,
    WorkspaceNameConflict,
    WorkspaceMoveInvalid,
    DirectoryUnreadable,
    DirectoryExists,
    DirectoryCreateFailed,
    DirectoryPickerUnavailable,
    AgentPresetReadOnly,
    AgentPresetLocked,
    AgentPresetConflict,
    AgentPresetNotFound,
    AgentPresetInvalid,
    AgentBusy,
    AttachmentError,
    QueueItemNotFound,
    SteerUnavailable,
    CommandError,
    UnknownCommand,
    SettingsRejected,
    SettingsNotExposed,
    SettingsConflict,
    CredentialRejected,
    ModelDiscoveryFailed,
    TitleInvalid,
    ForkUnavailable,
    SubagentParentUnavailable,
    SubagentNotFound,
    SubagentCatalogDiagnostic,
    SubagentNotResumable,
    SubagentUnauthorized,
    SubagentDeliveryUnavailable,
    Internal,
}

/// Shared gateway code used when no more specific typed dispatch failure applies.
pub const GATEWAY_INTERNAL_CODE: &str = "gateway/internal";

impl RpcErrorCode {
    /// The wire code literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BadRequest => "bad-request",
            Self::Cancelled => "cancelled",
            Self::SessionNotFound => "session-not-found",
            Self::ModelUnavailable => "model-unavailable",
            Self::SessionConflict => "session-conflict",
            Self::InvalidTimeZone => "invalid-time-zone",
            Self::WorkspaceAttachFailed => "workspace-attach-failed",
            Self::WorkspaceNotFound => "workspace-not-found",
            Self::WorkspaceInvalidPath => "workspace-invalid-path",
            Self::WorkspaceNameConflict => "workspace-name-conflict",
            Self::WorkspaceMoveInvalid => "workspace-move-invalid",
            Self::DirectoryUnreadable => "directory-unreadable",
            Self::DirectoryExists => "directory-exists",
            Self::DirectoryCreateFailed => "directory-create-failed",
            Self::DirectoryPickerUnavailable => "directory-picker-unavailable",
            Self::AgentPresetReadOnly => "agent-preset-read-only",
            Self::AgentPresetLocked => "agent-preset-locked",
            Self::AgentPresetConflict => "agent-preset-conflict",
            Self::AgentPresetNotFound => "agent-preset-not-found",
            Self::AgentPresetInvalid => "agent-preset-invalid",
            Self::AgentBusy => "agent-busy",
            Self::AttachmentError => "attachment-error",
            Self::QueueItemNotFound => "queue-item-not-found",
            Self::SteerUnavailable => "steer-unavailable",
            Self::CommandError => "command-error",
            Self::UnknownCommand => "unknown-command",
            Self::SettingsRejected => "settings-rejected",
            Self::SettingsNotExposed => "settings-not-exposed",
            Self::SettingsConflict => "settings-conflict",
            Self::CredentialRejected => "credential-rejected",
            Self::ModelDiscoveryFailed => "model-discovery-failed",
            Self::TitleInvalid => "title-invalid",
            Self::ForkUnavailable => "fork-unavailable",
            Self::SubagentParentUnavailable => "subagent-parent-unavailable",
            Self::SubagentNotFound => "subagent-not-found",
            Self::SubagentCatalogDiagnostic => "subagent-catalog-diagnostic",
            Self::SubagentNotResumable => "subagent-not-resumable",
            Self::SubagentUnauthorized => "subagent-unauthorized",
            Self::SubagentDeliveryUnavailable => "subagent-delivery-unavailable",
            Self::Internal => "internal",
        }
    }

    /// Parse a wire code literal (unknown codes are a schema failure).
    pub fn parse_wire_code(value: &str) -> Option<Self> {
        Some(match value {
            "bad-request" => Self::BadRequest,
            "cancelled" => Self::Cancelled,
            "session-not-found" => Self::SessionNotFound,
            "model-unavailable" => Self::ModelUnavailable,
            "session-conflict" => Self::SessionConflict,
            "invalid-time-zone" => Self::InvalidTimeZone,
            "workspace-attach-failed" => Self::WorkspaceAttachFailed,
            "workspace-not-found" => Self::WorkspaceNotFound,
            "workspace-invalid-path" => Self::WorkspaceInvalidPath,
            "workspace-name-conflict" => Self::WorkspaceNameConflict,
            "workspace-move-invalid" => Self::WorkspaceMoveInvalid,
            "directory-unreadable" => Self::DirectoryUnreadable,
            "directory-exists" => Self::DirectoryExists,
            "directory-create-failed" => Self::DirectoryCreateFailed,
            "directory-picker-unavailable" => Self::DirectoryPickerUnavailable,
            "agent-preset-read-only" => Self::AgentPresetReadOnly,
            "agent-preset-locked" => Self::AgentPresetLocked,
            "agent-preset-conflict" => Self::AgentPresetConflict,
            "agent-preset-not-found" => Self::AgentPresetNotFound,
            "agent-preset-invalid" => Self::AgentPresetInvalid,
            "agent-busy" => Self::AgentBusy,
            "attachment-error" => Self::AttachmentError,
            "queue-item-not-found" => Self::QueueItemNotFound,
            "steer-unavailable" => Self::SteerUnavailable,
            "command-error" => Self::CommandError,
            "unknown-command" => Self::UnknownCommand,
            "settings-rejected" => Self::SettingsRejected,
            "settings-not-exposed" => Self::SettingsNotExposed,
            "settings-conflict" => Self::SettingsConflict,
            "credential-rejected" => Self::CredentialRejected,
            "model-discovery-failed" => Self::ModelDiscoveryFailed,
            "title-invalid" => Self::TitleInvalid,
            "fork-unavailable" => Self::ForkUnavailable,
            "subagent-parent-unavailable" => Self::SubagentParentUnavailable,
            "subagent-not-found" => Self::SubagentNotFound,
            "subagent-catalog-diagnostic" => Self::SubagentCatalogDiagnostic,
            "subagent-not-resumable" => Self::SubagentNotResumable,
            "subagent-unauthorized" => Self::SubagentUnauthorized,
            "subagent-delivery-unavailable" => Self::SubagentDeliveryUnavailable,
            "internal" => Self::Internal,
            _ => return None,
        })
    }
}

/// `{}` details (cancelled, command-error, unknown-command, internal).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyDetails {}

/// `bad-request` details: validation issues (zod issue values pass through
/// untyped — the TS schema uses `z.custom`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BadRequestDetails {
    pub issues: Vec<serde_json::Value>,
}

/// `session-not-found` / `title-invalid` / `fork-unavailable` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIdDetails {
    pub session_id: String,
}

/// `model-unavailable` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUnavailableDetails {
    pub provider: String,
    pub model: String,
}

/// `session-conflict` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConflictDetails {
    pub session_id: String,
    pub requested_cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_cwd: Option<String>,
}

/// `invalid-time-zone` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueDetails {
    pub value: String,
}

/// `workspace-attach-failed` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAttachFailedDetails {
    pub session_id: String,
    pub workspace_id: String,
}

/// `workspace-not-found` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceIdDetails {
    pub workspace_id: String,
}

/// `workspace-invalid-path` / `directory-*` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathDetails {
    pub path: String,
}

/// `workspace-name-conflict` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameDetails {
    pub name: String,
}

/// `workspace-move-invalid` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMoveInvalidDetails {
    pub workspace_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_session_id: Option<String>,
}

/// `directory-picker-unavailable` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDetails {
    pub capability: String,
}

/// `agent-preset-read-only` / `agent-preset-invalid` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetReasonDetails {
    pub agent_preset: String,
    pub reason: String,
}

/// `agent-preset-locked` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetLockedDetails {
    pub session_id: String,
    pub agent_preset: String,
}

/// `agent-preset-conflict` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetConflictDetails {
    pub session_id: String,
    pub requested_preset: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_preset: Option<String>,
}

/// `agent-preset-not-found` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetNotFoundDetails {
    pub agent_preset: String,
    pub available: Vec<String>,
}

/// `agent-busy` / `attachment-error` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonDetails {
    pub reason: String,
}

/// `queue-item-not-found` / `steer-unavailable` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemIdDetails {
    pub item_id: String,
}

/// `settings-rejected` / `settings-not-exposed` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceDetails {
    pub ns: String,
}

/// `settings-conflict` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsConflictDetails {
    pub ns: String,
    pub expected: i64,
    pub actual: i64,
}

/// `credential-rejected` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRefDetails {
    #[serde(rename = "ref")]
    pub reference: String,
}

/// `model-discovery-failed` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDiscoveryFailedDetails {
    pub settings_ns: String,
    #[serde(rename = "baseURL", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// `subagent-parent-unavailable` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentSessionIdDetails {
    pub parent_session_id: String,
}

/// `subagent-not-found` / `subagent-catalog-diagnostic` shared ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentPairDetails {
    pub parent_session_id: String,
    pub child_session_id: String,
}

/// `subagent-catalog-diagnostic` reason vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentCatalogReason {
    Corrupt,
    Unsupported,
    Unavailable,
}

/// `subagent-catalog-diagnostic` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentCatalogDiagnosticDetails {
    pub parent_session_id: String,
    pub child_session_id: String,
    pub reason: SubagentCatalogReason,
}

/// `subagent-not-resumable` / `subagent-unauthorized` /
/// `subagent-delivery-unavailable` details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionIdDetails {
    pub child_session_id: String,
}

/// The error body: discriminated by `code`, per-branch details aligned to
/// the TS `RpcErrorDetailsMap`; `details` is required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "kebab-case")]
pub enum RpcError {
    BadRequest(RpcErrorBody<BadRequestDetails>),
    Cancelled(RpcErrorBody<EmptyDetails>),
    SessionNotFound(RpcErrorBody<SessionIdDetails>),
    ModelUnavailable(RpcErrorBody<ModelUnavailableDetails>),
    SessionConflict(RpcErrorBody<SessionConflictDetails>),
    InvalidTimeZone(RpcErrorBody<ValueDetails>),
    WorkspaceAttachFailed(RpcErrorBody<WorkspaceAttachFailedDetails>),
    WorkspaceNotFound(RpcErrorBody<WorkspaceIdDetails>),
    WorkspaceInvalidPath(RpcErrorBody<PathDetails>),
    WorkspaceNameConflict(RpcErrorBody<NameDetails>),
    WorkspaceMoveInvalid(RpcErrorBody<WorkspaceMoveInvalidDetails>),
    DirectoryUnreadable(RpcErrorBody<PathDetails>),
    DirectoryExists(RpcErrorBody<PathDetails>),
    DirectoryCreateFailed(RpcErrorBody<PathDetails>),
    DirectoryPickerUnavailable(RpcErrorBody<CapabilityDetails>),
    AgentPresetReadOnly(RpcErrorBody<AgentPresetReasonDetails>),
    AgentPresetLocked(RpcErrorBody<AgentPresetLockedDetails>),
    AgentPresetConflict(RpcErrorBody<AgentPresetConflictDetails>),
    AgentPresetNotFound(RpcErrorBody<AgentPresetNotFoundDetails>),
    AgentPresetInvalid(RpcErrorBody<AgentPresetReasonDetails>),
    AgentBusy(RpcErrorBody<ReasonDetails>),
    AttachmentError(RpcErrorBody<ReasonDetails>),
    QueueItemNotFound(RpcErrorBody<ItemIdDetails>),
    SteerUnavailable(RpcErrorBody<ItemIdDetails>),
    CommandError(RpcErrorBody<EmptyDetails>),
    UnknownCommand(RpcErrorBody<EmptyDetails>),
    SettingsRejected(RpcErrorBody<NamespaceDetails>),
    SettingsNotExposed(RpcErrorBody<NamespaceDetails>),
    SettingsConflict(RpcErrorBody<SettingsConflictDetails>),
    CredentialRejected(RpcErrorBody<CredentialRefDetails>),
    ModelDiscoveryFailed(RpcErrorBody<ModelDiscoveryFailedDetails>),
    TitleInvalid(RpcErrorBody<SessionIdDetails>),
    ForkUnavailable(RpcErrorBody<SessionIdDetails>),
    SubagentParentUnavailable(RpcErrorBody<ParentSessionIdDetails>),
    SubagentNotFound(RpcErrorBody<SubagentPairDetails>),
    SubagentCatalogDiagnostic(RpcErrorBody<SubagentCatalogDiagnosticDetails>),
    SubagentNotResumable(RpcErrorBody<ChildSessionIdDetails>),
    SubagentUnauthorized(RpcErrorBody<ChildSessionIdDetails>),
    SubagentDeliveryUnavailable(RpcErrorBody<ChildSessionIdDetails>),
    Internal(RpcErrorBody<EmptyDetails>),
}

/// The `message` + `details` body shared by every error branch (the tag is
/// the outer enum's `code`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcErrorBody<D> {
    pub message: String,
    pub details: D,
}

impl<D> RpcErrorBody<D>
where
    D: Serialize,
{
    /// Project one typed RPC body through the shared Remote failure vocabulary.
    pub fn as_remote_error(&self, code: impl Into<String>) -> RemoteError {
        RemoteError::new(
            code,
            self.message.clone(),
            serde_json::to_value(&self.details).unwrap_or_else(|_| serde_json::json!({})),
        )
    }
}

impl RpcError {
    pub fn code(&self) -> RpcErrorCode {
        match self {
            Self::BadRequest(_) => RpcErrorCode::BadRequest,
            Self::Cancelled(_) => RpcErrorCode::Cancelled,
            Self::SessionNotFound(_) => RpcErrorCode::SessionNotFound,
            Self::ModelUnavailable(_) => RpcErrorCode::ModelUnavailable,
            Self::SessionConflict(_) => RpcErrorCode::SessionConflict,
            Self::InvalidTimeZone(_) => RpcErrorCode::InvalidTimeZone,
            Self::WorkspaceAttachFailed(_) => RpcErrorCode::WorkspaceAttachFailed,
            Self::WorkspaceNotFound(_) => RpcErrorCode::WorkspaceNotFound,
            Self::WorkspaceInvalidPath(_) => RpcErrorCode::WorkspaceInvalidPath,
            Self::WorkspaceNameConflict(_) => RpcErrorCode::WorkspaceNameConflict,
            Self::WorkspaceMoveInvalid(_) => RpcErrorCode::WorkspaceMoveInvalid,
            Self::DirectoryUnreadable(_) => RpcErrorCode::DirectoryUnreadable,
            Self::DirectoryExists(_) => RpcErrorCode::DirectoryExists,
            Self::DirectoryCreateFailed(_) => RpcErrorCode::DirectoryCreateFailed,
            Self::DirectoryPickerUnavailable(_) => RpcErrorCode::DirectoryPickerUnavailable,
            Self::AgentPresetReadOnly(_) => RpcErrorCode::AgentPresetReadOnly,
            Self::AgentPresetLocked(_) => RpcErrorCode::AgentPresetLocked,
            Self::AgentPresetConflict(_) => RpcErrorCode::AgentPresetConflict,
            Self::AgentPresetNotFound(_) => RpcErrorCode::AgentPresetNotFound,
            Self::AgentPresetInvalid(_) => RpcErrorCode::AgentPresetInvalid,
            Self::AgentBusy(_) => RpcErrorCode::AgentBusy,
            Self::AttachmentError(_) => RpcErrorCode::AttachmentError,
            Self::QueueItemNotFound(_) => RpcErrorCode::QueueItemNotFound,
            Self::SteerUnavailable(_) => RpcErrorCode::SteerUnavailable,
            Self::CommandError(_) => RpcErrorCode::CommandError,
            Self::UnknownCommand(_) => RpcErrorCode::UnknownCommand,
            Self::SettingsRejected(_) => RpcErrorCode::SettingsRejected,
            Self::SettingsNotExposed(_) => RpcErrorCode::SettingsNotExposed,
            Self::SettingsConflict(_) => RpcErrorCode::SettingsConflict,
            Self::CredentialRejected(_) => RpcErrorCode::CredentialRejected,
            Self::ModelDiscoveryFailed(_) => RpcErrorCode::ModelDiscoveryFailed,
            Self::TitleInvalid(_) => RpcErrorCode::TitleInvalid,
            Self::ForkUnavailable(_) => RpcErrorCode::ForkUnavailable,
            Self::SubagentParentUnavailable(_) => RpcErrorCode::SubagentParentUnavailable,
            Self::SubagentNotFound(_) => RpcErrorCode::SubagentNotFound,
            Self::SubagentCatalogDiagnostic(_) => RpcErrorCode::SubagentCatalogDiagnostic,
            Self::SubagentNotResumable(_) => RpcErrorCode::SubagentNotResumable,
            Self::SubagentUnauthorized(_) => RpcErrorCode::SubagentUnauthorized,
            Self::SubagentDeliveryUnavailable(_) => RpcErrorCode::SubagentDeliveryUnavailable,
            Self::Internal(_) => RpcErrorCode::Internal,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::BadRequest(body) => &body.message,
            Self::Cancelled(body) => &body.message,
            Self::SessionNotFound(body) => &body.message,
            Self::ModelUnavailable(body) => &body.message,
            Self::SessionConflict(body) => &body.message,
            Self::InvalidTimeZone(body) => &body.message,
            Self::WorkspaceAttachFailed(body) => &body.message,
            Self::WorkspaceNotFound(body) => &body.message,
            Self::WorkspaceInvalidPath(body) => &body.message,
            Self::WorkspaceNameConflict(body) => &body.message,
            Self::WorkspaceMoveInvalid(body) => &body.message,
            Self::DirectoryUnreadable(body) => &body.message,
            Self::DirectoryExists(body) => &body.message,
            Self::DirectoryCreateFailed(body) => &body.message,
            Self::DirectoryPickerUnavailable(body) => &body.message,
            Self::AgentPresetReadOnly(body) => &body.message,
            Self::AgentPresetLocked(body) => &body.message,
            Self::AgentPresetConflict(body) => &body.message,
            Self::AgentPresetNotFound(body) => &body.message,
            Self::AgentPresetInvalid(body) => &body.message,
            Self::AgentBusy(body) => &body.message,
            Self::AttachmentError(body) => &body.message,
            Self::QueueItemNotFound(body) => &body.message,
            Self::SteerUnavailable(body) => &body.message,
            Self::CommandError(body) => &body.message,
            Self::UnknownCommand(body) => &body.message,
            Self::SettingsRejected(body) => &body.message,
            Self::SettingsNotExposed(body) => &body.message,
            Self::SettingsConflict(body) => &body.message,
            Self::CredentialRejected(body) => &body.message,
            Self::ModelDiscoveryFailed(body) => &body.message,
            Self::TitleInvalid(body) => &body.message,
            Self::ForkUnavailable(body) => &body.message,
            Self::SubagentParentUnavailable(body) => &body.message,
            Self::SubagentNotFound(body) => &body.message,
            Self::SubagentCatalogDiagnostic(body) => &body.message,
            Self::SubagentNotResumable(body) => &body.message,
            Self::SubagentUnauthorized(body) => &body.message,
            Self::SubagentDeliveryUnavailable(body) => &body.message,
            Self::Internal(body) => &body.message,
        }
    }
}

/// JSON `true` literal (serde has no native boolean literal tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct True;

impl Serialize for True {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = bool::deserialize(deserializer)?;
        if value {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected true"))
        }
    }
}

/// JSON `false` literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct False;

impl Serialize for False {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for False {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = bool::deserialize(deserializer)?;
        if !value {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected false"))
        }
    }
}

/// Business success/failure result, typed value form (TS
/// `RpcResult<T>`): methods never throw business errors. Hybrids
/// (`ok: true` without a value, `ok: false` without an error) fail to
/// deserialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum RpcResult<T> {
    Ok { ok: True, value: T },
    Err { ok: False, error: RpcError },
}

impl<T> RpcResult<T> {
    pub fn ok(value: T) -> Self {
        Self::Ok { ok: True, value }
    }

    pub fn fail(error: RpcError) -> Self {
        Self::Err { ok: False, error }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Ok { value, .. } => Some(value),
            Self::Err { .. } => None,
        }
    }

    pub fn error(&self) -> Option<&RpcError> {
        match self {
            Self::Err { error, .. } => Some(error),
            Self::Ok { .. } => None,
        }
    }
}

/// Wire-wide result: the `value` slot is optional because a void business
/// result serializes with no `value` field at all (TS `rpcResultSchema(z
/// .unknown().optional())`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum WireRpcResult {
    Ok {
        ok: True,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<serde_json::Value>,
    },
    Err {
        ok: False,
        error: RpcError,
    },
}

/// Fold a transport exception into the error branch (unified error API;
/// `internal` as the catch-all code). The Rust counterpart of
/// `transportError` takes the thrown value's message directly.
pub fn transport_error<T>(message: impl Into<String>) -> RpcResult<T> {
    RpcResult::fail(RpcError::Internal(RpcErrorBody {
        message: message.into(),
        details: EmptyDetails {},
    }))
}

/// Signature-layer narrow form, request side: rpcId is explicit, never
/// mixed into the business payload; the type tag and method are filled in
/// by the carrier layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcRequest<P> {
    pub rpc_id: RpcId,
    pub payload: P,
}

/// Signature-layer narrow form, response side: rpcId always echoes the
/// matching request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResponse<T> {
    pub rpc_id: RpcId,
    pub result: RpcResult<T>,
}

/// Wire full form: call initiated by the client (carrier: POST
/// /api/<method> body).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientRequest {
    #[serde(rename = "type")]
    pub kind: ClientRequestType,
    pub rpc_id: RpcId,
    pub method: String,
    pub payload: serde_json::Value,
}

/// Discriminant literal of [`ClientRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientRequestType {
    #[serde(rename = "client-request")]
    ClientRequest,
}

/// Wire full form: response to a ClientRequest (carrier: the HTTP response
/// body of that POST); rpcId echoed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerResponse {
    #[serde(rename = "type")]
    pub kind: ServerResponseType,
    pub rpc_id: RpcId,
    pub result: WireRpcResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerResponseType {
    #[serde(rename = "server-response")]
    ServerResponse,
}

/// Wire full form: message initiated by the server (carrier: downstream
/// stream frame).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerRequest {
    #[serde(rename = "type")]
    pub kind: ServerRequestType,
    pub rpc_id: RpcId,
    pub method: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerRequestType {
    #[serde(rename = "server-request")]
    ServerRequest,
}

/// Wire full form: response to a ServerRequest (carrier: POST /api/respond
/// body); rpcId echoed, never minted anew.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientResponse {
    #[serde(rename = "type")]
    pub kind: ClientResponseType,
    pub rpc_id: RpcId,
    pub result: WireRpcResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientResponseType {
    #[serde(rename = "client-response")]
    ClientResponse,
}

/// Authoritative wire full-form union; narrow via `match (message)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RpcMessage {
    ClientRequest {
        #[serde(rename = "rpcId")]
        rpc_id: RpcId,
        method: String,
        payload: serde_json::Value,
    },
    ServerResponse {
        #[serde(rename = "rpcId")]
        rpc_id: RpcId,
        result: WireRpcResult,
    },
    ServerRequest {
        #[serde(rename = "rpcId")]
        rpc_id: RpcId,
        method: String,
        payload: serde_json::Value,
    },
    ClientResponse {
        #[serde(rename = "rpcId")]
        rpc_id: RpcId,
        result: WireRpcResult,
    },
}

/// Rejection reason of a late/duplicate client response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RpcReceiptReason {
    NotPending,
    BadResponse,
}

/// Carrier receipt (not an `RpcMessage` — it belongs to the carrier layer,
/// same discipline as "HTTP status describes only the carrier").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum RpcReceipt {
    Accepted {
        accepted: True,
    },
    Rejected {
        accepted: False,
        reason: RpcReceiptReason,
    },
}
