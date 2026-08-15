//! Public type vocabulary of the workspace entity. Rust port of
//! `packages/workspace/workspace/src/types.ts`: the `WorkspaceId` brand and
//! the `Workspace` consumer surface (the TS interface collapses into a thin
//! handle over the single [`crate::entity::WorkspaceEntity`]
//! implementation).

use std::sync::Arc;

use dsh_brand::Branded;
use dsh_session::SessionId;

/// Marker for the workspace id brand.
#[doc(hidden)]
#[allow(dead_code)]
pub enum WorkspaceIdTag {}

/// Identifies one workspace record (TS `WorkspaceId`). A generated uuid,
/// never the path: path normalization rewrites paths, and a reference
/// anchor must stay stable.
pub type WorkspaceId = Branded<WorkspaceIdTag>;

/// Brand a string as a [`WorkspaceId`].
pub fn workspace_id(id: impl Into<String>) -> WorkspaceId {
    WorkspaceId::new(id)
}

/// One workspace: a stable id over an existing directory, a display title,
/// and an ordered candidate account of sessions (TS `Workspace`).
#[derive(Clone)]
pub struct Workspace {
    pub(crate) entity: Arc<crate::entity::WorkspaceEntity>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("id", &self.entity.id)
            .field("path", &self.entity.path())
            .finish()
    }
}

impl PartialEq for Workspace {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.entity, &other.entity)
    }
}

impl Workspace {
    pub(crate) fn new(entity: Arc<crate::entity::WorkspaceEntity>) -> Self {
        Self { entity }
    }

    /// Stable record id (generated uuid).
    pub fn id(&self) -> &WorkspaceId {
        &self.entity.id
    }

    /// Canonical directory path (never rewritten afterwards).
    pub fn path(&self) -> String {
        self.entity.path()
    }

    /// Display title.
    pub fn title(&self) -> String {
        self.entity.title()
    }

    /// ISO-8601 creation instant.
    pub fn created_at(&self) -> String {
        self.entity.created_at()
    }

    /// ISO-8601 instant of the last durable mutation.
    pub fn updated_at(&self) -> String {
        self.entity.updated_at()
    }

    /// Header-validated sessions in manually owned order (TS `sessionIds`).
    pub fn session_ids(&self) -> Vec<SessionId> {
        self.entity.session_ids()
    }

    /// Replace the display title durably (TS `setTitle`).
    pub async fn set_title(&self, title: &str) -> Result<(), String> {
        self.entity.set_title(title).await
    }

    /// Prepend a session to this workspace's candidate account (TS
    /// `attachSession`).
    pub async fn attach_session(&self, session_id: &SessionId) -> Result<(), String> {
        self.entity.attach_session(session_id).await
    }

    /// Move an accounted session within the manual order (TS
    /// `insertSessionBefore`).
    pub async fn insert_session_before(
        &self,
        session_id: &SessionId,
        before_session_id: Option<&SessionId>,
    ) -> Result<(), String> {
        self.entity
            .insert_session_before(session_id, before_session_id)
            .await
    }

    /// Remove a session from this workspace's account (TS `detachSession`).
    pub async fn detach_session(&self, session_id: &SessionId) -> Result<(), String> {
        self.entity.detach_session(session_id).await
    }

    /// Live directory check, uncached (TS `status`).
    pub async fn status(&self) -> Result<&'static str, String> {
        self.entity.status().await
    }
}
