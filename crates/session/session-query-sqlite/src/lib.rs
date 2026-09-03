//! Concrete session-query service with SQLite FTS5 over the live-preferred
//! corpus. Rust port of `packages/session-query/session-query-sqlite/src/index.ts`.
//!
//! # Deviations
//!
//! - The optional `sessionPersistence` binding is refreshed by polling the
//!   service registry at each observation checkpoint instead of an optional
//!   child fiber; identity changes follow the same observable rules.
//! - The Rust persistence API carries `String` failures, so persisted-source
//!   failures always surface as `SESSION_QUERY_PERSISTENCE_FAILED` with the
//!   wrapped detail (no typed pass-through).
//! - Read-path SQLite failures (outside reconciliation) surface as
//!   `SESSION_QUERY_INDEX_FAILED`.

pub mod invariant;
pub mod query;
pub mod schema;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cordis::{ArcValue, Context, Disposer, InjectSpec, Plugin, PluginError};
use dsh_session::{
    Session, SessionEvent, SessionHeader, SessionId, SessionStore, StreamingSurfaceFold, session_id,
};
use dsh_session_persistence::{SessionPersistenceApi, SessionPersistenceRevision};
use dsh_session_query::filters::surface_from_str;
use dsh_session_query::{
    SessionEventSearchHit, SessionEventSearchPage, SessionEventSearchRequest, SessionEventSurface,
    SessionQueryEngine, SessionQueryError, SessionQueryErrorCode, SessionQuerySearch,
    SessionRecord, SessionSearchCursor, SessionSearchExecContext, SessionSearchHit,
    SessionSearchPage, SessionSearchRequest, assert_session_headers_compatible,
    build_session_event_search_documents,
};
use rusqlite::{Connection, OptionalExtension};
use sha2::Digest;

use crate::query::{
    Binding, CursorPayload, FTS_HIGHLIGHT_END, FTS_HIGHLIGHT_START, NormalizedEventRequest,
    NormalizedSessionRequest, RequestFingerprint, build_event_where, build_session_where,
    decode_cursor, encode_cursor, make_snippet, normalize_event_request, normalize_session_request,
    quote_fts_data, request_fingerprint, sanitize_fts_text,
};
use crate::schema::open_search_database;

pub use crate::schema::{SESSION_QUERY_SQLITE_APPLICATION_ID, SESSION_QUERY_SQLITE_SCHEMA_VERSION};

/// Boot-context slot for a launcher-owned absolute path to this process's
/// derived query index.
pub const SESSION_QUERY_SQLITE_PATH_KEY: &str = "launcherSessionQueryPath";

/// Default result page size.
pub const SESSION_QUERY_SQLITE_DEFAULT_LIMIT: u64 = 20;
/// Maximum accepted result page size.
pub const SESSION_QUERY_SQLITE_MAX_LIMIT: u64 = 100;
/// Default maximum snippet length in Unicode code points.
pub const SESSION_QUERY_SQLITE_SNIPPET_CHARS: usize = 240;

// One transient source change gets a retry; repeated churn fails rather than
// monopolizing the queue.
const STABLE_OBSERVATION_ATTEMPTS: usize = 2;

/// SQLite module/handle opening phase; `never` disables full-text search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAt {
    Startup,
    FirstSearch,
    Never,
}

/// Supported SQLite journal modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalMode {
    Wal,
    Delete,
    Truncate,
    Persist,
}

/// Combined session-query configuration backed by SQLite full-text search.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Dedicated derived-index path; `:memory:` is supported for ephemeral
    /// indexes.
    pub path: String,
    /// Opening phase; defaults to `startup`.
    pub open_at: Option<OpenAt>,
    /// SQLite journal mode; defaults to `wal`.
    pub journal_mode: Option<JournalMode>,
    /// Page size when a request omits `limit`; defaults to 20.
    pub default_limit: Option<u64>,
    /// Largest accepted page size; defaults to 100.
    pub max_limit: Option<u64>,
    /// Maximum snippet length in Unicode code points; defaults to 240.
    pub snippet_chars: Option<usize>,
    /// Maximum concurrent persisted-log inspections in one inherited batch
    /// read; defaults to 4.
    pub read_window_max: Option<u64>,
    /// Maximum concurrent persisted-log inspections in one inherited batch
    /// read; defaults to 4.
    pub persisted_inspect_concurrency: Option<usize>,
}

/// Validated and defaulted backend configuration.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub path: String,
    pub open_at: OpenAt,
    pub journal_mode: JournalMode,
    pub default_limit: u64,
    pub max_limit: u64,
    pub snippet_chars: usize,
    pub read_window_max: u64,
    pub persisted_inspect_concurrency: usize,
}

/// Validate and default one caller configuration.
pub fn resolve_config(config: &Config) -> Result<ResolvedConfig, SessionQueryError> {
    let resolved = ResolvedConfig {
        path: config.path.clone(),
        open_at: config.open_at.unwrap_or(OpenAt::Startup),
        journal_mode: config.journal_mode.unwrap_or(JournalMode::Wal),
        default_limit: config
            .default_limit
            .unwrap_or(SESSION_QUERY_SQLITE_DEFAULT_LIMIT),
        max_limit: config.max_limit.unwrap_or(SESSION_QUERY_SQLITE_MAX_LIMIT),
        snippet_chars: config
            .snippet_chars
            .unwrap_or(SESSION_QUERY_SQLITE_SNIPPET_CHARS),
        read_window_max: config
            .read_window_max
            .unwrap_or(dsh_session_query::SESSION_QUERY_READ_WINDOW_MAX),
        persisted_inspect_concurrency: config
            .persisted_inspect_concurrency
            .unwrap_or(dsh_session_query::SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY),
    };
    if resolved.path.trim().is_empty() {
        return Err(invalid_config("path must not be blank"));
    }
    assert_page_limit("defaultLimit", resolved.default_limit)?;
    assert_page_limit("maxLimit", resolved.max_limit)?;
    assert_positive_integer("snippetChars", resolved.snippet_chars)?;
    assert_positive_integer(
        "persistedInspectConcurrency",
        resolved.persisted_inspect_concurrency,
    )?;
    if resolved.default_limit > resolved.max_limit {
        return Err(invalid_config(
            "defaultLimit must be less than or equal to maxLimit",
        ));
    }
    Ok(resolved)
}

fn assert_positive_integer(name: &str, value: usize) -> Result<(), SessionQueryError> {
    if value < 1 {
        return Err(invalid_config(&format!(
            "{name} must be a positive integer"
        )));
    }
    Ok(())
}

fn assert_page_limit(name: &str, value: u64) -> Result<(), SessionQueryError> {
    if value < 1 || value > crate::query::SQLITE_MAX_PAGE_LIMIT {
        return Err(invalid_config(&format!(
            "{name} must be an integer between 1 and {}",
            crate::query::SQLITE_MAX_PAGE_LIMIT
        )));
    }
    Ok(())
}

fn invalid_config(detail: &str) -> SessionQueryError {
    SessionQueryError::new(
        SessionQueryErrorCode::SessionQueryInvalidConfig,
        format!("session-search SQLite config: {detail}"),
    )
}

fn index_closed() -> SessionQueryError {
    SessionQueryError::new(
        SessionQueryErrorCode::SessionQueryIndexFailed,
        "session-search SQLite index is closed",
    )
}

fn index_open_failed(detail: &str) -> SessionQueryError {
    SessionQueryError::new(
        SessionQueryErrorCode::SessionQueryIndexFailed,
        format!("session-search SQLite index failed to open: {detail}"),
    )
}

fn abort_error() -> SessionQueryError {
    SessionQueryError::new(
        SessionQueryErrorCode::SessionQueryAborted,
        "session-search aborted",
    )
}

fn aborted(signal: Option<&dsh_session_query::corpus::SessionQueryAbort>) -> bool {
    signal.is_some_and(|signal| signal())
}

fn assert_not_aborted(
    signal: Option<&dsh_session_query::corpus::SessionQueryAbort>,
) -> Result<(), SessionQueryError> {
    if aborted(signal) {
        return Err(abort_error());
    }
    Ok(())
}

/// The optional persistence binding (TS `PersistenceBinding`).
#[derive(Clone)]
pub(crate) struct PersistenceBinding {
    identity: u64,
    service: Option<Arc<dyn SessionPersistenceApi>>,
}

struct ObservedSession {
    header: SessionHeader,
    documents: Vec<dsh_session_query::SessionEventSearchDocument>,
    fingerprint: String,
}

struct ObservedPersistedSession {
    header: SessionHeader,
    revision: SessionPersistenceRevision,
}

struct Observation {
    persistence_binding: PersistenceBinding,
    persisted: HashMap<String, ObservedPersistedSession>,
    live: HashMap<String, ObservedSession>,
}

struct IndexedPersistedRow {
    id: String,
    revision: String,
    _generation: u64,
}

struct IndexedLiveRow {
    id: String,
    fingerprint: String,
    persisted: i64,
    _generation: u64,
}

struct SessionHeaderRow {
    session_id: String,
    version: i64,
    created_at: i64,
    cwd: Option<String>,
    parent_session: Option<String>,
    seed_length: Option<i64>,
    delegation_depth: Option<i64>,
    agent_preset: Option<String>,
}

struct SearchRow {
    session_id: String,
    version: i64,
    created_at: i64,
    cwd: Option<String>,
    parent_session: Option<String>,
    seed_length: Option<i64>,
    delegation_depth: Option<i64>,
    agent_preset: Option<String>,
    live: i64,
    persisted: i64,
    seq: i64,
    type_: String,
    time: i64,
    surface: String,
    marked_text: String,
    match_count: i64,
    document_length: i64,
}

impl Clone for SearchRow {
    fn clone(&self) -> Self {
        Self {
            session_id: self.session_id.clone(),
            version: self.version,
            created_at: self.created_at,
            cwd: self.cwd.clone(),
            parent_session: self.parent_session.clone(),
            seed_length: self.seed_length,
            delegation_depth: self.delegation_depth,
            agent_preset: self.agent_preset.clone(),
            live: self.live,
            persisted: self.persisted,
            seq: self.seq,
            type_: self.type_.clone(),
            time: self.time,
            surface: self.surface.clone(),
            marked_text: self.marked_text.clone(),
            match_count: self.match_count,
            document_length: self.document_length,
        }
    }
}

impl SearchRow {
    fn header_row(&self) -> SessionHeaderRow {
        SessionHeaderRow {
            session_id: self.session_id.clone(),
            version: self.version,
            created_at: self.created_at,
            cwd: self.cwd.clone(),
            parent_session: self.parent_session.clone(),
            seed_length: self.seed_length,
            delegation_depth: self.delegation_depth,
            agent_preset: self.agent_preset.clone(),
        }
    }
}

/// Mutable generation bookkeeping (TS instance fields).
#[derive(Default)]
struct GenState {
    global_generation: u64,
    local_generation: u64,
    persistence_epoch: u64,
    last_persistence_identity: Option<u64>,
}

struct PendingReconcileTransaction {
    db: Arc<parking_lot::Mutex<Option<Connection>>>,
    active: bool,
}

impl PendingReconcileTransaction {
    fn begin(db: Arc<parking_lot::Mutex<Option<Connection>>>) -> Result<Self, SessionQueryError> {
        {
            let guard = db.lock();
            let connection = guard.as_ref().ok_or_else(index_closed)?;
            connection
                .execute_batch("BEGIN IMMEDIATE")
                .map_err(|error| {
                    SessionQueryError::new(
                        SessionQueryErrorCode::SessionQueryIndexFailed,
                        format!("session-search transaction begin failed: {error}"),
                    )
                })?;
        }
        Ok(Self { db, active: true })
    }

    fn commit(&mut self) -> Result<(), SessionQueryError> {
        let guard = self.db.lock();
        let connection = guard.as_ref().ok_or_else(index_closed)?;
        connection.execute_batch("COMMIT").map_err(|error| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryIndexFailed,
                format!("session-search transaction commit failed: {error}"),
            )
        })?;
        self.active = false;
        Ok(())
    }
}

impl Drop for PendingReconcileTransaction {
    fn drop(&mut self) {
        if self.active {
            if let Some(connection) = self.db.lock().as_ref() {
                let _ = connection.execute_batch("ROLLBACK");
            }
        }
    }
}

/// Concrete SQLite owner of the combined `ctx.sessionQuery` service.
///
/// The `gate` serializes one operation at a time (TS `_tail`); the SQLite
/// handle is only touched in synchronous sections (TS `DatabaseSync`).
pub struct SqliteSearch {
    ctx: Context,
    config: ResolvedConfig,
    binding: Arc<parking_lot::Mutex<PersistenceBinding>>,
    identity_counter: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    gate: tokio::sync::Mutex<()>,
    db: Arc<parking_lot::Mutex<Option<Connection>>>,
    instance: String,
    generations: parking_lot::Mutex<GenState>,
    /// Optional-persistence child fiber keeping the binding cell fresh on
    /// mount and reset on unmount (TS optional inject + child effect).
    _binding_fiber: Option<Arc<cordis::FiberCore>>,
}

impl SqliteSearch {
    /// Build the backend for a validated configuration (TS constructor).
    pub fn new(ctx: &Context, config: ResolvedConfig) -> Result<Arc<Self>, SessionQueryError> {
        let binding = Arc::new(parking_lot::Mutex::new(PersistenceBinding {
            identity: 0,
            service: None,
        }));
        let identity_counter = Arc::new(AtomicU64::new(0));
        let binding_fiber = ctx.inject(
            InjectSpec::new(["sessionPersistence"]),
            Arc::new({
                let binding = binding.clone();
                let counter = identity_counter.clone();
                move |type_ctx: &Context, _config: ArcValue| {
                    let type_ctx = type_ctx.clone();
                    let binding = binding.clone();
                    let counter = counter.clone();
                    Box::pin(async move {
                        let service = type_ctx
                            .get_typed::<Arc<dyn SessionPersistenceApi>>(
                                "sessionPersistence",
                                false,
                            )
                            .map(|slot| slot.as_ref().clone());
                        {
                            let mut cell = binding.lock();
                            let identity = counter.fetch_add(1, Ordering::Relaxed) + 1;
                            *cell = PersistenceBinding { identity, service };
                        }
                        // Reset the binding when the dependency disappears
                        // (TS `childCtx.effect` disposer on unmount).
                        let disposer: Disposer = Arc::new(move || {
                            let binding = binding.clone();
                            let counter = counter.clone();
                            Box::pin(async move {
                                let mut cell = binding.lock();
                                let identity = counter.fetch_add(1, Ordering::Relaxed) + 1;
                                *cell = PersistenceBinding {
                                    identity,
                                    service: None,
                                };
                            })
                        });
                        let _ = type_ctx.effect(
                            "sessionQuerySqlite.persistenceBinding",
                            Box::pin(async move { Some(disposer) }),
                        );
                        Ok(())
                    })
                }
            }),
        );
        Ok(Arc::new(Self {
            ctx: ctx.clone(),
            config,
            binding,
            identity_counter,
            closed: Arc::new(AtomicBool::new(false)),
            gate: tokio::sync::Mutex::new(()),
            db: Arc::new(parking_lot::Mutex::new(None)),
            instance: uuid::Uuid::new_v4().to_string(),
            generations: parking_lot::Mutex::new(GenState::default()),
            _binding_fiber: Some(binding_fiber),
        }))
    }

    /// The validated and defaulted backend configuration.
    pub fn config(&self) -> &ResolvedConfig {
        &self.config
    }

    /// Validate, open on the configured boundary, mount the combined engine
    /// on `ctx.sessionQuery`, and own the close-on-dispose effect (the sync
    /// form of the plugin's apply; the plugin delegates here).
    pub fn install(ctx: &Context, config: &Config) -> Result<Arc<Self>, SessionQueryError> {
        let resolved = resolve_config(config)?;
        let search = SqliteSearch::new(ctx, resolved)?;
        if search.config().open_at == OpenAt::Startup {
            futures::executor::block_on(search.ensure_open(None))?;
        }
        let seam_config = dsh_session_query::Config {
            read_window_max: Some(search.config().read_window_max),
            persisted_inspect_concurrency: Some(search.config().persisted_inspect_concurrency),
        };
        SessionQueryEngine::install(
            ctx,
            &seam_config,
            Some(search.clone() as Arc<dyn SessionQuerySearch>),
        )?;
        let close_disposer: Disposer = Arc::new({
            let search = search.clone();
            move || {
                let search = search.clone();
                Box::pin(async move {
                    search.close().await;
                })
            }
        });
        ctx.effect(
            "sessionQuerySqlite.close",
            Box::pin(async move { Some(close_disposer) }),
        );
        Ok(search)
    }

    /// The binding mutex, exposed for lifecycle tests mirroring the TS
    /// internal `_persistenceBinding` pokes.
    pub(crate) fn binding_cell(&self) -> &parking_lot::Mutex<PersistenceBinding> {
        &self.binding
    }

    /// Refuse full-text calls under `openAt: 'never'` before any request
    /// normalization or SQLite work.
    fn assert_search_enabled(&self) -> Result<(), SessionQueryError> {
        if self.config.open_at != OpenAt::Never {
            return Ok(());
        }
        Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQuerySearchDisabled,
            "session search is disabled: this deployment configures the session-query index with openAt \"never\"",
        ))
    }

    fn sessions_service(&self) -> Result<Arc<SessionStore>, SessionQueryError> {
        self.ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| {
                SessionQueryError::new(
                    SessionQueryErrorCode::SessionQueryIndexFailed,
                    "session-search requires the sessions service",
                )
            })
    }

    /// Read the current optional persistence binding, allocating a fresh
    /// identity whenever the mounted service pointer changes.
    fn current_binding(&self) -> PersistenceBinding {
        let service = self
            .ctx
            .get_typed::<Arc<dyn SessionPersistenceApi>>("sessionPersistence", false)
            .map(|slot| slot.as_ref().clone());
        let mut binding = self.binding.lock();
        let changed = match (&binding.service, &service) {
            (Some(current), Some(next)) => !Arc::ptr_eq(current, next),
            (None, None) => false,
            _ => true,
        };
        if changed {
            let identity = self.identity_counter.fetch_add(1, Ordering::Relaxed) + 1;
            *binding = PersistenceBinding { identity, service };
        }
        binding.clone()
    }

    /// Open the SQLite handle on first use (TS `_open` + `_ensureReady`).
    pub async fn ensure_open(
        self: &Arc<Self>,
        signal: Option<&dsh_session_query::corpus::SessionQueryAbort>,
    ) -> Result<(), SessionQueryError> {
        self.ensure_open_locked(signal).await
    }

    async fn ensure_open_locked(
        &self,
        signal: Option<&dsh_session_query::corpus::SessionQueryAbort>,
    ) -> Result<(), SessionQueryError> {
        if self.db.lock().is_some() {
            return Ok(());
        }
        assert_not_aborted(signal)?;
        let db = open_search_database(&self.config.path, self.config.journal_mode)
            .map_err(|message| index_open_failed(&message))?;
        let global: i64 = db
            .query_row(
                "SELECT global_generation FROM search_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| index_open_failed(&error.to_string()))?;
        {
            let mut generations = self.generations.lock();
            generations.global_generation = global as u64;
            generations.local_generation = global as u64;
        }
        *self.db.lock() = Some(db);
        assert_not_aborted(signal)?;
        Ok(())
    }

    /// Close the database after every accepted operation reaches quiescence.
    pub async fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        let _gate = self.gate.lock().await;
        *self.db.lock() = None;
    }

    /// Reconcile the derived index against the live-preferred corpus.
    async fn reconcile(
        &self,
        signal: Option<&dsh_session_query::corpus::SessionQueryAbort>,
    ) -> Result<PersistenceBinding, SessionQueryError> {
        assert_not_aborted(signal)?;
        let (persisted_rows, live_rows) = {
            let db = self.db.lock();
            let db = db.as_ref().ok_or_else(index_closed)?;
            (read_persisted_rows(db)?, read_live_rows(db)?)
        };
        let persisted_by_id: HashMap<String, &IndexedPersistedRow> = persisted_rows
            .iter()
            .map(|row| (row.id.clone(), row))
            .collect();
        let live_by_id: HashMap<String, &IndexedLiveRow> =
            live_rows.iter().map(|row| (row.id.clone(), row)).collect();
        let observation = self.observe_stable(&persisted_by_id, signal).await?;
        assert_not_aborted(signal)?;
        let binding = &observation.persistence_binding;
        let can_reuse_indexed = self.generations.lock().last_persistence_identity.is_none()
            || self.generations.lock().last_persistence_identity == Some(binding.identity);
        let persistent_changes: Vec<&ObservedPersistedSession> = observation
            .persisted
            .values()
            .filter(|entry| {
                !observation.live.contains_key(entry.header.id.as_str())
                    && !(can_reuse_indexed
                        && persisted_by_id
                            .get(entry.header.id.as_str())
                            .is_some_and(|row| row.revision == entry.revision.as_str()))
            })
            .collect();
        let persistent_deletes: Vec<&IndexedPersistedRow> = persisted_rows
            .iter()
            .filter(|row| !observation.persisted.contains_key(&row.id))
            .collect();
        let live_changes: Vec<&ObservedSession> = observation
            .live
            .values()
            .filter(|entry| {
                let indexed = live_by_id.get(&entry.header.id.as_str().to_string());
                let persisted = if observation.persisted.contains_key(entry.header.id.as_str()) {
                    1
                } else {
                    0
                };
                indexed.is_none_or(|indexed| {
                    indexed.fingerprint != entry.fingerprint || indexed.persisted != persisted
                })
            })
            .collect();
        let live_deletes: Vec<&IndexedLiveRow> = live_rows
            .iter()
            .filter(|row| !observation.live.contains_key(&row.id))
            .collect();
        let pointer_changed = self.generations.lock().last_persistence_identity.is_some()
            && self.generations.lock().last_persistence_identity != Some(binding.identity);
        let has_writes = !persistent_changes.is_empty()
            || !persistent_deletes.is_empty()
            || !live_changes.is_empty()
            || !live_deletes.is_empty();

        let mut next_main_generation = {
            let db = self.db.lock();
            let db = db.as_ref().ok_or_else(index_closed)?;
            main_generation(db)?
        };
        let mut next_local_generation = self.generations.lock().local_generation;
        if !persistent_changes.is_empty() || !persistent_deletes.is_empty() {
            next_main_generation += 1;
        }
        let live_replacements: Vec<(&ObservedSession, u64, bool)> = live_changes
            .iter()
            .map(|entry| {
                next_local_generation = next_local_generation.max(next_main_generation) + 1;
                (
                    *entry,
                    next_local_generation,
                    observation.persisted.contains_key(entry.header.id.as_str()),
                )
            })
            .collect();

        let mut transaction = if has_writes {
            Some(PendingReconcileTransaction::begin(self.db.clone())?)
        } else {
            None
        };

        if let Some(persistence) = binding.service.as_ref() {
            for entry in &persistent_changes {
                assert_not_aborted(signal)?;
                index_persisted_streaming(
                    persistence,
                    self.db.clone(),
                    &entry.header,
                    &entry.revision,
                    next_main_generation,
                )
                .await
                .map_err(|message| {
                    SessionQueryError::new(
                        SessionQueryErrorCode::SessionQueryIndexFailed,
                        format!("session-search streaming index failed: {message}"),
                    )
                })?;
            }
        }

        if has_writes {
            let db = self.db.lock();
            let db = db.as_ref().ok_or_else(index_closed)?;
            for row in &persistent_deletes {
                delete_session(db, false, &row.id).map_err(|error| {
                    SessionQueryError::new(
                        SessionQueryErrorCode::SessionQueryIndexFailed,
                        error.to_string(),
                    )
                })?;
            }
            if !persistent_changes.is_empty() || !persistent_deletes.is_empty() {
                db.execute(
                    "UPDATE search_state SET global_generation = ?1 WHERE singleton = 1",
                    [next_main_generation as i64],
                )
                .map_err(|error| {
                    SessionQueryError::new(
                        SessionQueryErrorCode::SessionQueryIndexFailed,
                        error.to_string(),
                    )
                })?;
            }
            for row in &live_deletes {
                delete_session(db, true, &row.id).map_err(|error| {
                    SessionQueryError::new(
                        SessionQueryErrorCode::SessionQueryIndexFailed,
                        error.to_string(),
                    )
                })?;
            }
            for (entry, generation, persisted) in &live_replacements {
                replace_live_session(db, entry, *generation, *persisted).map_err(|error| {
                    SessionQueryError::new(
                        SessionQueryErrorCode::SessionQueryIndexFailed,
                        error.to_string(),
                    )
                })?;
            }
        }
        if let Some(transaction) = transaction.as_mut() {
            transaction.commit()?;
        }

        {
            let mut generations = self.generations.lock();
            if has_writes || pointer_changed {
                generations.global_generation += 1;
            }
            if pointer_changed {
                generations.persistence_epoch += 1;
            }
            generations.local_generation = next_local_generation;
            generations.last_persistence_identity = Some(binding.identity);
        }
        Ok(binding.clone())
    }

    /// Observe live and persisted sources until one snapshot stays stable.
    async fn observe_stable(
        &self,
        indexed: &HashMap<String, &IndexedPersistedRow>,
        signal: Option<&dsh_session_query::corpus::SessionQueryAbort>,
    ) -> Result<Observation, SessionQueryError> {
        for _attempt in 0..STABLE_OBSERVATION_ATTEMPTS {
            assert_not_aborted(signal)?;
            let persistence_binding = self.current_binding();
            let persistence = persistence_binding.service.clone();
            let sessions = self.sessions_service()?;
            let initially_live: HashSet<String> = sessions
                .list()
                .iter()
                .map(|session| session.id().as_str().to_string())
                .collect();
            let mut persisted: HashMap<String, ObservedPersistedSession> = HashMap::new();
            if let Some(persistence) = &persistence {
                match self
                    .observe_persistence(
                        &persistence,
                        indexed,
                        &initially_live,
                        &sessions,
                        &persistence_binding,
                        signal,
                    )
                    .await?
                {
                    Some(observed) => persisted = observed,
                    None => continue,
                }
            }
            let mut live: HashMap<String, ObservedSession> = HashMap::new();
            for session in sessions.list() {
                let observed = observe_live(&session)?;
                if let Some(durable) = persisted.get(session.id().as_str()) {
                    assert_session_headers_compatible(&observed.header, &durable.header)?;
                }
                live.insert(session.id().as_str().to_string(), observed);
            }
            if !same_session_ids(&initially_live, &live) {
                continue;
            }
            return Ok(Observation {
                persistence_binding,
                persisted,
                live,
            });
        }
        Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryPersistenceFailed,
            "session-search persistence observation did not stabilize after one retry",
        ))
    }

    /// One persisted-source observation pass; `Ok(None)` requests a retry.
    async fn observe_persistence(
        &self,
        persistence: &Arc<dyn SessionPersistenceApi>,
        indexed: &HashMap<String, &IndexedPersistedRow>,
        initially_live: &HashSet<String>,
        sessions: &Arc<SessionStore>,
        binding: &PersistenceBinding,
        signal: Option<&dsh_session_query::corpus::SessionQueryAbort>,
    ) -> Result<Option<HashMap<String, ObservedPersistedSession>>, SessionQueryError> {
        let can_reuse_indexed = self.generations.lock().last_persistence_identity.is_none()
            || self.generations.lock().last_persistence_identity == Some(binding.identity);
        let before = match persistence.list_snapshots().await {
            Ok(snapshots) => snapshots,
            Err(message) => return self.persistence_failure(binding, signal, &message),
        };
        assert_not_aborted(signal)?;
        let persisted = match materialize_snapshots(&before) {
            Ok(observed) => observed,
            Err(message) => return self.persistence_failure(binding, signal, &message),
        };
        // Loading searchable documents is deliberately deferred until after
        // the source snapshot has stabilized. Reconcile then builds and
        // commits one changed session at a time, so this observation map never
        // owns documents from multiple cold sessions.
        let _ = (can_reuse_indexed, indexed, initially_live, sessions);
        assert_not_aborted(signal)?;
        let after = match persistence.list_snapshots().await {
            Ok(snapshots) => snapshots,
            Err(message) => return self.persistence_failure(binding, signal, &message),
        };
        assert_not_aborted(signal)?;
        let after_map = match materialize_snapshots(&after) {
            Ok(observed) => observed,
            Err(message) => return self.persistence_failure(binding, signal, &message),
        };
        if !same_persistence_snapshots(&persisted, &after_map) {
            return Ok(None);
        }
        if self.current_binding().identity != binding.identity {
            return Ok(None);
        }
        Ok(Some(persisted))
    }

    /// Map one persisted-source failure: abort wins, a changed binding
    /// retries, anything else becomes `SESSION_QUERY_PERSISTENCE_FAILED`.
    fn persistence_failure(
        &self,
        binding: &PersistenceBinding,
        signal: Option<&dsh_session_query::corpus::SessionQueryAbort>,
        message: &str,
    ) -> Result<Option<HashMap<String, ObservedPersistedSession>>, SessionQueryError> {
        if aborted(signal) {
            return Err(abort_error());
        }
        if self.current_binding().identity != binding.identity {
            return Ok(None);
        }
        Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryPersistenceFailed,
            format!("session-search persistence observation failed: {message}"),
        ))
    }

    fn query_sessions(
        &self,
        request: &NormalizedSessionRequest,
        offset: u64,
        persistence_visible: bool,
    ) -> Result<Vec<SearchRow>, SessionQueryError> {
        let session_where = build_session_where(&request.session_filters)?;
        let event_where = build_event_where(&request.event_filters)?;
        crate::query::assert_fts5_outer_predicate_count(
            session_where.predicate_count + event_where.predicate_count,
        )?;
        let where_sql = [session_where.sql.as_str(), event_where.sql.as_str()]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" AND ");
        let where_clause = if where_sql.is_empty() {
            String::new()
        } else {
            format!("WHERE {where_sql}")
        };
        let mut bindings = selected_documents_params(&request.query, persistence_visible);
        bindings.extend(session_where.params);
        bindings.extend(event_where.params);
        bindings.push(Binding::Integer(request.limit as i64 + 1));
        bindings.push(Binding::Integer(offset as i64));
        crate::query::assert_portable_binding_count(bindings.len())?;
        let sql = format!(
            "{},
            filtered AS (
              SELECT * FROM matched {where_clause}
            ),
            ranked AS (
              SELECT *, ROW_NUMBER() OVER (
                PARTITION BY session_id
                ORDER BY match_count DESC, document_length ASC, time DESC, seq DESC
              ) AS event_rank
              FROM filtered
            )
            SELECT
              session_id, version, created_at, cwd, parent_session, seed_length,
              delegation_depth, agent_preset, live, persisted, seq, type, time,
              surface, marked_text, match_count, document_length
            FROM ranked
            WHERE event_rank = 1
            ORDER BY match_count DESC, document_length ASC, time DESC, session_id ASC, seq DESC
            LIMIT ? OFFSET ?",
            selected_documents_sql()
        );
        let db = self.db.lock();
        let db = db.as_ref().ok_or_else(index_closed)?;
        query_rows(db, &sql, &bindings)
    }

    fn query_events(
        &self,
        request: &NormalizedEventRequest,
        offset: u64,
        persistence_visible: bool,
    ) -> Result<Vec<SearchRow>, SessionQueryError> {
        let event_where = build_event_where(&request.filters)?;
        crate::query::assert_fts5_outer_predicate_count(1 + event_where.predicate_count)?;
        let where_sql = format!(
            "session_id = ? {}",
            if event_where.sql.is_empty() {
                String::new()
            } else {
                format!("AND {}", event_where.sql)
            }
        );
        let mut bindings = selected_documents_params(&request.query, persistence_visible);
        bindings.push(Binding::Text(request.session_id.as_str().to_string()));
        bindings.extend(event_where.params);
        bindings.push(Binding::Integer(request.limit as i64 + 1));
        bindings.push(Binding::Integer(offset as i64));
        crate::query::assert_portable_binding_count(bindings.len())?;
        let sql = format!(
            "{}
            SELECT
              session_id, version, created_at, cwd, parent_session, seed_length,
              delegation_depth, agent_preset, live, persisted, seq, type, time,
              surface, marked_text, match_count, document_length
            FROM matched
            WHERE {}
            ORDER BY match_count DESC, document_length ASC, time DESC, seq DESC
            LIMIT ? OFFSET ?",
            selected_documents_sql(),
            where_sql
        );
        let db = self.db.lock();
        let db = db.as_ref().ok_or_else(index_closed)?;
        query_rows(db, &sql, &bindings)
    }

    fn target_observation(
        &self,
        session_id: &SessionId,
        binding: &PersistenceBinding,
    ) -> Result<(SessionHeader, String), SessionQueryError> {
        let db = self.db.lock();
        let db = db.as_ref().ok_or_else(index_closed)?;
        let live_row = read_header_row(
            db,
            "SELECT
              id AS session_id, version, created_at, cwd, parent_session, seed_length,
              delegation_depth, agent_preset, generation
            FROM temp.live_sessions
            WHERE id = ?",
            session_id.as_str(),
        )?;
        if let Some((header, generation)) = live_row {
            return Ok((header, format!("live:{generation}")));
        }
        if binding.service.is_some() {
            let persisted_row = read_header_row(
                db,
                "SELECT
                  id AS session_id, version, created_at, cwd, parent_session, seed_length,
                  delegation_depth, agent_preset, generation
                FROM persisted_sessions
                WHERE id = ?",
                session_id.as_str(),
            )?;
            if let Some((header, generation)) = persisted_row {
                return Ok((
                    header,
                    format!(
                        "persisted:{}:{generation}",
                        self.generations.lock().persistence_epoch
                    ),
                ));
            }
        }
        Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQuerySessionNotFound,
            format!("session \"{}\" not found", session_id.as_str()),
        ))
    }

    fn event_hit(&self, row: &SearchRow) -> SessionEventSearchHit {
        SessionEventSearchHit {
            session_id: session_id(&row.session_id),
            seq: row.seq as u64,
            type_: row.type_.clone(),
            time: row.time,
            surface: surface_from_str(&row.surface).unwrap_or(SessionEventSurface::LogOnly),
            snippet: make_snippet(&row.marked_text, self.config.snippet_chars),
        }
    }

    fn session_hit(&self, row: &SearchRow) -> SessionSearchHit {
        SessionSearchHit {
            record: SessionRecord {
                header: row_header(&row.header_row()),
                live: row.live == 1,
                persisted: row.persisted == 1,
            },
            best_match: self.event_hit(row),
        }
    }
}

#[async_trait::async_trait]
impl SessionQuerySearch for SqliteSearch {
    async fn search_sessions(
        &self,
        _engine: &SessionQueryEngine,
        request: &SessionSearchRequest,
        exec: Option<&SessionSearchExecContext>,
    ) -> Result<SessionSearchPage<SessionSearchHit>, SessionQueryError> {
        self.assert_search_enabled()?;
        let normalized = normalize_session_request(request, &self.config)?;
        let signal = exec.as_ref().and_then(|exec| exec.signal.as_ref());
        if self.closed.load(Ordering::Relaxed) {
            return Err(index_closed());
        }
        let _gate = self.gate.lock().await;
        if self.closed.load(Ordering::Relaxed) {
            return Err(index_closed());
        }
        self.ensure_open_locked(signal).await?;
        let binding = self.reconcile(signal).await?;
        assert_not_aborted(signal)?;
        let generation = self.generations.lock().global_generation.to_string();
        let fingerprint = request_fingerprint(&RequestFingerprint::Sessions {
            query: &normalized.query,
            session_filters: &normalized.session_filters,
            event_filters: &normalized.event_filters,
            limit: normalized.limit,
        });
        let offset = match &normalized.cursor {
            None => 0,
            Some(cursor) => decode_cursor(
                cursor,
                &self.instance,
                "sessions",
                &fingerprint,
                &generation,
            )?,
        };
        let rows = self.query_sessions(&normalized, offset, binding.service.is_some())?;
        let (items, next_cursor) = page(&rows, normalized.limit, offset, |cursor_offset| {
            encode_cursor(&CursorPayload {
                version: 1,
                instance: self.instance.clone(),
                scope: "sessions".to_string(),
                fingerprint: fingerprint.clone(),
                generation: generation.clone(),
                offset: cursor_offset,
            })
        });
        Ok(SessionSearchPage {
            items: items.iter().map(|row| self.session_hit(row)).collect(),
            next_cursor,
        })
    }

    async fn search_events(
        &self,
        _engine: &SessionQueryEngine,
        request: &SessionEventSearchRequest,
        exec: Option<&SessionSearchExecContext>,
    ) -> Result<SessionEventSearchPage, SessionQueryError> {
        self.assert_search_enabled()?;
        let normalized = normalize_event_request(request, &self.config)?;
        let signal = exec.as_ref().and_then(|exec| exec.signal.as_ref());
        if self.closed.load(Ordering::Relaxed) {
            return Err(index_closed());
        }
        let _gate = self.gate.lock().await;
        if self.closed.load(Ordering::Relaxed) {
            return Err(index_closed());
        }
        self.ensure_open_locked(signal).await?;
        let binding = self.reconcile(signal).await?;
        assert_not_aborted(signal)?;
        let target = self.target_observation(&normalized.session_id, &binding)?;
        let fingerprint = request_fingerprint(&RequestFingerprint::Events {
            session_id: &normalized.session_id,
            query: &normalized.query,
            filters: &normalized.filters,
            limit: normalized.limit,
        });
        let offset = match &normalized.cursor {
            None => 0,
            Some(cursor) => {
                decode_cursor(cursor, &self.instance, "events", &fingerprint, &target.1)?
            }
        };
        let rows = self.query_events(&normalized, offset, binding.service.is_some())?;
        let (items, next_cursor) = page(&rows, normalized.limit, offset, |cursor_offset| {
            encode_cursor(&CursorPayload {
                version: 1,
                instance: self.instance.clone(),
                scope: "events".to_string(),
                fingerprint: fingerprint.clone(),
                generation: target.1.clone(),
                offset: cursor_offset,
            })
        });
        Ok(SessionEventSearchPage {
            session: target.0,
            items: items.iter().map(|row| self.event_hit(row)).collect(),
            next_cursor,
        })
    }
}

/// Split one result page and mint its continuation cursor.
fn page<F: Fn(u64) -> SessionSearchCursor>(
    rows: &[SearchRow],
    limit: u64,
    offset: u64,
    next_cursor: F,
) -> (Vec<SearchRow>, Option<SessionSearchCursor>) {
    let has_more = rows.len() as u64 > limit;
    let items = rows
        .iter()
        .take(limit as usize)
        .cloned()
        .collect::<Vec<_>>();
    let cursor = if has_more {
        Some(next_cursor(offset + limit))
    } else {
        None
    };
    (items, cursor)
}

fn selected_documents_sql() -> String {
    format!(
        "WITH candidates AS (
          SELECT
            pd.session_id AS session_id,
            ps.version AS version,
            ps.created_at AS created_at,
            ps.cwd AS cwd,
            ps.parent_session AS parent_session,
            ps.seed_length AS seed_length,
            ps.delegation_depth AS delegation_depth,
            ps.agent_preset AS agent_preset,
            0 AS live,
            1 AS persisted,
            CAST(pd.seq AS INTEGER) AS seq,
            pd.type AS type,
            CAST(pd.time AS INTEGER) AS time,
            pd.surface AS surface,
            highlight(persisted_docs, 0, ?, ?) AS marked_text,
            CAST(pd.codepoint_length AS INTEGER) AS document_length
          FROM persisted_docs AS pd
          JOIN persisted_sessions AS ps ON ps.id = pd.session_id
          WHERE persisted_docs MATCH ?
            AND ? = 1
            AND NOT EXISTS (SELECT 1 FROM temp.live_sessions AS ls WHERE ls.id = pd.session_id)
          UNION ALL
          SELECT
            ld.session_id AS session_id,
            ls.version AS version,
            ls.created_at AS created_at,
            ls.cwd AS cwd,
            ls.parent_session AS parent_session,
            ls.seed_length AS seed_length,
            ls.delegation_depth AS delegation_depth,
            ls.agent_preset AS agent_preset,
            1 AS live,
            CASE WHEN ? = 1 THEN ls.persisted ELSE 0 END AS persisted,
            CAST(ld.seq AS INTEGER) AS seq,
            ld.type AS type,
            CAST(ld.time AS INTEGER) AS time,
            ld.surface AS surface,
            highlight(live_docs, 0, ?, ?) AS marked_text,
            CAST(ld.codepoint_length AS INTEGER) AS document_length
          FROM temp.live_docs AS ld
          JOIN temp.live_sessions AS ls ON ls.id = ld.session_id
          WHERE live_docs MATCH ?
        ), matched AS (
          SELECT *,
            (
              length(CAST(marked_text AS BLOB))
              - length(CAST(replace(marked_text, ?, '') AS BLOB))
            ) / ? AS match_count
          FROM candidates
        )"
    )
}

fn selected_documents_params(query: &str, persistence_visible: bool) -> Vec<Binding> {
    let expression = quote_fts_data(query);
    let visible = if persistence_visible { 1 } else { 0 };
    vec![
        Binding::Text(FTS_HIGHLIGHT_START.to_string()),
        Binding::Text(FTS_HIGHLIGHT_END.to_string()),
        Binding::Text(expression.clone()),
        Binding::Integer(visible),
        Binding::Integer(visible),
        Binding::Text(FTS_HIGHLIGHT_START.to_string()),
        Binding::Text(FTS_HIGHLIGHT_END.to_string()),
        Binding::Text(expression),
        Binding::Text(FTS_HIGHLIGHT_START.to_string()),
        Binding::Integer(FTS_HIGHLIGHT_START.len_utf8() as i64),
    ]
}

fn observe_live(session: &Session) -> Result<ObservedSession, SessionQueryError> {
    observe_session(session.header(), &session.events())
}

async fn index_persisted_streaming(
    persistence: &Arc<dyn SessionPersistenceApi>,
    db: Arc<parking_lot::Mutex<Option<Connection>>>,
    header: &SessionHeader,
    revision: &SessionPersistenceRevision,
    generation: u64,
) -> Result<(), String> {
    const CHUNK_EVENTS: usize = 1_024;
    let fold = Arc::new(parking_lot::Mutex::new(StreamingSurfaceFold::default()));
    let fold_visitor = {
        let fold = fold.clone();
        Arc::new(move |events: &[SessionEvent]| {
            let mut fold = fold.lock();
            for event in events {
                fold.push(event)?;
            }
            Ok(())
        })
    };
    persistence
        .visit_event_chunks(&header.id, CHUNK_EVENTS, fold_visitor)
        .await?;
    let folded = std::mem::take(&mut *fold.lock()).finish();
    let mut surface_by_seq = HashMap::new();
    for seq in folded.nodes {
        surface_by_seq.insert(seq, SessionEventSurface::Current);
    }
    for replacement in folded.replacements {
        for seq in replacement.shadowed_seqs {
            surface_by_seq.insert(seq, SessionEventSurface::Shadowed);
        }
    }
    let surface_by_seq = Arc::new(surface_by_seq);
    {
        let db_guard = db.lock();
        let connection = db_guard
            .as_ref()
            .ok_or_else(|| "search index is closed".to_string())?;
        delete_session(connection, false, header.id.as_str())
            .and_then(|_| insert_persisted_header(connection, header, revision, generation))
            .map_err(|error| error.to_string())?;
    }
    let document_visitor = {
        let session_id = header.id.clone();
        let surface_by_seq = surface_by_seq.clone();
        let db = db.clone();
        Arc::new(move |events: &[SessionEvent]| {
            let db_guard = db.lock();
            let connection = db_guard
                .as_ref()
                .ok_or_else(|| "search index is closed".to_string())?;
            for event in events {
                let text = dsh_session_query::extract_session_event_text(event);
                if text.is_empty() {
                    continue;
                }
                let document = dsh_session_query::SessionEventSearchDocument {
                    session_id: session_id.clone(),
                    seq: event.seq.get(),
                    type_: event.type_.clone(),
                    time: event.time,
                    surface: surface_by_seq
                        .get(&event.seq.get())
                        .copied()
                        .unwrap_or(SessionEventSurface::LogOnly),
                    text,
                };
                insert_document(connection, "persisted_docs", &document)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        })
    };
    persistence
        .visit_event_chunks(&header.id, CHUNK_EVENTS, document_visitor)
        .await?;
    let current = persistence.read_snapshot(&header.id).await?;
    if current.as_ref().map(|snapshot| &snapshot.revision) != Some(revision) {
        return Err(format!(
            "session-search source changed while indexing session \"{}\"",
            header.id
        ));
    }
    Ok(())
}

fn observe_session(
    header: &SessionHeader,
    events: &[SessionEvent],
) -> Result<ObservedSession, SessionQueryError> {
    let documents = build_session_event_search_documents(&header.id, events)?;
    let fingerprint_json = serde_json::json!({ "header": header, "events": events });
    let encoded = serde_json::to_string(&fingerprint_json).expect("fingerprint json");
    let digest = sha2::Sha256::digest(encoded.as_bytes());
    use base64::Engine;
    let fingerprint = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    Ok(ObservedSession {
        header: header.clone(),
        documents,
        fingerprint,
    })
}

fn materialize_snapshots(
    snapshots: &[dsh_session_persistence::SessionPersistenceSnapshot],
) -> Result<HashMap<String, ObservedPersistedSession>, String> {
    let mut result: HashMap<String, ObservedPersistedSession> = HashMap::new();
    for snapshot in snapshots {
        if result.contains_key(snapshot.header.id.as_str()) {
            return Err(format!(
                "persistence listed duplicate session \"{}\"",
                snapshot.header.id.as_str()
            ));
        }
        result.insert(
            snapshot.header.id.as_str().to_string(),
            ObservedPersistedSession {
                header: snapshot.header.clone(),
                revision: snapshot.revision.clone(),
            },
        );
    }
    Ok(result)
}

fn same_persistence_snapshots(
    before: &HashMap<String, ObservedPersistedSession>,
    after: &HashMap<String, ObservedPersistedSession>,
) -> bool {
    if before.len() != after.len() {
        return false;
    }
    for (id, first) in before {
        let Some(second) = after.get(id) else {
            return false;
        };
        if first.revision != second.revision || !same_header(&first.header, &second.header) {
            return false;
        }
    }
    true
}

fn same_session_ids(before: &HashSet<String>, after: &HashMap<String, ObservedSession>) -> bool {
    if before.len() != after.len() {
        return false;
    }
    before.iter().all(|id| after.contains_key(id))
}

fn same_header(a: &SessionHeader, b: &SessionHeader) -> bool {
    a.version == b.version
        && a.id == b.id
        && a.created_at == b.created_at
        && a.cwd == b.cwd
        && a.parent_session == b.parent_session
        && a.is_seeded == b.is_seeded
        && a.delegation_depth.unwrap_or(0) == b.delegation_depth.unwrap_or(0)
        && a.agent_preset == b.agent_preset
}

/// The header columns both session upserts bind. `Option` columns become
/// SQLite NULL at statement level (see `option_params`).
fn header_option_bindings(header: &SessionHeader) -> Vec<Option<Binding>> {
    vec![
        Some(Binding::Text(header.id.as_str().to_string())),
        Some(Binding::Integer(header.version as i64)),
        Some(Binding::Integer(header.created_at as i64)),
        header.cwd.clone().map(Binding::Text),
        header
            .parent_session
            .as_ref()
            .map(|id| Binding::Text(id.as_str().to_string())),
        header.is_seeded.then_some(Binding::Integer(0)),
        header.delegation_depth.map(|v| Binding::Integer(v as i64)),
        header.agent_preset.clone().map(Binding::Text),
    ]
}

fn option_params(values: Vec<Option<Binding>>) -> Vec<rusqlite::types::Value> {
    values
        .into_iter()
        .map(|value| match value {
            Some(binding) => binding.to_sql_value(),
            None => rusqlite::types::Value::Null,
        })
        .collect()
}

fn delete_session(db: &Connection, live: bool, id: &str) -> Result<(), rusqlite::Error> {
    let (docs, sessions) = if live {
        ("temp.live_docs", "temp.live_sessions")
    } else {
        ("persisted_docs", "persisted_sessions")
    };
    db.execute(&format!("DELETE FROM {docs} WHERE session_id = ?1"), [id])?;
    db.execute(&format!("DELETE FROM {sessions} WHERE id = ?1"), [id])?;
    Ok(())
}

fn insert_persisted_header(
    db: &Connection,
    header: &SessionHeader,
    revision: &SessionPersistenceRevision,
    generation: u64,
) -> Result<(), rusqlite::Error> {
    let mut bindings = option_params(header_option_bindings(header));
    bindings.push(rusqlite::types::Value::Text(revision.as_str().to_string()));
    bindings.push(rusqlite::types::Value::Integer(generation as i64));
    db.execute(
        "INSERT INTO persisted_sessions
          (id, version, created_at, cwd, parent_session, seed_length, delegation_depth, agent_preset, revision, generation)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params_from_iter(bindings.iter()),
    )?;
    Ok(())
}

fn replace_live_session(
    db: &Connection,
    entry: &ObservedSession,
    generation: u64,
    persisted: bool,
) -> Result<(), rusqlite::Error> {
    delete_session(db, true, entry.header.id.as_str())?;
    let mut bindings = option_params(header_option_bindings(&entry.header));
    bindings.push(rusqlite::types::Value::Text(entry.fingerprint.clone()));
    bindings.push(rusqlite::types::Value::Integer(if persisted {
        1
    } else {
        0
    }));
    bindings.push(rusqlite::types::Value::Integer(generation as i64));
    db.execute(
        "INSERT INTO temp.live_sessions
          (id, version, created_at, cwd, parent_session, seed_length, delegation_depth, agent_preset, fingerprint, persisted, generation)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params_from_iter(bindings.iter()),
    )?;
    insert_documents(db, "temp.live_docs", entry)?;
    Ok(())
}

fn insert_document(
    db: &Connection,
    table: &str,
    document: &dsh_session_query::SessionEventSearchDocument,
) -> Result<(), rusqlite::Error> {
    let text = sanitize_fts_text(&document.text);
    let length = text.chars().count() as i64;
    db.execute(
        &format!(
            "INSERT INTO {table} (text, session_id, seq, type, time, surface, codepoint_length)
            VALUES (?, ?, ?, ?, ?, ?, ?)"
        ),
        rusqlite::params![
            text,
            document.session_id.as_str().to_string(),
            document.seq as i64,
            document.type_.clone(),
            document.time,
            document.surface.as_str(),
            length,
        ],
    )?;
    Ok(())
}

fn insert_documents(
    db: &Connection,
    table: &str,
    entry: &ObservedSession,
) -> Result<(), rusqlite::Error> {
    for document in &entry.documents {
        insert_document(db, table, document)?;
    }
    Ok(())
}

fn main_generation(db: &Connection) -> Result<u64, SessionQueryError> {
    db.query_row(
        "SELECT global_generation FROM search_state WHERE singleton = 1",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value as u64)
    .map_err(|error| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryIndexFailed,
            format!("session-search reconciliation failed: {error}"),
        )
    })
}

fn read_persisted_rows(db: &Connection) -> Result<Vec<IndexedPersistedRow>, SessionQueryError> {
    let mut statement = db
        .prepare("SELECT id, revision, generation FROM persisted_sessions")
        .map_err(|error| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryIndexFailed,
                format!("session-search reconciliation failed: {error}"),
            )
        })?;
    let rows = statement
        .query_map([], |row| {
            Ok(IndexedPersistedRow {
                id: row.get(0)?,
                revision: row.get(1)?,
                _generation: row.get::<_, i64>(2)? as u64,
            })
        })
        .map_err(|error| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryIndexFailed,
                format!("session-search reconciliation failed: {error}"),
            )
        })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryIndexFailed,
            format!("session-search reconciliation failed: {error}"),
        )
    })
}

fn read_live_rows(db: &Connection) -> Result<Vec<IndexedLiveRow>, SessionQueryError> {
    let mut statement = db
        .prepare("SELECT id, fingerprint, persisted, generation FROM temp.live_sessions")
        .map_err(|error| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryIndexFailed,
                format!("session-search reconciliation failed: {error}"),
            )
        })?;
    let rows = statement
        .query_map([], |row| {
            Ok(IndexedLiveRow {
                id: row.get(0)?,
                fingerprint: row.get(1)?,
                persisted: row.get(2)?,
                _generation: row.get::<_, i64>(3)? as u64,
            })
        })
        .map_err(|error| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryIndexFailed,
                format!("session-search reconciliation failed: {error}"),
            )
        })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryIndexFailed,
            format!("session-search reconciliation failed: {error}"),
        )
    })
}

fn read_header_row(
    db: &Connection,
    sql: &str,
    id: &str,
) -> Result<Option<(SessionHeader, i64)>, SessionQueryError> {
    let mut statement = db.prepare(sql).map_err(|error| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryIndexFailed,
            format!("session-search query failed: {error}"),
        )
    })?;
    let row = statement
        .query_row([id], |row| {
            Ok((
                SessionHeader {
                    version: row.get::<_, i64>(1)? as u64,
                    id: session_id(row.get::<_, String>(0)?),
                    created_at: row.get::<_, i64>(2)? as u64,
                    cwd: row.get(3)?,
                    parent_session: row.get::<_, Option<String>>(4)?.map(session_id),
                    is_seeded: row.get::<_, Option<i64>>(5)?.is_some(),
                    origin: None,
                    delegation_depth: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                    agent_preset: row.get(7)?,
                },
                row.get::<_, i64>(8)?,
            ))
        })
        .optional()
        .map_err(|error| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryIndexFailed,
                format!("session-search query failed: {error}"),
            )
        })?;
    Ok(row)
}

fn query_rows(
    db: &Connection,
    sql: &str,
    bindings: &[Binding],
) -> Result<Vec<SearchRow>, SessionQueryError> {
    let values = bindings
        .iter()
        .map(Binding::to_sql_value)
        .collect::<Vec<_>>();
    let mut statement = db.prepare(sql).map_err(|error| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryIndexFailed,
            format!("session-search query failed: {error}"),
        )
    })?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok(SearchRow {
                session_id: row.get(0)?,
                version: row.get(1)?,
                created_at: row.get(2)?,
                cwd: row.get(3)?,
                parent_session: row.get(4)?,
                seed_length: row.get(5)?,
                delegation_depth: row.get(6)?,
                agent_preset: row.get(7)?,
                live: row.get(8)?,
                persisted: row.get(9)?,
                seq: row.get(10)?,
                type_: row.get(11)?,
                time: row.get(12)?,
                surface: row.get(13)?,
                marked_text: row.get(14)?,
                match_count: row.get(15)?,
                document_length: row.get(16)?,
            })
        })
        .map_err(|error| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryIndexFailed,
                format!("session-search query failed: {error}"),
            )
        })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryIndexFailed,
            format!("session-search query failed: {error}"),
        )
    })
}

fn row_header(row: &SessionHeaderRow) -> SessionHeader {
    SessionHeader {
        version: row.version as u64,
        id: session_id(&row.session_id),
        created_at: row.created_at as u64,
        cwd: row.cwd.clone(),
        parent_session: row.parent_session.as_ref().map(session_id),
        is_seeded: row.seed_length.is_some(),
        origin: None,
        delegation_depth: row.delegation_depth.map(|value| value as u64),
        agent_preset: row.agent_preset.clone(),
    }
}

/// The Cordis plugin form (TS loader default export).
pub struct SqliteSessionQueryPlugin;

#[async_trait::async_trait]
impl Plugin for SqliteSessionQueryPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("session-query-sqlite")
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["sessions"])
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config.downcast_ref::<Config>().cloned().unwrap_or_default();
        SqliteSearch::install(ctx, &config)
            .map(|_| ())
            .map_err(|error| PluginError::from(anyhow::anyhow!(error.message)))
    }
}
