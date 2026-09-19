//! Workspace entity registry (`ctx.workspaceRegistry`). Rust port of
//! `@deepseek-ai/dsh-workspace`.

pub mod entity;
pub mod index;
pub mod invariant;
pub mod paths;
pub mod spec;
pub mod types;

pub use entity::{WorkspaceEntity, WorkspaceEntityHost, WorkspaceMoveInvalidError};
pub use index::{
    LiveSessionStore, SessionDeleteFn, StoreLiveSessions, WorkspaceAggregateError,
    WorkspaceOrderInvalidError, WorkspaceRegistry, WorkspaceSessionLiveError,
    WorkspaceSessionNotArchivedError, WorkspaceUnknownSessionError,
};
pub use paths::realpath_normalize;
pub use spec::{
    WorkspaceDomainState, WorkspacePendingMutation, WorkspaceRecord, record_from_value,
    state_from_value, workspace_domain_spec, workspace_domain_state_schema,
    workspace_record_schema,
};
pub use types::{Workspace, WorkspaceId, WorkspaceIdTag, workspace_id};

/// Verifies provider-owned remote workspace identities without treating them as local paths.
pub trait WorkspaceExecutionProvider: Send + Sync + 'static {
    fn verify(&self, uri: String) -> futures::future::BoxFuture<'static, Result<String, String>>;
}
impl cordis::Service for dyn WorkspaceExecutionProvider {
    fn service_name(&self) -> &'static str {
        "workspaceExecutionProvider"
    }
}
