//! Workspace entity registry (`ctx.workspaceRegistry`). Rust port of
//! `packages/workspace/workspace/src/index.ts`: durable workspace records,
//! stable registry order, and header-validated session membership over the
//! domain data form.
//!
//! # Deviations
//!
//! - The TS `Service.init` (async open + one-time history bootstrap) runs
//!   inside [`WorkspaceRegistry::install`] via a one-time `block_on` (the
//!   install boundary performs the open/list I/O).
//! - The TS inject-gated startup (pending until `storageDomain` +
//!   `sessionPersistence` exist) collapses into explicit install
//!   parameters; the `not started` rejection surface shrinks to
//!   construction-time errors.
//! - The live-session peer is a seam-local [`LiveSessionStore`] trait
//!   (the real [`dsh_session::SessionStore`] adapts through
//!   [`StoreLiveSessions`]); the TS reads `ctx.sessions` dynamically.
//! - `sessionPersistence.delete` is not ported on the Rust persistence
//!   seam yet: [`WorkspaceRegistry::delete_archived_session`] requires it
//!   through a caller-supplied closure until the backend milestone lands.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::Mutex;

use cordis::{Context, Service, arc};
use dsh_session::{SessionHeader, SessionId, SessionStore};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_storage_domain::{Domain, DomainFacility, KvTable};

use crate::entity::{WorkspaceEntity, WorkspaceEntityHost};
use crate::paths::realpath_normalize;
use crate::spec::{
    WorkspaceDomainState, WorkspacePendingMutation, WorkspaceRecord, record_from_value,
    state_from_value, workspace_domain_spec,
};
use crate::types::{Workspace, WorkspaceId, workspace_id};

/// An archiveSession request named a session neither live nor in session
/// persistence (TS `WorkspaceUnknownSessionError`).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceUnknownSessionError {
    pub session_id: SessionId,
}

impl std::fmt::Display for WorkspaceUnknownSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot archive session '{}': live sessions and session persistence hold no such session",
            self.session_id
        )
    }
}

impl std::error::Error for WorkspaceUnknownSessionError {}

/// Permanent deletion named a session outside the archive set (TS
/// `WorkspaceSessionNotArchivedError`).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceSessionNotArchivedError {
    pub session_id: SessionId,
}

impl std::fmt::Display for WorkspaceSessionNotArchivedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot permanently delete session '{}': it is not archived",
            self.session_id
        )
    }
}

impl std::error::Error for WorkspaceSessionNotArchivedError {}

/// Permanent deletion named a session whose live lifecycle still owns it
/// (TS `WorkspaceSessionLiveError`).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceSessionLiveError {
    pub session_id: SessionId,
    pub message: String,
}

impl WorkspaceSessionLiveError {
    fn new(session_id: &SessionId) -> Self {
        Self {
            session_id: session_id.clone(),
            message: format!("cannot permanently delete session '{session_id}' while it is live"),
        }
    }

    fn not_detached(session_id: &SessionId) -> Self {
        Self {
            session_id: session_id.clone(),
            message: format!(
                "cannot permanently delete session '{session_id}': its live lifecycle did not fully detach"
            ),
        }
    }
}

impl std::fmt::Display for WorkspaceSessionLiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WorkspaceSessionLiveError {}

/// A workspace reorder named a source or anchor absent from the durable
/// registry order (TS `WorkspaceOrderInvalidError`).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceOrderInvalidError {
    pub workspace_id: WorkspaceId,
}

impl std::fmt::Display for WorkspaceOrderInvalidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot reorder unknown workspace '{}'",
            self.workspace_id
        )
    }
}

impl std::error::Error for WorkspaceOrderInvalidError {}

/// Two failures reported together (the TS `AggregateError` collapse).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceAggregateError {
    pub message: String,
}

impl WorkspaceAggregateError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WorkspaceAggregateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WorkspaceAggregateError {}

/// The live-session peer surface the registry consults (TS `ctx.sessions`
/// usage collapsed).
pub trait LiveSessionStore: Send + Sync {
    fn get(&self, id: &SessionId) -> Option<SessionHeader>;
    fn list(&self) -> Vec<SessionHeader>;
}

/// Adapt the real [`SessionStore`] onto [`LiveSessionStore`].
pub struct StoreLiveSessions(pub Arc<SessionStore>);

impl LiveSessionStore for StoreLiveSessions {
    fn get(&self, id: &SessionId) -> Option<SessionHeader> {
        self.0.get(id).map(|session| session.header().clone())
    }

    fn list(&self) -> Vec<SessionHeader> {
        self.0
            .list()
            .iter()
            .map(|session| session.header().clone())
            .collect()
    }
}

/// The persistent-session deletion closure (the unported
/// `sessionPersistence.delete` seam; production wires the backend once the
/// persistence delete lands).
pub type SessionDeleteFn =
    Arc<dyn Fn(&SessionId) -> BoxFuture<'static, Result<bool, String>> + Send + Sync>;

fn box_future<T: Send + 'static>(
    future: impl futures::Future<Output = T> + Send + 'static,
) -> BoxFuture<'static, T> {
    Box::pin(future)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

struct BootstrapGroup {
    path: String,
    headers: Vec<SessionHeader>,
    newest_at: u64,
}

fn same_ids(left: &[WorkspaceId], right: &[WorkspaceId]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a == b)
}

fn same_session_ids(left: &[SessionId], right: &[SessionId]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a == b)
}

/// Shared machinery behind the entity host (the registry and the entities
/// share the indexer's maps — no reference cycle).
struct RegistryHost {
    table: Arc<dyn KvTable>,
    persistence: Arc<dyn SessionPersistenceApi>,
    live: Option<Arc<dyn LiveSessionStore>>,
    indexer: Arc<RegistryHostIndexer>,
}

/// A shareable header index: canonical session paths, invalid-cwd reasons,
/// and the header table itself.
struct RegistryHostIndexer {
    headers: Arc<Mutex<HashMap<SessionId, SessionHeader>>>,
    session_paths: Arc<Mutex<HashMap<SessionId, String>>>,
    invalid_session_paths: Arc<Mutex<HashMap<SessionId, String>>>,
}

impl RegistryHostIndexer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            headers: Arc::new(Mutex::new(HashMap::new())),
            session_paths: Arc::new(Mutex::new(HashMap::new())),
            invalid_session_paths: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn index_header(&self, header: &SessionHeader) {
        self.headers
            .lock()
            .insert(header.id.clone(), header.clone());
        self.session_paths.lock().remove(&header.id);
        let Some(cwd) = header.cwd.as_deref() else {
            self.invalid_session_paths
                .lock()
                .insert(header.id.clone(), "header has no cwd".to_string());
            return;
        };
        match realpath_normalize(cwd).await {
            Ok(path) => {
                if tokio::fs::metadata(&path)
                    .await
                    .is_ok_and(|meta| meta.is_dir())
                {
                    self.session_paths.lock().insert(header.id.clone(), path);
                    self.invalid_session_paths.lock().remove(&header.id);
                } else {
                    self.invalid_session_paths
                        .lock()
                        .insert(header.id.clone(), format!("cwd '{cwd}' is not a directory"));
                }
            }
            Err(_) => {
                self.invalid_session_paths
                    .lock()
                    .insert(header.id.clone(), format!("cwd '{cwd}' does not resolve"));
            }
        }
    }

    async fn index_headers(&self, headers: &[SessionHeader]) {
        for header in headers {
            self.index_header(header).await;
        }
    }

    async fn replace_header_index(&self, headers: &[SessionHeader]) {
        self.headers.lock().clear();
        self.session_paths.lock().clear();
        self.invalid_session_paths.lock().clear();
        self.index_headers(headers).await;
    }
}

#[async_trait::async_trait]
impl WorkspaceEntityHost for RegistryHost {
    fn table(&self) -> Arc<dyn KvTable> {
        self.table.clone()
    }

    fn session_path(&self, id: &SessionId) -> Option<String> {
        self.indexer.session_paths.lock().get(id).cloned()
    }

    fn read_session_header(
        &self,
        id: &SessionId,
    ) -> BoxFuture<'static, Result<SessionHeader, String>> {
        let id = id.clone();
        let live = self.live.clone();
        let indexer = Arc::clone(&self.indexer);
        let persistence = self.persistence.clone();
        Box::pin(async move {
            if let Some(live) = &live
                && let Some(header) = live.get(&id)
            {
                indexer.headers.lock().insert(id.clone(), header.clone());
                return Ok(header);
            }
            if let Some(cached) = indexer.headers.lock().get(&id) {
                return Ok(cached.clone());
            }
            let listed = persistence.list().await?;
            indexer.index_headers(&listed).await;
            indexer.headers.lock().get(&id).cloned().ok_or_else(|| {
                format!("cannot validate session '{id}': session persistence holds no such session")
            })
        })
    }

    fn remember_session_path(&self, id: &SessionId, path: &str) {
        self.indexer
            .session_paths
            .lock()
            .insert(id.clone(), path.to_string());
        self.indexer.invalid_session_paths.lock().remove(id);
    }
}

/// Durable workspace registry.
pub struct WorkspaceRegistry {
    ctx: Context,
    domain: Arc<Domain>,
    table: Arc<dyn KvTable>,
    host: Arc<RegistryHost>,
    state: Mutex<WorkspaceDomainState>,
    entities: Mutex<HashMap<String, Arc<WorkspaceEntity>>>,
    operation_tail: Arc<tokio::sync::Mutex<()>>,
    session_delete: SessionDeleteFn,
}

impl Service for WorkspaceRegistry {
    fn service_name(&self) -> &'static str {
        "workspaceRegistry"
    }
}

impl WorkspaceRegistry {
    /// Open the domain, finish bootstrap when required, and rebuild the
    /// ordered cache (TS constructor + `Service.init`). The one-time
    /// open/list I/O runs at this boundary.
    pub fn install(
        ctx: &Context,
        facility: &Arc<DomainFacility>,
        persistence: Arc<dyn SessionPersistenceApi>,
        live: Option<Arc<dyn LiveSessionStore>>,
        session_delete: SessionDeleteFn,
    ) -> Result<Arc<Self>, String> {
        let domain = futures::executor::block_on(facility.open(&workspace_domain_spec()))?;
        let table = domain.table("workspaces");
        let host = Arc::new(RegistryHost {
            table: table.clone(),
            persistence: persistence.clone(),
            live,
            indexer: RegistryHostIndexer::new(),
        });
        let registry = Arc::new(Self {
            ctx: ctx.clone(),
            domain: domain.clone(),
            table,
            host,
            state: Mutex::new(WorkspaceDomainState::initial()),
            entities: Mutex::new(HashMap::new()),
            operation_tail: Arc::new(tokio::sync::Mutex::new(())),
            session_delete,
        });
        ctx.register_service(registry.clone());

        // Close the domain with the plugin (TS `workspace.domainClose`).
        let domain_for_dispose = domain.clone();
        let _ = ctx.effect(
            "workspace.domainClose",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let domain = domain_for_dispose.clone();
                    Box::pin(async move {
                        domain.close().await;
                    })
                }))
            }),
        );

        // Startup sequence (TS `Service.init`).
        let state = state_from_value(&domain.global().get())?;
        *registry.state.lock() = state.clone();
        futures::executor::block_on(registry.recover_pending_mutation())?;
        registry.validate_stored_state(&state)?;
        if !state.initialized {
            let headers = futures::executor::block_on(persistence.list())?;
            futures::executor::block_on(registry.host.indexer.replace_header_index(&headers));
            futures::executor::block_on(registry.bootstrap(&headers))?;
        }
        let current = registry.require_state();
        registry.validate_stored_state(&current)?;
        registry.rebuild_entities()?;
        if state.initialized {
            registry.seed_paths_from_durable_records();
        }
        if let Some(live) = &registry.host.live {
            futures::executor::block_on(registry.host.indexer.index_headers(&live.list()));
        }
        registry.report_filtered_candidates();
        Ok(registry)
    }

    /// Create or reuse a workspace for an existing directory (TS `create`).
    pub async fn create(
        self: &Arc<Self>,
        path: &str,
        title: Option<&str>,
    ) -> Result<Workspace, String> {
        let canonical = realpath_normalize(path)
            .await
            .map_err(|error| format!("cannot create a workspace at '{path}': {error}"))?;
        if !tokio::fs::metadata(&canonical)
            .await
            .is_ok_and(|meta| meta.is_dir())
        {
            return Err(format!(
                "cannot create a workspace at '{canonical}': path is not a directory"
            ));
        }
        let canonical = canonical.clone();
        let title = title.map(|title| title.to_string());
        let registry = Arc::clone(self);
        self.enqueue_operation(move || {
            let canonical = canonical.clone();
            let title = title.clone();
            let registry = registry.clone();
            box_future(async move {
                registry
                    .create_canonical(&canonical, title.as_deref())
                    .await
            })
        })
        .await
    }

    /// Look up a workspace by id (TS `get`).
    pub fn get(&self, id: &WorkspaceId) -> Option<Workspace> {
        self.entities
            .lock()
            .get(id.as_str())
            .map(|entity| Workspace::new(entity.clone()))
    }

    /// Synchronous workspace projection in durable registry order (TS
    /// `list`).
    pub fn list(&self) -> Result<Vec<Workspace>, String> {
        let state = self.require_state();
        let entities = self.entities.lock();
        let mut workspaces = Vec::new();
        for id in &state.workspace_ids {
            let entity = entities.get(id.as_str()).ok_or_else(|| {
                format!("workspace registry order references missing workspace '{id}'")
            })?;
            workspaces.push(Workspace::new(entity.clone()));
        }
        Ok(workspaces)
    }

    /// Delete one workspace registration while retaining its directory (TS
    /// `delete`).
    pub async fn delete(self: &Arc<Self>, id: &WorkspaceId) -> Result<bool, String> {
        let id = id.clone();
        let registry = Arc::clone(self);
        self.enqueue_operation(move || {
            let registry = registry.clone();
            box_future(async move { registry.delete_known(&id).await })
        })
        .await
    }

    /// Move one workspace within the durable display order (TS
    /// `insertBefore`).
    pub async fn insert_before(
        self: &Arc<Self>,
        id: &WorkspaceId,
        before_id: Option<&WorkspaceId>,
    ) -> Result<Vec<WorkspaceId>, String> {
        let id = id.clone();
        let before_id = before_id.cloned();
        let registry = Arc::clone(self);
        self.enqueue_operation(move || {
            let registry = registry.clone();
            box_future(async move {
                let state = registry.require_state();
                if !state.workspace_ids.contains(&id) {
                    return Err(WorkspaceOrderInvalidError { workspace_id: id }.to_string());
                }
                if let Some(before) = &before_id
                    && !state.workspace_ids.contains(before)
                {
                    return Err(WorkspaceOrderInvalidError {
                        workspace_id: before.clone(),
                    }
                    .to_string());
                }
                if before_id.as_ref() == Some(&id) {
                    return Ok(state.workspace_ids);
                }
                let without: Vec<WorkspaceId> = state
                    .workspace_ids
                    .iter()
                    .filter(|workspace_id| *workspace_id != &id)
                    .cloned()
                    .collect();
                let at = match &before_id {
                    None => without.len(),
                    Some(before) => without
                        .iter()
                        .position(|workspace_id| workspace_id == before)
                        .expect("anchor present"),
                };
                let mut workspace_ids = without;
                workspace_ids.insert(at, id);
                if same_ids(&workspace_ids, &state.workspace_ids) {
                    return Ok(state.workspace_ids);
                }
                let mut next = state;
                next.workspace_ids = workspace_ids.clone();
                registry.set_state(&next).await?;
                Ok(workspace_ids)
            })
        })
        .await
    }

    /// The registry-global archive set (TS `archivedSessionIds`).
    pub fn archived_session_ids(&self) -> Vec<SessionId> {
        self.require_state().archived_session_ids
    }

    /// Archive one session durably (TS `archiveSession`).
    pub async fn archive_session(self: &Arc<Self>, session_id: &SessionId) -> Result<(), String> {
        let session_id = session_id.clone();
        let registry = Arc::clone(self);
        self.enqueue_operation(move || {
            let registry = registry.clone();
            box_future(async move {
                if registry
                    .require_state()
                    .archived_session_ids
                    .contains(&session_id)
                {
                    return Ok(());
                }
                if !(registry.session_known(&session_id).await?) {
                    return Err(WorkspaceUnknownSessionError { session_id }.to_string());
                }
                let mut next = registry.require_state();
                next.archived_session_ids.push(session_id);
                registry.set_state(&next).await
            })
        })
        .await
    }

    /// Remove one session from the archive set (TS `unarchiveSession`).
    pub async fn unarchive_session(self: &Arc<Self>, session_id: &SessionId) -> Result<(), String> {
        let session_id = session_id.clone();
        let registry = Arc::clone(self);
        self.enqueue_operation(move || {
            let registry = registry.clone();
            box_future(async move {
                let state = registry.require_state();
                if !state.archived_session_ids.contains(&session_id) {
                    return Ok(());
                }
                let mut next = state;
                next.archived_session_ids.retain(|id| *id != session_id);
                registry.set_state(&next).await
            })
        })
        .await
    }

    /// Permanently delete one archived, cold session (TS
    /// `deleteArchivedSession`; the durable-log deletion goes through the
    /// caller-supplied closure until the persistence delete seam lands).
    pub async fn delete_archived_session(
        self: &Arc<Self>,
        session_id: &SessionId,
        release_live: Option<Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>>,
    ) -> Result<bool, String> {
        if let Some(live) = &self.host.live
            && live.get(session_id).is_some()
        {
            let Some(release) = &release_live else {
                return Err(WorkspaceSessionLiveError::new(session_id).to_string());
            };
            release().await;
            if live.get(session_id).is_some() {
                return Err(WorkspaceSessionLiveError::not_detached(session_id).to_string());
            }
        }
        let session_id = session_id.clone();
        let registry = Arc::clone(self);
        self.enqueue_operation(move || {
            let registry = registry.clone();
            box_future(async move {
                let state = registry.require_state();
                if !state.archived_session_ids.contains(&session_id) {
                    return Err(WorkspaceSessionNotArchivedError { session_id }.to_string());
                }
                let deleted = (registry.session_delete)(&session_id).await?;
                let entities = registry
                    .entities
                    .lock()
                    .values()
                    .cloned()
                    .collect::<Vec<_>>();
                for entity in entities {
                    let _ = entity.detach_session(&session_id).await;
                }
                registry.host.indexer.headers.lock().remove(&session_id);
                registry
                    .host
                    .indexer
                    .session_paths
                    .lock()
                    .remove(&session_id);
                registry
                    .host
                    .indexer
                    .invalid_session_paths
                    .lock()
                    .remove(&session_id);
                let mut next = registry.require_state();
                next.archived_session_ids.retain(|id| *id != session_id);
                registry.set_state(&next).await?;
                registry
                    .ctx
                    .emit("workspace/session-deleted", vec![arc(session_id)]);
                Ok(deleted)
            })
        })
        .await
    }

    /// Resolve by canonical directory path without creating or mutating (TS
    /// `resolveByPath`).
    pub async fn resolve_by_path(&self, path: &str) -> Result<Option<Workspace>, String> {
        let canonical = realpath_normalize(path)
            .await
            .map_err(|error| format!("cannot resolve workspace path '{path}': {error}"))?;
        for entity in self.entities.lock().values() {
            if entity.path() == canonical {
                return Ok(Some(Workspace::new(entity.clone())));
            }
        }
        Ok(None)
    }

    /// Whether a session is live, header-indexed, or present in a fresh
    /// persistence listing (TS `sessionKnown`).
    async fn session_known(&self, id: &SessionId) -> Result<bool, String> {
        if let Some(live) = &self.host.live
            && live.get(id).is_some()
        {
            return Ok(true);
        }
        if self.host.indexer.headers.lock().contains_key(id) {
            return Ok(true);
        }
        let headers = self.host.persistence.list().await?;
        self.host.indexer.index_headers(&headers).await;
        Ok(self.host.indexer.headers.lock().contains_key(id))
    }

    async fn create_canonical(
        &self,
        canonical: &str,
        title: Option<&str>,
    ) -> Result<Workspace, String> {
        for entity in self.entities.lock().values() {
            if entity.path() == canonical {
                return Ok(Workspace::new(entity.clone()));
            }
        }
        let workspace_name = title
            .map(|title| title.to_string())
            .unwrap_or_else(|| basename(canonical).to_string());
        let state = self.require_state();
        let id = workspace_id(uuid::Uuid::new_v4().to_string());
        let now = now_iso();
        let record = WorkspaceRecord {
            path: canonical.to_string(),
            title: workspace_name,
            session_ids: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        };
        let entity = Arc::new(WorkspaceEntity::new(
            self.host.clone(),
            id.clone(),
            record.clone(),
        ));
        self.entities
            .lock()
            .insert(id.as_str().to_string(), entity.clone());
        let pending_state = WorkspaceDomainState {
            pending_mutation: Some(WorkspacePendingMutation::Create {
                workspace_id: id.clone(),
            }),
            ..state.clone()
        };
        if let Err(error) = self.set_state(&pending_state).await {
            self.entities.lock().remove(id.as_str());
            return Err(error);
        }
        if let Err(error) = self
            .table
            .put(id.as_str(), serde_json::to_value(&record).expect("record"))
            .await
        {
            self.entities.lock().remove(id.as_str());
            if let Err(rollback_error) = self.set_state(&state).await {
                return Err(WorkspaceAggregateError::new(format!(
                    "workspace '{id}' record write and pending-marker rollback both failed: {error}; {rollback_error}"
                ))
                .to_string());
            }
            return Err(error);
        }
        let next_state = WorkspaceDomainState {
            initialized: true,
            workspace_ids: {
                let mut ids = vec![id.clone()];
                ids.extend(state.workspace_ids.iter().cloned());
                ids
            },
            archived_session_ids: state.archived_session_ids.clone(),
            pending_mutation: None,
        };
        if let Err(error) = self.set_state(&next_state).await {
            self.entities.lock().remove(id.as_str());
            if let Err(rollback_error) = self.table.delete(id.as_str()).await {
                return Err(WorkspaceAggregateError::new(format!(
                    "workspace '{id}' order write and record rollback both failed; the pending marker remains recoverable: {error}; {rollback_error}"
                ))
                .to_string());
            }
            if let Err(rollback_error) = self.set_state(&state).await {
                return Err(WorkspaceAggregateError::new(format!(
                    "workspace '{id}' order write and pending-marker rollback both failed: {error}; {rollback_error}"
                ))
                .to_string());
            }
            return Err(error);
        }
        Ok(Workspace::new(entity))
    }

    async fn delete_known(&self, id: &WorkspaceId) -> Result<bool, String> {
        let Some(entity) = self.entities.lock().get(id.as_str()).cloned() else {
            return Ok(false);
        };
        let state = self.require_state();
        let next_state = WorkspaceDomainState {
            initialized: true,
            workspace_ids: state
                .workspace_ids
                .iter()
                .filter(|workspace_id| *workspace_id != id)
                .cloned()
                .collect(),
            archived_session_ids: state.archived_session_ids.clone(),
            pending_mutation: None,
        };
        let pending_state = WorkspaceDomainState {
            pending_mutation: Some(WorkspacePendingMutation::Delete {
                workspace_id: id.clone(),
            }),
            ..next_state.clone()
        };
        self.set_state(&pending_state).await?;
        self.entities.lock().remove(id.as_str());
        if let Err(error) = self.table.delete(id.as_str()).await {
            self.entities
                .lock()
                .insert(id.as_str().to_string(), entity.clone());
            if let Err(rollback_error) = self.set_state(&state).await {
                self.entities.lock().remove(id.as_str());
                return Err(WorkspaceAggregateError::new(format!(
                    "workspace '{id}' record deletion and registry-order rollback both failed: {error}; {rollback_error}"
                ))
                .to_string());
            }
            return Err(error);
        }
        if let Err(error) = self.set_state(&next_state).await {
            self.ctx.named_logger(Some("workspace")).warn(vec![arc(format!(
                "workspace '{id}' was deleted but its pending marker could not be cleared: {error}"
            ))]);
        }
        Ok(true)
    }

    /// Complete the one mutation explicitly named by durable state (TS
    /// `recoverPendingMutation`).
    async fn recover_pending_mutation(&self) -> Result<(), String> {
        let state = self.require_state();
        let Some(pending) = state.pending_mutation.clone() else {
            return Ok(());
        };
        let pending_id = match &pending {
            WorkspacePendingMutation::Create { workspace_id }
            | WorkspacePendingMutation::Delete { workspace_id } => workspace_id.clone(),
        };
        let operation = match &pending {
            WorkspacePendingMutation::Create { .. } => "create",
            WorkspacePendingMutation::Delete { .. } => "delete",
        };
        if state.workspace_ids.contains(&pending_id) {
            return Err(format!(
                "workspace domain is inconsistent: pending {operation} workspace '{pending_id}' is still present in registry order"
            ));
        }
        self.table.delete(pending_id.as_str()).await?;
        self.set_state(&WorkspaceDomainState {
            pending_mutation: None,
            ..state
        })
        .await
    }

    async fn bootstrap(&self, headers: &[SessionHeader]) -> Result<(), String> {
        let state = self.require_state();
        let mut groups_by_path: HashMap<String, Vec<SessionHeader>> = HashMap::new();
        for header in headers {
            let Some(path) = self
                .host
                .indexer
                .session_paths
                .lock()
                .get(&header.id)
                .cloned()
            else {
                continue;
            };
            groups_by_path.entry(path).or_default().push(header.clone());
        }
        let mut groups: Vec<BootstrapGroup> = groups_by_path
            .into_iter()
            .map(|(path, mut group_headers)| {
                group_headers.sort_by(|left, right| {
                    right
                        .created_at
                        .cmp(&left.created_at)
                        .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
                });
                let newest_at = group_headers[0].created_at;
                BootstrapGroup {
                    path,
                    headers: group_headers,
                    newest_at,
                }
            })
            .collect();
        groups.sort_by(|left, right| {
            right
                .newest_at
                .cmp(&left.newest_at)
                .then_with(|| left.path.cmp(&right.path))
        });

        let mut by_path: HashMap<String, WorkspaceId> = HashMap::new();
        let mut accounted: HashMap<SessionId, WorkspaceId> = HashMap::new();
        for (id, record_value) in self.table.entries() {
            let record = record_from_value(&record_value)?;
            let workspace_id = workspace_id(id);
            by_path.insert(record.path.clone(), workspace_id.clone());
            for session_id in &record.session_ids {
                accounted.insert(session_id.clone(), workspace_id.clone());
            }
        }

        for group in &groups {
            let existing = by_path.get(&group.path).cloned();
            if existing.is_none() {
                let session_ids: Vec<SessionId> = group
                    .headers
                    .iter()
                    .map(|header| header.id.clone())
                    .filter(|session_id| !accounted.contains_key(session_id))
                    .collect();
                if session_ids.is_empty() {
                    continue;
                }
                let id = workspace_id(uuid::Uuid::new_v4().to_string());
                let created_at =
                    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(group.newest_at as i64)
                        .map(|instant| instant.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                        .unwrap_or_else(now_iso);
                let record = WorkspaceRecord {
                    path: group.path.clone(),
                    title: basename(&group.path).to_string(),
                    session_ids: session_ids.clone(),
                    created_at: created_at.clone(),
                    updated_at: created_at,
                };
                self.table
                    .put(id.as_str(), serde_json::to_value(&record).expect("record"))
                    .await?;
                by_path.insert(group.path.clone(), id.clone());
                for session_id in session_ids {
                    accounted.insert(session_id, id.clone());
                }
                continue;
            }
            let id = existing.expect("checked above");
            let current_value = self.table.get(id.as_str()).ok_or_else(|| {
                format!("workspace domain is inconsistent: table record '{id}' vanished")
            })?;
            let current = record_from_value(&current_value)?;
            let historical: Vec<SessionId> = group
                .headers
                .iter()
                .map(|header| header.id.clone())
                .filter(|session_id| accounted.get(session_id).is_none_or(|holder| *holder == id))
                .collect();
            let historical_set: HashSet<SessionId> = historical.iter().cloned().collect();
            let mut session_ids = historical.clone();
            session_ids.extend(
                current
                    .session_ids
                    .iter()
                    .filter(|session_id| !historical_set.contains(session_id))
                    .cloned(),
            );
            if same_session_ids(&current.session_ids, &session_ids) {
                continue;
            }
            let mut next = current.clone();
            next.session_ids = session_ids;
            next.updated_at = now_iso();
            self.table
                .put(id.as_str(), serde_json::to_value(&next).expect("record"))
                .await?;
            for session_id in historical {
                accounted.insert(session_id, id.clone());
            }
        }

        let group_rank: HashMap<String, u64> = groups
            .iter()
            .map(|group| (group.path.clone(), group.newest_at))
            .collect();
        let prior_rank: HashMap<WorkspaceId, usize> = state
            .workspace_ids
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), index))
            .collect();
        let mut entries: Vec<(WorkspaceId, WorkspaceRecord)> = Vec::new();
        for (id, value) in self.table.entries() {
            entries.push((workspace_id(id), record_from_value(&value)?));
        }
        entries.sort_by(|(left_id, left), (right_id, right)| {
            let left_time = group_rank
                .get(&left.path)
                .copied()
                .unwrap_or_else(|| parse_iso(&left.created_at));
            let right_time = group_rank
                .get(&right.path)
                .copied()
                .unwrap_or_else(|| parse_iso(&right.created_at));
            right_time
                .cmp(&left_time)
                .then_with(|| {
                    prior_rank
                        .get(left_id)
                        .copied()
                        .unwrap_or(usize::MAX)
                        .cmp(&prior_rank.get(right_id).copied().unwrap_or(usize::MAX))
                })
                .then_with(|| left_id.to_string().cmp(&right_id.to_string()))
        });
        let workspace_ids: Vec<WorkspaceId> = entries.into_iter().map(|(id, _)| id).collect();
        if !same_ids(&state.workspace_ids, &workspace_ids) {
            self.set_state(&WorkspaceDomainState {
                initialized: false,
                workspace_ids: workspace_ids.clone(),
                archived_session_ids: state.archived_session_ids.clone(),
                pending_mutation: None,
            })
            .await?;
        }
        self.set_state(&WorkspaceDomainState {
            initialized: true,
            workspace_ids,
            archived_session_ids: state.archived_session_ids.clone(),
            pending_mutation: None,
        })
        .await
    }

    fn validate_stored_state(&self, state: &WorkspaceDomainState) -> Result<(), String> {
        let mut order = HashSet::new();
        for id in &state.workspace_ids {
            if !order.insert(id.clone()) {
                return Err(format!(
                    "workspace domain is inconsistent: registry order repeats workspace '{id}'"
                ));
            }
            if self.table.get(id.as_str()).is_none() {
                return Err(format!(
                    "workspace domain is inconsistent: registry order references missing workspace '{id}'"
                ));
            }
        }
        if state.initialized && order.len() != self.table.len() {
            let orphan = self
                .table
                .keys()
                .into_iter()
                .find(|id| !order.contains(&workspace_id(id.clone())))
                .map(workspace_id)
                .unwrap_or_else(|| workspace_id("<unknown>"));
            return Err(format!(
                "workspace domain is inconsistent: workspace '{orphan}' is absent from registry order"
            ));
        }
        let mut paths: HashMap<String, WorkspaceId> = HashMap::new();
        let mut accounted: HashMap<SessionId, WorkspaceId> = HashMap::new();
        for (id, record_value) in self.table.entries() {
            let record = record_from_value(&record_value)?;
            let id = workspace_id(id);
            if let Some(holder) = paths.get(&record.path) {
                return Err(format!(
                    "workspace domain is inconsistent: path '{}' is claimed by both workspace '{holder}' and workspace '{id}'",
                    record.path
                ));
            }
            paths.insert(record.path.clone(), id.clone());
            for session_id in &record.session_ids {
                if let Some(holder) = accounted.get(session_id) {
                    return Err(format!(
                        "workspace domain is inconsistent: session '{session_id}' is accounted by both workspace '{holder}' and workspace '{id}'"
                    ));
                }
                accounted.insert(session_id.clone(), id.clone());
            }
        }
        Ok(())
    }

    fn seed_paths_from_durable_records(&self) {
        for entity in self.entities.lock().values() {
            let record = entity.record();
            for session_id in &record.session_ids {
                self.host
                    .indexer
                    .session_paths
                    .lock()
                    .insert(session_id.clone(), record.path.clone());
            }
        }
    }

    fn rebuild_entities(&self) -> Result<(), String> {
        let mut entities = self.entities.lock();
        entities.clear();
        for id in &self.require_state().workspace_ids {
            let value = self.table.get(id.as_str()).ok_or_else(|| {
                format!("workspace registry order references missing workspace '{id}'")
            })?;
            let record = record_from_value(&value)?;
            entities.insert(
                id.as_str().to_string(),
                Arc::new(WorkspaceEntity::new(self.host.clone(), id.clone(), record)),
            );
        }
        Ok(())
    }

    fn report_filtered_candidates(&self) {
        for entity in self.entities.lock().values() {
            let record = entity.record();
            for session_id in &record.session_ids {
                let path = self
                    .host
                    .indexer
                    .session_paths
                    .lock()
                    .get(session_id)
                    .cloned();
                if path.as_deref() == Some(record.path.as_str()) {
                    continue;
                }
                let reason = self
                    .host
                    .indexer
                    .invalid_session_paths
                    .lock()
                    .get(session_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        if self.host.indexer.headers.lock().contains_key(session_id) {
                            format!(
                                "canonical cwd '{}' differs from workspace path '{}'",
                                path.unwrap_or_default(),
                                record.path
                            )
                        } else {
                            "session header is missing".to_string()
                        }
                    });
                self.ctx
                    .named_logger(Some("workspace"))
                    .warn(vec![arc(format!(
                        "workspace '{}' filtered session '{session_id}' from membership: {reason}",
                        entity.id
                    ))]);
            }
        }
    }

    fn require_state(&self) -> WorkspaceDomainState {
        self.state.lock().clone()
    }

    async fn set_state(&self, state: &WorkspaceDomainState) -> Result<(), String> {
        self.domain
            .global()
            .set(serde_json::to_value(state).expect("state serializes"))
            .await?;
        *self.state.lock() = state.clone();
        Ok(())
    }

    async fn enqueue_operation<T, F>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce() -> BoxFuture<'static, Result<T, String>>,
        T: Send + 'static,
    {
        let _guard = self.operation_tail.lock().await;
        // A committed delete may leave only its marker cleanup pending.
        self.recover_pending_mutation().await?;
        f().await
    }

    /// The domain handle (diagnostic surface).
    pub fn domain(&self) -> &Arc<Domain> {
        &self.domain
    }

    /// The shared table handle (diagnostic surface).
    pub fn table(&self) -> &Arc<dyn KvTable> {
        &self.table
    }

    /// Diagnostic: drop one cached entity without touching the durable
    /// medium (the TS spec's direct `entities.delete` divergence injection).
    #[doc(hidden)]
    pub fn uncache(&self, id: &WorkspaceId) {
        self.entities.lock().remove(id.as_str());
    }
}

fn parse_iso(timestamp: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|instant| instant.timestamp_millis().max(0) as u64)
        .unwrap_or(0)
}

fn basename(path: &str) -> &str {
    let display = path.strip_prefix(r"\\?\").unwrap_or(path);
    let trimmed = display.trim_end_matches(['/', '\\']);
    if trimmed.is_empty()
        || (trimmed.len() == 2
            && trimmed.as_bytes()[0].is_ascii_alphabetic()
            && trimmed.ends_with(':'))
    {
        return display;
    }
    trimmed.rsplit(['/', '\\']).next().unwrap_or(display)
}

#[cfg(test)]
mod root_title_tests {
    #[test]
    fn roots_keep_a_nonempty_title_and_children_use_the_last_component() {
        for (path, expected) in [
            ("/", "/"),
            ("C:\\", "C:\\"),
            (r"\\?\C:\", "C:\\"),
            ("C:/", "C:/"),
            ("C:\\work\\", "work"),
            ("/work/project/", "project"),
        ] {
            assert_eq!(super::basename(path), expected);
        }
    }
}
