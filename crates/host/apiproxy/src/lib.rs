//! Host API gateway: four-quadrant RPC message model and the closed
//! client-request method registry. Rust port of
//! `packages/host/apiproxy` — contract layer first; the carrier layer
//! (`fetch/handler` + `fetch/client`) and the `createApiProxy` composition
//! follow in later milestones.

pub mod agent_lookup;
pub mod api;
pub mod capabilities;
pub mod fetch;
mod interactions;
pub mod native_path_opener;
pub mod proxy;
pub mod session_export;

pub use agent_lookup::{
    AgentResolver, ApiRemoteAgentOptions, ApiRemoteAgentResult, has_api_remote_subagent_owner,
};
pub use api::agent_presets::{
    AgentPresetCopyRequest, AgentPresetEntry, AgentPresetListResult,
    AgentPresetOpenDocumentRequest, AgentPresetOpenDocumentResult, AgentPresetReadRequest,
    AgentPresetReadResult, AgentPresetRemoveRequest, AgentPresetSelectRequest,
    AgentPresetSelectResult, AgentPresetTrust, AgentPresetsApi,
};
pub use api::approvals::{ApprovalClientOutcome, ApprovalResponsePayload};
pub use api::credentials::{
    CredentialView, CredentialsApi, CredentialsDescribeRequest, CredentialsDescribeResult,
    CredentialsSetRequest, CredentialsUnsetRequest,
};
pub use api::downloads::{DownloadsApi, SessionLogRequest};
pub use api::events::{
    EventsApi, HostFrame, HostSessionOrigin, MuxFrame, QuestionOutcome, QueuedInboxItem,
    QueuedInboxPlacement, ToolEventView,
};
pub use api::goals::{
    GoalClearRequest, GoalClearResult, GoalCreateRequest, GoalEditRequest, GoalId, GoalRef,
    GoalRefResult, GoalVerbRequest, GoalsApi,
};
pub use api::host::{
    DirectoryEntry, DirectoryListing, HostApi, HostCreateDirectoryRequest,
    HostCreateDirectoryResult, HostDescribeResult, HostListDirectoryRequest, HostOpenPathRequest,
    HostOpenPathResult, HostPickDirectoryResult,
};
pub use api::jobs::{JobStatus, JobView};
pub use api::llm::{
    ConfigurableProviderView, DiscoveredModelView, LlmApi, LlmDiscoverModelsRequest,
    LlmDiscoverModelsResult, LlmModelsResult, LlmProvidersResult,
};
pub use api::questions::QuestionResponsePayload;
pub use api::rpc::{
    AgentPresetConflictDetails, AgentPresetLockedDetails, AgentPresetNotFoundDetails,
    AgentPresetReasonDetails, BadRequestDetails, CapabilityDetails, ChildSessionIdDetails,
    ClientRequest, ClientRequestType, ClientResponse, ClientResponseType, CredentialRefDetails,
    EmptyDetails, False, ItemIdDetails, ModelDiscoveryFailedDetails, ModelUnavailableDetails,
    NameDetails, NamespaceDetails, ParentSessionIdDetails, PathDetails, ReasonDetails, RpcError,
    RpcErrorBody, RpcErrorCode, RpcId, RpcMessage, RpcReceipt, RpcReceiptReason, RpcRequest,
    RpcResponse, RpcResult, ServerRequest, ServerRequestType, ServerResponse, ServerResponseType,
    SessionConflictDetails, SessionIdDetails, SettingsConflictDetails,
    SubagentCatalogDiagnosticDetails, SubagentCatalogReason, SubagentPairDetails, True,
    ValueDetails, WireRpcResult, WorkspaceAttachFailedDetails, WorkspaceIdDetails,
    WorkspaceMoveInvalidDetails, rpc_id, transport_error,
};
pub use api::rpc_map::{CLIENT_REQUEST_METHODS, is_client_request_method};
pub use api::sessions::{
    AcceptedResult, HistoryEntry, ModelCatalogFailure, ModelCatalogModel, ModelProviderGroup,
    ModelReasoning, ModelReasoningEffort, ModelSelection, PromptCommandKind, PromptCommandSlot,
    PromptContentPart, PromptMode, QueueAction, SessionAttachmentRequest, SessionAttachmentResult,
    SessionCreateRequest, SessionCreateResult, SessionForkRequest, SessionForkResult,
    SessionHistoryRequest, SessionHistoryResult, SessionListMetadata, SessionListRequest,
    SessionListResult, SessionModels, SessionOrigin, SessionProjectionsBlock, SessionPromptRequest,
    SessionPromptResult, SessionRefRequest, SessionRenameRequest, SessionRenameResult,
    SessionSearchItem, SessionSearchRequest, SessionSearchResult, SessionSelectModelRequest,
    SessionSelectModelResult, SessionSummary, SessionUpdateQueueRequest, SessionUpdateTodosRequest,
    SessionsApi, TodoAction,
};
pub use api::settings::{
    SettingsApi, SettingsApplies, SettingsDescribeResult, SettingsMutateRequest,
    SettingsNamespaceView, SettingsOpenDocumentResult, SettingsPathOpView, SettingsReplaceRequest,
    SettingsSecretView, SettingsUpdateRequest,
};
pub use api::skills::{SkillEntry, SkillListRequest, SkillListResult, SkillsApi};
pub use api::subagents::{
    SubagentActivity, SubagentAddress, SubagentCatalog, SubagentDiagnosticReason,
    SubagentHistoryRequest, SubagentHistoryResult, SubagentInterruptReceipt,
    SubagentInterruptRequest, SubagentListEntry, SubagentListRequest, SubagentMode,
    SubagentPromptReceipt, SubagentPromptRequest, SubagentsApi,
};
pub use api::workspace::{WorkspaceId, WorkspaceView};
pub use fetch::handler::{
    AbortSignal, ApiProxyCarrier, Body, CarrierRequest, CarrierResponse, DownloadResponse,
    FetchHandler, FrameRequest, SessionLogQuery, to_fetch_handler,
};
pub use proxy::{ApiProxyDefaults, ApiProxyService, HOST_VERSION};
