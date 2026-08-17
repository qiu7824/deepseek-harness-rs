//! Package-private workspace entity: the single `Workspace`
//! implementation. Rust port of
//! `packages/workspace/workspace/src/entity.ts`. Holds a record snapshot
//! swapped in place after each durable mutation; every write funnels
//! through the private `mutate` so `updatedAt` stamping and
//! invalid-account pruning happen exactly once.
//!
//! # Deviations
//!
//! - The TS unchanged sentinel is an `Error` thrown inside the domain
//!   update closure; the Rust port panics with a dedicated payload
//!   (`panic_any`) and `mutate` catches it around the update call (the
//!   domain update closure has no error channel).
//! - `updatedAt` stamps with `chrono` UTC ISO-8601 (millisecond precision,
//!   matching `Date.toISOString`).

use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::Mutex;

use dsh_session::{SessionHeader, SessionId};

use crate::paths::realpath_normalize;
use crate::spec::{WorkspaceRecord, record_from_value};
use crate::types::WorkspaceId;

/// An insertSessionBefore request named a session or anchor not on the
/// account (TS `WorkspaceMoveInvalidError`).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceMoveInvalidError {
    pub message: String,
}

impl WorkspaceMoveInvalidError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl std::fmt::Display for WorkspaceMoveInvalidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WorkspaceMoveInvalidError {}

/// The registry-owned machinery an entity mutates through (TS
/// `WorkspaceEntityHost`).
pub trait WorkspaceEntityHost: Send + Sync {
    /// Resolve the open `workspaces` table.
    fn table(&self) -> Arc<dyn dsh_storage_domain::KvTable>;

    /// Read a session's canonical directory from the registry's header
    /// index.
    fn session_path(&self, id: &SessionId) -> Option<String>;

    /// Read one stored session header for attach validation.
    fn read_session_header(
        &self,
        id: &SessionId,
    ) -> BoxFuture<'static, Result<SessionHeader, String>>;

    /// Publish a successfully validated canonical cwd to the projection
    /// index.
    fn remember_session_path(&self, id: &SessionId, path: &str);
}

/// Chain-slot abort sentinel (the TS `unchangedSentinel`): `mutate` only
/// observes it.
pub(crate) struct UnchangedSentinel;

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The single `Workspace` implementation; constructed only by the
/// registry.
pub struct WorkspaceEntity {
    pub id: WorkspaceId,
    record: Mutex<WorkspaceRecord>,
    host: Arc<dyn WorkspaceEntityHost>,
}

impl WorkspaceEntity {
    pub fn new(
        host: Arc<dyn WorkspaceEntityHost>,
        id: WorkspaceId,
        record: WorkspaceRecord,
    ) -> Self {
        Self { id, record: Mutex::new(record), host }
    }

    pub fn path(&self) -> String {
        self.record.lock().path.clone()
    }

    pub fn title(&self) -> String {
        self.record.lock().title.clone()
    }

    pub fn created_at(&self) -> String {
        self.record.lock().created_at.clone()
    }

    pub fn updated_at(&self) -> String {
        self.record.lock().updated_at.clone()
    }

    /// Header-validated sessions (the TS `sessionIds` getter).
    pub fn session_ids(&self) -> Vec<SessionId> {
        let record = self.record.lock();
        record
            .session_ids
            .iter()
            .filter(|id| self.host.session_path(id).as_deref() == Some(record.path.as_str()))
            .cloned()
            .collect()
    }

    /// The raw durable account (unfiltered).
    pub fn durable_session_ids(&self) -> Vec<SessionId> {
        self.record.lock().session_ids.clone()
    }

    /// The full record snapshot.
    pub fn record(&self) -> WorkspaceRecord {
        self.record.lock().clone()
    }

    /// Replace the snapshot after a durable write.
    pub fn set_record(&self, record: WorkspaceRecord) {
        *self.record.lock() = record;
    }

    pub async fn set_title(&self, title: &str) -> Result<(), String> {
        let title = title.to_string();
        self.mutate(move |record| {
            let mut next = record;
            next.title = title.clone();
            Ok(Some(next))
        })
        .await
    }

    pub async fn attach_session(&self, session_id: &SessionId) -> Result<(), String> {
        // Validation is skipped when the settled snapshot already accounts
        // the id.
        if !self.record.lock().session_ids.contains(session_id) {
            let header = self.host.read_session_header(session_id).await?;
            let Some(header_cwd) = header.cwd.as_deref() else {
                return Err(format!(
                    "cannot attach session '{session_id}' to workspace '{}': \
                     its stored header carries no cwd to validate against",
                    self.record.lock().path
                ));
            };
            let cwd = realpath_normalize(header_cwd).await.map_err(|error| {
                format!(
                    "cannot attach session '{session_id}' to workspace '{}': \
                     its cwd '{header_cwd}' does not resolve, so it cannot be validated ({error})",
                    self.record.lock().path
                )
            })?;
            if !tokio::fs::metadata(&cwd).await.is_ok_and(|meta| meta.is_dir()) {
                return Err(format!(
                    "cannot attach session '{session_id}' to workspace '{}': \
                     its cwd '{header_cwd}' is not a directory",
                    self.record.lock().path
                ));
            }
            if cwd != self.record.lock().path {
                return Err(format!(
                    "cannot attach session '{session_id}' to workspace '{}': \
                     its cwd resolves to '{cwd}'",
                    self.record.lock().path
                ));
            }
            self.host.remember_session_path(session_id, &cwd);
        }
        let session_id = session_id.clone();
        self.mutate(move |record| {
            if record.session_ids.contains(&session_id) {
                Ok(None)
            } else {
                let mut next = record;
                next.session_ids.insert(0, session_id.clone());
                Ok(Some(next))
            }
        })
        .await
    }

    pub async fn insert_session_before(
        &self,
        session_id: &SessionId,
        before_session_id: Option<&SessionId>,
    ) -> Result<(), String> {
        let session_id = session_id.clone();
        let before_session_id = before_session_id.cloned();
        self.mutate(move |record| {
            if !record.session_ids.contains(&session_id) {
                return Err(format!(
                    "cannot move session '{session_id}' in workspace '{}': the session is not accounted",
                    record.path
                ));
            }
            if let Some(anchor) = &before_session_id {
                if !record.session_ids.contains(anchor) {
                    return Err(format!(
                        "cannot move session '{session_id}' before '{anchor}' in workspace '{}': \
                         the anchor session is not accounted",
                        record.path
                    ));
                }
            }
            if before_session_id.as_ref() == Some(&session_id) {
                return Ok(None);
            }
            let without: Vec<SessionId> = record
                .session_ids
                .iter()
                .filter(|id| *id != &session_id)
                .cloned()
                .collect();
            let at = match &before_session_id {
                None => without.len(),
                Some(anchor) => without
                    .iter()
                    .position(|id| id == anchor)
                    .expect("anchor accounted above"),
            };
            let mut session_ids = without;
            session_ids.insert(at, session_id.clone());
            if session_ids == record.session_ids {
                Ok(None)
            } else {
                let mut next = record;
                next.session_ids = session_ids;
                Ok(Some(next))
            }
        })
        .await
    }

    pub async fn detach_session(&self, session_id: &SessionId) -> Result<(), String> {
        let session_id = session_id.clone();
        self.mutate(move |record| {
            if !record.session_ids.contains(&session_id) {
                return Ok(None);
            }
            let mut next = record;
            next.session_ids.retain(|id| *id != session_id);
            Ok(Some(next))
        })
        .await
    }

    pub async fn status(&self) -> Result<&'static str, String> {
        let path = self.record.lock().path.clone();
        match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_dir() => Ok("ok"),
            _ => Ok("missing-dir"),
        }
    }

    /// The single write path (TS `mutate`): run `fn` on the domain write
    /// chain, stamping `updatedAt` and pruning candidates that no longer
    /// pass the id-plus-canonical-cwd membership check, then swap the
    /// snapshot. `Ok(None)` is the TS "return `current` verbatim" signal:
    /// the slot aborts without writing unless pruning demands one.
    async fn mutate<F>(&self, f: F) -> Result<(), String>
    where
        F: Fn(WorkspaceRecord) -> Result<Option<WorkspaceRecord>, String> + Send + Sync + 'static,
    {
        let host = self.host.clone();
        let id = self.id.clone();
        let closure: Arc<dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync> =
            Arc::new(move |current_value| {
                let current = record_from_value(&current_value)
                    .expect("stored workspace record parses");
                let changed = match f(current.clone()) {
                    Ok(Some(changed)) => changed,
                    Ok(None) => current.clone(),
                    Err(error) => std::panic::panic_any(error),
                };
                let session_ids: Vec<SessionId> = changed
                    .session_ids
                    .iter()
                    .filter(|session_id| {
                        host.session_path(session_id).as_deref() == Some(changed.path.as_str())
                    })
                    .cloned()
                    .collect();
                if changed == current && session_ids.len() == current.session_ids.len() {
                    std::panic::panic_any(UnchangedSentinel);
                }
                let mut next = changed;
                next.session_ids = session_ids;
                next.updated_at = now_iso();
                serde_json::to_value(next).expect("workspace record serializes")
            });
        let table = self.host.table();
        let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            table.update(&id, closure),
        ))
        .await;
        match outcome {
            Ok(Ok(value)) => {
                let next = record_from_value(&value)?;
                self.set_record(next);
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(payload) => match payload.downcast::<String>() {
                Ok(message) => Err(*message),
                Err(payload) => match payload.downcast::<UnchangedSentinel>() {
                    Ok(_) => Ok(()),
                    Err(payload) => std::panic::resume_unwind(payload),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::task::{Context as TaskContext, Poll};

    use futures::future::BoxFuture;
    use futures::task::noop_waker_ref;

    use super::*;
    use crate::types::workspace_id;

    struct UnusedHost;

    impl WorkspaceEntityHost for UnusedHost {
        fn table(&self) -> Arc<dyn dsh_storage_domain::KvTable> {
            panic!("status does not access storage")
        }

        fn session_path(&self, _id: &SessionId) -> Option<String> {
            None
        }

        fn read_session_header(
            &self,
            _id: &SessionId,
        ) -> BoxFuture<'static, Result<SessionHeader, String>> {
            Box::pin(async { Err("unused".to_string()) })
        }

        fn remember_session_path(&self, _id: &SessionId, _path: &str) {}
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_releases_record_lock_before_filesystem_await() {
        let entity = WorkspaceEntity::new(
            Arc::new(UnusedHost),
            workspace_id("status-lock"),
            WorkspaceRecord {
                path: "D:\\definitely-missing-dsh-workspace-status".to_string(),
                title: "status".to_string(),
                session_ids: Vec::new(),
                created_at: "2026-01-01T00:00:00.000Z".to_string(),
                updated_at: "2026-01-01T00:00:00.000Z".to_string(),
            },
        );
        let mut status = Box::pin(entity.status());
        let mut task_context = TaskContext::from_waker(noop_waker_ref());

        assert!(matches!(status.as_mut().poll(&mut task_context), Poll::Pending));
        assert!(
            entity.record.try_lock().is_some(),
            "status must release the record lock before awaiting filesystem I/O"
        );
    }
}
