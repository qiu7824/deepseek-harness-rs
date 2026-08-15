//! Shared test harness for the workspace spec port. Rust port of the TS
//! `workspace.spec.ts` harness: the real storage/domain/registry composition
//! over controllable header-only peers.
//!
//! Deviations from the TS harness:
//!
//! - `ctx.plugin(WorkspaceRegistry)` collapses into
//!   [`WorkspaceRegistry::install`]; "fiber dispose" collapses into
//!   `registry.domain().close()` followed by a re-install.
//! - The `selectiveFailureBackend` wrapper injects failures by counting
//!   durable primitive calls exactly like the TS one.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cordis::Context;
use dsh_session::{SessionEvent, SessionHeader, SessionId, session_id};
use dsh_session_persistence::{
    SessionInspection, SessionLocation, SessionPersistenceApi, SessionPersistenceSnapshot,
    SessionReadFromResult,
};
use dsh_storage::{
    KvFacet, KvUnit, KvUnitDescriptor, KvUnitSnapshot, Storage, StorageBackend, StorageError,
    StorageErrorCode,
};
use dsh_storage_domain::{DomainChanged, DomainFacility, DomainFacilityConfig};
pub use dsh_storage_test_support::MemoryMediaPool;
use dsh_storage_test_support::{MemoryMedium, MemoryStorageBackend};
use dsh_workspace::{LiveSessionStore, SessionDeleteFn, WorkspaceDomainState, WorkspaceId, WorkspaceRecord, WorkspaceRegistry, record_from_value, state_from_value, workspace_id};
use parking_lot::Mutex;
use serde_json::json;

pub const DOMAIN_VERSION: u64 = 2;

// ---------------------------------------------------------------------------
// value helpers

pub fn sid(id: &str) -> SessionId {
    session_id(id.to_string())
}

pub fn wid(id: &str) -> WorkspaceId {
    workspace_id(id.to_string())
}

pub fn header(id: &str, cwd: Option<&str>, created_at: u64) -> SessionHeader {
    SessionHeader {
        version: 0,
        id: sid(id),
        created_at,
        cwd: cwd.map(|cwd| cwd.to_string()),
        parent_session: None,
        seed_length: None,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    }
}

fn basename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

pub fn record(path: &str, session_ids: &[&str], created_at: &str) -> WorkspaceRecord {
    WorkspaceRecord {
        path: path.to_string(),
        title: basename(path),
        session_ids: session_ids.iter().map(|id| sid(id)).collect(),
        created_at: created_at.to_string(),
        updated_at: created_at.to_string(),
    }
}

// ---------------------------------------------------------------------------
// temp directories

/// A per-test temp root that cleans itself up (the TS `makeDir` + `afterEach`).
pub struct TempRoot {
    base: std::path::PathBuf,
}

impl TempRoot {
    pub fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "dsh-workspace-rs-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("create temp root");
        Self { base }
    }

    /// Create a child directory and return its raw path.
    pub fn dir(&self, name: &str) -> String {
        let dir = self.base.join(name);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.to_string_lossy().into_owned()
    }

    /// A child path that must NOT exist yet.
    pub fn path(&self, name: &str) -> String {
        self.base.join(name).to_string_lossy().into_owned()
    }

    pub fn base(&self) -> String {
        self.base.to_string_lossy().into_owned()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

// ---------------------------------------------------------------------------
// stored media

/// Pre-materialize one stored workspace medium (the TS `storedPool`).
/// `omit_archived` removes the `archivedSessionIds` field to exercise the
/// schema default upgrade path.
pub fn stored_pool(
    entries: &[(&str, WorkspaceRecord)],
    state: WorkspaceDomainState,
    omit_archived: bool,
) -> Arc<MemoryMediaPool> {
    let pool = Arc::new(MemoryMediaPool::new());
    pool.versions.lock().insert("workspace".to_string(), DOMAIN_VERSION);
    let mut workspaces = HashMap::new();
    for (id, record) in entries {
        workspaces.insert(id.to_string(), serde_json::to_value(record).expect("record"));
    }
    let mut tables = HashMap::new();
    tables.insert("workspaces".to_string(), workspaces);
    let mut global = serde_json::to_value(&state).expect("state");
    if omit_archived {
        global
            .as_object_mut()
            .expect("state object")
            .remove("archivedSessionIds");
    }
    pool.media.lock().insert(
        "workspace".to_string(),
        MemoryMedium { tables, global },
    );
    pool
}

/// Read one stored workspace record from the medium (the TS `storedRecord`).
pub fn stored_record(pool: &Arc<MemoryMediaPool>, id: &str) -> WorkspaceRecord {
    let media = pool.media.lock();
    let medium = media.get("workspace").expect("workspace medium");
    let table = medium.tables.get("workspaces").expect("workspaces table");
    record_from_value(table.get(id).expect("stored record")).expect("record parses")
}

/// Read the stored workspace domain state (the TS `storedState`).
pub fn stored_state(pool: &Arc<MemoryMediaPool>) -> WorkspaceDomainState {
    let media = pool.media.lock();
    let medium = media.get("workspace").expect("workspace medium");
    state_from_value(&medium.global).expect("state parses")
}

// ---------------------------------------------------------------------------
// peer fakes

/// Header-only session persistence fake (the TS `sessionPersistence` stub).
pub struct FakePersistence {
    ctx: Context,
    listed: Arc<Mutex<Vec<SessionHeader>>>,
    pub list_calls: Arc<AtomicUsize>,
    pub load_calls: Arc<AtomicUsize>,
    pub inspect_calls: Arc<AtomicUsize>,
    pub delete_calls: Arc<Mutex<Vec<SessionId>>>,
    list_error: Arc<Mutex<Option<String>>>,
}

impl FakePersistence {
    pub fn new(ctx: &Context, sessions: &[SessionHeader]) -> Arc<Self> {
        Arc::new(Self {
            ctx: ctx.clone(),
            listed: Arc::new(Mutex::new(sessions.to_vec())),
            list_calls: Arc::new(AtomicUsize::new(0)),
            load_calls: Arc::new(AtomicUsize::new(0)),
            inspect_calls: Arc::new(AtomicUsize::new(0)),
            delete_calls: Arc::new(Mutex::new(Vec::new())),
            list_error: Arc::new(Mutex::new(None)),
        })
    }

    pub fn set_sessions(&self, headers: &[SessionHeader]) {
        *self.listed.lock() = headers.to_vec();
    }

    pub fn set_list_error(&self, error: impl Into<String>) {
        *self.list_error.lock() = Some(error.into());
    }
}

#[async_trait::async_trait]
impl SessionPersistenceApi for FakePersistence {
    fn locate(&self, _meta: &SessionHeader) -> Option<SessionLocation> {
        None
    }

    fn supports_raw_artifacts(&self) -> bool {
        false
    }

    async fn create(&self, _meta: SessionHeader) -> Result<(), String> {
        Ok(())
    }

    async fn append(&self, _id: &SessionId, _events: &[SessionEvent]) -> Result<(), String> {
        Ok(())
    }

    async fn load(&self, _id: &SessionId) -> Result<SessionInspection, String> {
        self.load_calls.fetch_add(1, Ordering::SeqCst);
        Err("event bodies must not be loaded".to_string())
    }

    async fn inspect(&self, _id: &SessionId) -> Result<SessionInspection, String> {
        self.inspect_calls.fetch_add(1, Ordering::SeqCst);
        Err("event bodies must not be inspected".to_string())
    }

    async fn read_from(
        &self,
        _id: &SessionId,
        _from_seq: u64,
    ) -> Result<SessionReadFromResult, String> {
        Err("event bodies must not be read".to_string())
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.list_error.lock().take() {
            return Err(error);
        }
        Ok(self.listed.lock().clone())
    }

    async fn list_snapshots(&self) -> Result<Vec<SessionPersistenceSnapshot>, String> {
        Ok(Vec::new())
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }
}

/// Mutable header-only live-session fake (the TS `ctx.sessions` stub).
pub struct FakeLiveSessions {
    headers: Mutex<HashMap<SessionId, SessionHeader>>,
}

impl FakeLiveSessions {
    pub fn new(headers: &[SessionHeader]) -> Arc<Self> {
        let map = headers.iter().map(|h| (h.id.clone(), h.clone())).collect();
        Arc::new(Self { headers: Mutex::new(map) })
    }

    pub fn remove(&self, id: &SessionId) {
        self.headers.lock().remove(id);
    }
}

impl LiveSessionStore for FakeLiveSessions {
    fn get(&self, id: &SessionId) -> Option<SessionHeader> {
        self.headers.lock().get(id).cloned()
    }

    fn list(&self) -> Vec<SessionHeader> {
        self.headers.lock().values().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// selective-failure backend (the TS `selectiveFailureBackend`)

#[derive(Debug, Clone, Default)]
pub struct FailureSpec {
    /// Fail the Nth `put_record` call (1-based).
    pub put_at: Option<usize>,
    /// Fail the Nth `delete_record` call (1-based).
    pub delete_at: Option<usize>,
    /// Fail the Nth `set_global` call (1-based).
    pub global_at: Option<usize>,
    /// Additional `set_global` failures (the TS array form).
    pub extra_global_at: HashSet<usize>,
}

impl FailureSpec {
    pub fn global(mut self, at: usize) -> Self {
        self.global_at = Some(at);
        self
    }

    pub fn also_global(mut self, at: usize) -> Self {
        self.extra_global_at.insert(at);
        self
    }
}

pub fn selective_failure_backend(
    pool: Arc<MemoryMediaPool>,
    spec: FailureSpec,
) -> Arc<dyn StorageBackend> {
    Arc::new(SelectiveBackend { inner: MemoryStorageBackend::with_shared_pool(pool), spec })
}

struct SelectiveBackend {
    inner: Arc<MemoryStorageBackend>,
    spec: FailureSpec,
}

#[async_trait::async_trait]
impl StorageBackend for SelectiveBackend {
    fn kv(&self) -> Option<Arc<dyn KvFacet>> {
        self.inner.kv().map(|inner| {
            Arc::new(SelectiveFacet { inner, spec: self.spec.clone() }) as Arc<dyn KvFacet>
        })
    }

    async fn close(&self) -> Result<(), StorageError> {
        self.inner.close().await
    }
}

#[derive(Clone)]
struct Counts {
    puts: usize,
    deletes: usize,
    globals: usize,
}

struct SelectiveFacet {
    inner: Arc<dyn KvFacet>,
    spec: FailureSpec,
}

#[async_trait::async_trait]
impl KvFacet for SelectiveFacet {
    async fn open(&self, descriptor: &KvUnitDescriptor) -> Result<Arc<dyn KvUnit>, StorageError> {
        let inner = self.inner.open(descriptor).await?;
        Ok(Arc::new(SelectiveUnit {
            inner,
            counts: Arc::new(Mutex::new(Counts { puts: 0, deletes: 0, globals: 0 })),
            spec: self.spec.clone(),
        }))
    }
}

struct SelectiveUnit {
    inner: Arc<dyn KvUnit>,
    counts: Arc<Mutex<Counts>>,
    spec: FailureSpec,
}

impl SelectiveUnit {
    fn fail(message: &str) -> StorageError {
        StorageError::new(StorageErrorCode::Closed, message.to_string())
    }
}

#[async_trait::async_trait]
impl KvUnit for SelectiveUnit {
    async fn load_all(&self) -> Result<KvUnitSnapshot, StorageError> {
        self.inner.load_all().await
    }

    async fn put_record(
        &self,
        table: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), StorageError> {
        let n = {
            let mut counts = self.counts.lock();
            counts.puts += 1;
            counts.puts
        };
        if self.spec.put_at == Some(n) {
            return Err(Self::fail("selected bootstrap put failure"));
        }
        self.inner.put_record(table, key, value).await
    }

    async fn delete_record(&self, table: &str, key: &str) -> Result<(), StorageError> {
        let n = {
            let mut counts = self.counts.lock();
            counts.deletes += 1;
            counts.deletes
        };
        if self.spec.delete_at == Some(n) {
            return Err(Self::fail("selected rollback delete failure"));
        }
        self.inner.delete_record(table, key).await
    }

    async fn set_global(&self, value: serde_json::Value) -> Result<(), StorageError> {
        let n = {
            let mut counts = self.counts.lock();
            counts.globals += 1;
            counts.globals
        };
        if self.spec.global_at == Some(n) || self.spec.extra_global_at.contains(&n) {
            return Err(Self::fail("selected bootstrap marker failure"));
        }
        self.inner.set_global(value).await
    }

    async fn close(&self) -> Result<(), StorageError> {
        self.inner.close().await
    }
}

// ---------------------------------------------------------------------------
// the harness

pub struct Harness {
    pub ctx: Context,
    pub registry: Arc<WorkspaceRegistry>,
    pub pool: Arc<MemoryMediaPool>,
    pub changes: Arc<Mutex<Vec<DomainChanged>>>,
    pub persistence: Arc<FakePersistence>,
    pub deleted_sessions: Arc<Mutex<Vec<SessionId>>>,
    pub hub: Arc<Storage>,
    pub facility: Arc<DomainFacility>,
}

/// Boot the real storage/domain/registry composition (the TS `harness`).
/// Panics when the install boundary rejects (the success-expected form).
pub async fn harness(
    pool: Arc<MemoryMediaPool>,
    sessions: &[SessionHeader],
    live: Option<Arc<dyn LiveSessionStore>>,
) -> Harness {
    harness_with_backend(pool, sessions, live, None)
        .await
        .expect("workspace registry install")
}

/// Boot the composition, surfacing an install-boundary rejection (the TS
/// `expect(...).rejects` form over startup failure cases).
pub async fn harness_with_backend(
    pool: Arc<MemoryMediaPool>,
    sessions: &[SessionHeader],
    live: Option<Arc<dyn LiveSessionStore>>,
    backend: Option<Arc<dyn StorageBackend>>,
) -> Result<Harness, String> {
    let ctx = Context::root();
    let hub = Storage::install(&ctx);
    let backend = backend.unwrap_or_else(|| MemoryStorageBackend::with_shared_pool(pool.clone()));
    hub.backend.register("memory", backend).expect("register backend");
    let facility = DomainFacility::install(
        &ctx,
        DomainFacilityConfig { backend: "memory".to_string(), routes: Default::default() },
    )
    .expect("domain facility");

    let persistence = FakePersistence::new(&ctx, sessions);

    let changes: Arc<Mutex<Vec<DomainChanged>>> = Arc::new(Mutex::new(Vec::new()));
    let changes_for_listener = changes.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let change = cordis::downcast::<DomainChanged>(&args[0]).cloned();
        let changes = changes_for_listener.clone();
        Box::pin(async move {
            if let Some(change) = change {
                changes.lock().push(change);
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "domain/changed",
        listener,
        cordis::EventOptions::default(),
    ));

    let deleted_sessions: Arc<Mutex<Vec<SessionId>>> = Arc::new(Mutex::new(Vec::new()));
    let deleted_for_listener = deleted_sessions.clone();
    let deleted_listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let id = cordis::downcast::<SessionId>(&args[0]).cloned();
        let deleted = deleted_for_listener.clone();
        Box::pin(async move {
            if let Some(id) = id {
                deleted.lock().push(id);
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "workspace/session-deleted",
        deleted_listener,
        cordis::EventOptions::default(),
    ));

    let delete_calls = persistence.delete_calls.clone();
    let session_delete: SessionDeleteFn = Arc::new(move |id| {
        let delete_calls = delete_calls.clone();
        let id = id.clone();
        Box::pin(async move {
            delete_calls.lock().push(id.clone());
            Ok(true)
        })
    });

    let registry = WorkspaceRegistry::install(&ctx, &facility, persistence.clone(), live, session_delete)?;

    // The install boundary emitted its own changes; tests count from a
    // settled post-install snapshot (the TS harness splits initChanges).
    settle().await;
    changes.lock().clear();

    Ok(Harness { ctx, registry, pool, changes, persistence, deleted_sessions, hub, facility })
}

/// Yield until every fire-and-forget event task has run.
pub async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

/// The default no-op durable session deletion seam.
pub fn noop_delete() -> SessionDeleteFn {
    Arc::new(|_id| Box::pin(async move { Ok(true) }))
}

/// JSON value for a `domain/changed` put of one workspace record key.
pub fn put_change(key: &str) -> DomainChanged {
    DomainChanged::Put {
        domain: "workspace".to_string(),
        table: "workspaces".to_string(),
        key: key.to_string(),
        value: json!({}),
    }
}

pub fn deleted_change(key: &str) -> DomainChanged {
    DomainChanged::Deleted {
        domain: "workspace".to_string(),
        table: "workspaces".to_string(),
        key: key.to_string(),
    }
}
