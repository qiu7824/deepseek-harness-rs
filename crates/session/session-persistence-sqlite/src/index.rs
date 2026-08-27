//! SQLite durable session-persistence backend. Rust port of
//! `packages/session/session-persistence-sqlite/src/index.ts`.
//!
//! It maps each session header and event to rows and delegates write-path
//! orchestration to [`PersistenceCoordinator`]. It has no independent
//! per-session artifact, so its locator returns `None`.
//!
//! # Deviations
//!
//! - The backend runs SQLite synchronously under one connection mutex (the
//!   TS `node:sqlite` `DatabaseSync` equivalent); database open is
//!   off-thread (`spawn_blocking`) so plugin construction does not block.
//! - AbortSignal parameters of the TS API have no Rust counterpart and are
//!   omitted (consistent with the rest of the port).
//! - The file identity prefix uses `dev:ino:created_ns` on unix and
//!   `len:created_ns` on Windows (Rust `std` exposes no inode there);
//!   `birthtimeNs` is approximated by `created()`.

use std::path::Path;
use std::sync::Arc;

use cordis::{Context, Service};
use dsh_session::{SessionEvent, SessionHeader, SessionId, SessionPreparation};
use dsh_session_persistence::{
    DEFAULT_PREPARED_SESSION_CACHE_SIZE, DEFAULT_WRITE_BATCH_MAX_DELAY_MS,
    MAX_WRITE_BATCH_DELAY_MS, PersistenceBackend, PersistenceCoordinator,
    PersistenceCoordinatorOptions, SessionReadWindowRequest, SessionReadWindowResult, StoredPrefix,
    StoredSuffix,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use crate::schema::{
    EventRow, JournalMode, ScanRowsResult, SessionRow, open_database, row_to_event, row_to_meta,
    scan_rows,
};

/// Plugin configuration (TS `Config`).
#[derive(Debug, Clone)]
pub struct SqliteConfig {
    /// Filesystem path to the SQLite database file, or `:memory:`.
    pub path: String,
    /// SQLite `journal_mode` pragma.
    pub journal_mode: JournalMode,
    /// Maximum cold Session preparations retained.
    pub prepared_session_cache_size: usize,
    /// Fixed live-event coalescing window.
    pub write_batch_max_delay_ms: u64,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            journal_mode: JournalMode::Wal,
            prepared_session_cache_size: DEFAULT_PREPARED_SESSION_CACHE_SIZE,
            write_batch_max_delay_ms: DEFAULT_WRITE_BATCH_MAX_DELAY_MS,
        }
    }
}

/// Parse a loader-supplied JSON config (path required).
pub fn parse_config(value: &serde_json::Value) -> Result<SqliteConfig, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "sqlite config must be an object".to_string())?;
    let path = object
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "sqlite config path is required".to_string())?
        .to_string();
    let journal_mode = match object.get("journalMode") {
        None | Some(serde_json::Value::Null) => JournalMode::Wal,
        Some(serde_json::Value::String(v)) => match v.as_str() {
            "wal" => JournalMode::Wal,
            "delete" => JournalMode::Delete,
            "truncate" => JournalMode::Truncate,
            "persist" => JournalMode::Persist,
            _ => {
                return Err(
                    "journalMode must be \"wal\", \"delete\", \"truncate\", or \"persist\""
                        .to_string(),
                );
            }
        },
        Some(_) => return Err("journalMode must be a string".to_string()),
    };
    let prepared_session_cache_size = object
        .get("preparedSessionCacheSize")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_PREPARED_SESSION_CACHE_SIZE as u64);
    if prepared_session_cache_size < 1 {
        return Err("preparedSessionCacheSize must be an integer >= 1".to_string());
    }
    let write_batch_max_delay_ms = object
        .get("writeBatchMaxDelayMs")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_WRITE_BATCH_MAX_DELAY_MS);
    if !(1..=MAX_WRITE_BATCH_DELAY_MS).contains(&write_batch_max_delay_ms) {
        return Err(format!(
            "writeBatchMaxDelayMs must be an integer between 1 and {MAX_WRITE_BATCH_DELAY_MS}"
        ));
    }
    Ok(SqliteConfig {
        path,
        journal_mode,
        prepared_session_cache_size: prepared_session_cache_size as usize,
        write_batch_max_delay_ms,
    })
}

/// Serialize an event's optional envelope fields for SQL binding (TS
/// `envelopeBindings`).
fn envelope_bindings(event: &SessionEvent) -> (Option<String>, Option<String>, Option<i64>) {
    (
        event
            .source_event_seqs
            .as_ref()
            .map(|seqs| serde_json::to_string(seqs).expect("number array is JSON-serializable")),
        event
            .surface_op
            .as_ref()
            .map(|op| serde_json::to_string(op).expect("surface op is JSON-serializable")),
        (event.ignorable == Some(true)).then_some(1),
    )
}

/// Build the source-qualified revision shared by full and lightweight reads
/// (TS `sqliteRevision`).
fn sqlite_revision(
    store_identity: &str,
    row: &SessionRow,
) -> dsh_session_persistence::SessionPersistenceRevision {
    dsh_session_persistence::session_persistence_revision(format!(
        "{store_identity}:incarnation:{}:revision:{}",
        row.incarnation, row.revision
    ))
}

/// The SQLite persistence backend (registers as `ctx.sessionPersistence`).
pub struct SqliteSessionPersistence {
    ctx: Context,
    path: String,
    journal_mode: JournalMode,
    db: Arc<Mutex<Option<Connection>>>,
    store_identity: Arc<Mutex<Option<String>>>,
    ready: tokio::sync::OnceCell<Result<(), String>>,
    coordinator: Mutex<Option<Arc<PersistenceCoordinator<u64>>>>,
}

impl SqliteSessionPersistence {
    /// Create the backend, register the service, and build the coordinator.
    pub fn install(ctx: &Context, config: SqliteConfig) -> Result<Arc<Self>, String> {
        if config.path.is_empty() {
            return Err("sqlite config path is required".to_string());
        }
        let backend = Arc::new(Self {
            ctx: ctx.clone(),
            path: config.path,
            journal_mode: config.journal_mode,
            db: Arc::new(Mutex::new(None)),
            store_identity: Arc::new(Mutex::new(None)),
            ready: tokio::sync::OnceCell::new(),
            coordinator: Mutex::new(None),
        });
        // Register the ERASED service shape: the session-query seam and the
        // schedule/corpus consumers observe `Arc<dyn SessionPersistenceApi>`.
        let erased: Arc<dyn dsh_session_persistence::SessionPersistenceApi> = backend.clone();
        ctx.register_service(erased);
        let coordinator = PersistenceCoordinator::new(
            ctx,
            backend.clone(),
            PersistenceCoordinatorOptions {
                prepared_session_cache_size: config.prepared_session_cache_size,
                write_batch_max_delay_ms: config.write_batch_max_delay_ms,
            },
        );
        *backend.coordinator.lock() = Some(coordinator);
        Ok(backend)
    }

    fn coordinator(&self) -> Arc<PersistenceCoordinator<u64>> {
        self.coordinator
            .lock()
            .as_ref()
            .expect("coordinator installed")
            .clone()
    }

    /// Await the shared one-shot database open (TS `this.ready`).
    async fn ensure_ready(&self) -> Result<(), String> {
        let ready = self.ready.get_or_init(|| {
            let db = Arc::clone(&self.db);
            let identity = Arc::clone(&self.store_identity);
            let path = self.path.clone();
            let journal_mode = self.journal_mode;
            async move { open_db_owned(path, journal_mode, db, identity).await }
        });
        ready.await.clone()
    }
}

/// The one-shot database open (TS `this.ready = this.openDb(...)`), off the
/// async runtime thread; every storage hook awaits it through
/// [`SqliteSessionPersistence::ensure_ready`].
async fn open_db_owned(
    path: String,
    journal_mode: JournalMode,
    db: Arc<Mutex<Option<Connection>>>,
    identity: Arc<Mutex<Option<String>>>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || open_db_blocking(&path, journal_mode, &db, &identity))
        .await
        .map_err(|error| format!("sqlite database open failed: {error}"))?
}

fn open_db_blocking(
    path: &str,
    journal_mode: JournalMode,
    db: &Mutex<Option<Connection>>,
    store_identity: &Mutex<Option<String>>,
) -> Result<(), String> {
    let actual = if path == ":memory:" {
        path.to_string()
    } else {
        std::path::absolute(path)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .to_string()
    };
    if actual != ":memory:" {
        let file = Path::new(&actual);
        let parent = file
            .parent()
            .ok_or_else(|| format!("session database at \"{actual}\" has no parent directory"))?;
        create_parent_dirs(parent)?;
        create_database_file(file)?;
    }
    let connection = open_database(&actual, journal_mode)?;
    let identity = match read_store_identity(&connection, &actual) {
        Ok(identity) => identity,
        Err(error) => {
            drop(connection);
            return Err(error);
        }
    };
    *store_identity.lock() = Some(identity);
    *db.lock() = Some(connection);
    Ok(())
}

fn create_parent_dirs(parent: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(parent)
            .map_err(|error| error.to_string())
            .map(|_| ())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())
    }
}

/// Exclusively create a missing database file with owner-only permissions
/// (TS `createDatabaseFile`).
fn create_database_file(path: &Path) -> Result<(), String> {
    let open = || -> Result<std::fs::File, std::io::Error> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
        }
        #[cfg(not(unix))]
        {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
        }
    };
    match open() {
        Ok(file) => {
            drop(file);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn read_store_identity(db: &Connection, actual: &str) -> Result<String, String> {
    let store_id: Option<String> = db
        .query_row(
            "SELECT store_id FROM persistence_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(store_id) = store_id else {
        return Err(format!(
            "session database at \"{actual}\" has no store identity"
        ));
    };
    if store_id.is_empty() {
        return Err(format!(
            "session database at \"{actual}\" has no valid store identity"
        ));
    }
    if actual == ":memory:" {
        return Ok(format!("memory:store:{store_id}"));
    }
    let metadata = std::fs::metadata(actual).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let created = metadata
            .created()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        Ok(format!(
            "file:{}:{}:{}:store:{}",
            metadata.dev(),
            metadata.ino(),
            created,
            store_id
        ))
    }
    #[cfg(not(unix))]
    {
        let created = metadata
            .created()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        Ok(format!(
            "file:{}:{}:store:{}",
            metadata.len(),
            created,
            store_id
        ))
    }
}

impl SqliteSessionPersistence {
    /// Run one synchronous operation under the connection mutex.
    fn with_db<R>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<R, String>,
    ) -> Result<R, String> {
        let guard = self.db.lock();
        let db = guard
            .as_ref()
            .ok_or_else(|| "sqlite database is not open".to_string())?;
        operation(db)
    }

    fn store_identity(&self) -> String {
        self.store_identity
            .lock()
            .as_ref()
            .expect("store identity ready")
            .clone()
    }

    /// Fetch a session's row, or None if absent (TS `rowFor`).
    fn row_for(&self, db: &Connection, id: &SessionId) -> Result<Option<SessionRow>, String> {
        let row = db
            .query_row(
                "SELECT id, version, created_at, cwd, parent_session, seed_length, origin, incarnation, revision, delegation_depth, agent_preset FROM sessions WHERE id = ?1",
                [id.as_str()],
                |row| {
                    Ok(SessionRow {
                        id: row.get(0)?,
                        version: row.get(1)?,
                        created_at: row.get(2)?,
                        cwd: row.get(3)?,
                        parent_session: row.get(4)?,
                        seed_length: row.get(5)?,
                        origin: row.get(6)?,
                        incarnation: row.get(7)?,
                        revision: row.get(8)?,
                        delegation_depth: row.get(9)?,
                        agent_preset: row.get(10)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        Ok(row)
    }

    fn event_rows(&self, db: &Connection, id: &SessionId) -> Result<Vec<EventRow>, String> {
        self.event_rows_from(db, id, 0)
    }

    fn event_rows_from(
        &self,
        db: &Connection,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<Vec<EventRow>, String> {
        let mut statement = db
            .prepare(
                "SELECT seq, type, time, data, source_event_seqs, surface_op, ignorable FROM events WHERE session_id = ?1 AND seq >= ?2 ORDER BY seq",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![id.as_str(), from_seq as i64], |row| {
                Ok(EventRow {
                    seq: row.get(0)?,
                    type_: row.get(1)?,
                    time: row.get(2)?,
                    data: row.get(3)?,
                    source_event_seqs: row.get(4)?,
                    surface_op: row.get(5)?,
                    ignorable: row.get(6)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn history_window_cut(
        &self,
        db: &Connection,
        id: &SessionId,
        before_seq: u64,
        max_messages: u64,
    ) -> Result<u64, String> {
        let mut statement = db
            .prepare(
                "SELECT seq, source_event_seqs, surface_op FROM events \
                 WHERE session_id = ?1 AND seq < ?2 \
                 AND type IN ('user/message', 'assistant/message') ORDER BY seq DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![id.as_str(), before_seq as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        let mut count = 0_u64;
        for row in rows {
            let (seq, sources, surface_op) = row.map_err(|error| error.to_string())?;
            let surface_op = surface_op
                .as_deref()
                .map(serde_json::from_str::<dsh_session::SurfaceOp>)
                .transpose()
                .map_err(|error| format!("stored surfaceOp is not valid JSON: {error}"))?;
            if !surface_op.as_ref().is_none_or(|op| op.is_append()) {
                continue;
            }
            count += 1;
            if count >= max_messages.max(1) {
                let seq = u64::try_from(seq)
                    .map_err(|_| format!("stored session event seq is negative: {seq}"))?;
                let source_min = sources
                    .as_deref()
                    .map(serde_json::from_str::<Vec<u64>>)
                    .transpose()
                    .map_err(|error| format!("stored sourceEventSeqs is not valid JSON: {error}"))?
                    .and_then(|values| values.into_iter().min());
                return Ok(source_min.unwrap_or(seq).min(seq));
            }
        }
        Ok(0)
    }

    fn history_event_count(
        &self,
        db: &Connection,
        id: &SessionId,
        start_seq: u64,
        before_seq: u64,
    ) -> Result<usize, String> {
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND seq >= ?2 AND seq < ?3",
                params![id.as_str(), start_seq as i64, before_seq as i64],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        usize::try_from(count).map_err(|_| format!("history event count is invalid: {count}"))
    }

    fn event_rows_range(
        &self,
        db: &Connection,
        id: &SessionId,
        start_seq: u64,
        before_seq: u64,
    ) -> Result<Vec<EventRow>, String> {
        let mut statement = db
            .prepare(
                "SELECT seq, type, time, data, source_event_seqs, surface_op, ignorable \
                 FROM events WHERE session_id = ?1 AND seq >= ?2 AND seq < ?3 ORDER BY seq",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(
                params![id.as_str(), start_seq as i64, before_seq as i64],
                |row| {
                    Ok(EventRow {
                        seq: row.get(0)?,
                        type_: row.get(1)?,
                        time: row.get(2)?,
                        data: row.get(3)?,
                        source_event_seqs: row.get(4)?,
                        surface_op: row.get(5)?,
                        ignorable: row.get(6)?,
                    })
                },
            )
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn all_session_rows(&self, db: &Connection) -> Result<Vec<SessionRow>, String> {
        let mut statement = db
            .prepare(
                "SELECT id, version, created_at, cwd, parent_session, seed_length, origin, incarnation, revision, delegation_depth, agent_preset FROM sessions",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok(SessionRow {
                    id: row.get(0)?,
                    version: row.get(1)?,
                    created_at: row.get(2)?,
                    cwd: row.get(3)?,
                    parent_session: row.get(4)?,
                    seed_length: row.get(5)?,
                    origin: row.get(6)?,
                    incarnation: row.get(7)?,
                    revision: row.get(8)?,
                    delegation_depth: row.get(9)?,
                    agent_preset: row.get(10)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    /// Read a stored prefix by id (ids are globally unique — no scope to
    /// scan). Mirrors the TS `readPrefix` BEGIN/COMMIT snapshot.
    async fn read_prefix(&self, id: &SessionId) -> Result<Option<StoredPrefix<u64>>, String> {
        self.ensure_ready().await?;
        let identity = self.store_identity();

        self.with_db(|db| {
            db.execute_batch("BEGIN")
                .map_err(|error| error.to_string())?;
            let outcome = (|| -> Result<Option<StoredPrefix<u64>>, String> {
                let Some(row) = self.row_for(db, id)? else {
                    return Ok(None);
                };
                let event_rows = self.event_rows(db, id)?;
                let scan = scan_rows(&event_rows, 0)?;
                Ok(Some(StoredPrefix {
                    meta: row_to_meta(&row)?,
                    events: scan.preserved,
                    revision: sqlite_revision(&identity, &row),
                    torn_marker: scan.torn_from,
                }))
            })();
            match outcome {
                Ok(value) => {
                    db.execute_batch("COMMIT")
                        .map_err(|error| error.to_string())?;
                    Ok(value)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    fn insert_event(
        &self,
        db: &Connection,
        id: &SessionId,
        event: &SessionEvent,
    ) -> Result<(), String> {
        let (surface_seqs, surface_op, ignorable) = envelope_bindings(event);
        db.execute(
            "INSERT INTO events (session_id, seq, type, time, data, source_event_seqs, surface_op, ignorable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id.as_str(),
                event.seq as i64,
                event.type_,
                { event.time },
                serde_json::to_string(&event.data).expect("session event data is lossless JSON"),
                surface_seqs,
                surface_op,
                ignorable,
            ],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Insert-or-replace a session's metadata row (TS `writeRow`); writing
    /// the row IS the materialization.
    fn write_row(&self, db: &Connection, meta: &SessionHeader) -> Result<(), String> {
        db.execute(
            "INSERT INTO sessions
        (id, version, created_at, cwd, parent_session, seed_length, origin, delegation_depth, agent_preset, incarnation, revision)
      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)
      ON CONFLICT(id) DO UPDATE SET
        version = excluded.version,
        created_at = excluded.created_at,
        cwd = excluded.cwd,
        parent_session = excluded.parent_session,
        seed_length = excluded.seed_length,
        origin = excluded.origin,
        delegation_depth = excluded.delegation_depth,
        agent_preset = excluded.agent_preset",
            params![
                meta.id.as_str(),
                meta.version as i64,
                meta.created_at as i64,
                meta.cwd,
                meta.parent_session.as_ref().map(|id| id.as_str()),
                meta.seed_length.map(|value| value as i64),
                meta.origin,
                meta.delegation_depth.map(|value| value as i64),
                meta.agent_preset,
                uuid::Uuid::new_v4().simple().to_string(),
            ],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}

impl Service for SqliteSessionPersistence {
    fn service_name(&self) -> &'static str {
        "sessionPersistence"
    }
}

#[async_trait::async_trait]
impl dsh_session_persistence::SessionPersistenceApi for SqliteSessionPersistence {
    fn locate(&self, _meta: &SessionHeader) -> Option<dsh_session_persistence::SessionLocation> {
        None
    }

    fn supports_raw_artifacts(&self) -> bool {
        false
    }

    async fn create(&self, meta: SessionHeader) -> Result<(), String> {
        self.coordinator().create(meta).await
    }

    async fn append(&self, id: &SessionId, events: &[SessionEvent]) -> Result<(), String> {
        self.coordinator().append(id, events).await
    }

    async fn prepare(&self, id: &SessionId) -> Result<SessionPreparation, String> {
        self.coordinator().prepare(id).await
    }

    async fn load(
        &self,
        id: &SessionId,
    ) -> Result<dsh_session_persistence::SessionInspection, String> {
        self.coordinator().load(id).await
    }

    async fn inspect(
        &self,
        id: &SessionId,
    ) -> Result<dsh_session_persistence::SessionInspection, String> {
        self.coordinator().inspect(id).await
    }

    async fn read_from(
        &self,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<dsh_session_persistence::SessionReadFromResult, String> {
        self.coordinator().read_from(id, from_seq).await
    }

    async fn read_event_chunk(
        &self,
        id: &SessionId,
        from_seq: u64,
        max_events: usize,
    ) -> Result<dsh_session_persistence::SessionEventChunk, String> {
        if max_events == 0 {
            return Err("event chunk size must be positive".to_string());
        }
        self.ensure_ready().await?;
        self.with_db(|db| {
            if self.row_for(db, id)?.is_none() {
                return Err(format!("session \"{}\" not found", id.as_str()));
            }
            let mut statement = db
                .prepare(
                    "SELECT seq, type, time, data, source_event_seqs, surface_op, ignorable \
                     FROM events WHERE session_id = ?1 AND seq >= ?2 ORDER BY seq LIMIT ?3",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map(
                    params![id.as_str(), from_seq as i64, max_events as i64 + 1],
                    |row| {
                        Ok(EventRow {
                            seq: row.get(0)?,
                            type_: row.get(1)?,
                            time: row.get(2)?,
                            data: row.get(3)?,
                            source_event_seqs: row.get(4)?,
                            surface_op: row.get(5)?,
                            ignorable: row.get(6)?,
                        })
                    },
                )
                .map_err(|error| error.to_string())?;
            let mut events = Vec::with_capacity(max_events);
            let mut expected = from_seq;
            let mut has_more = false;
            for row in rows {
                let event = row_to_event(&row.map_err(|error| error.to_string())?)?;
                if event.seq != expected {
                    return Err(format!(
                        "corrupt session log: seq gap (expected {expected}, got {})",
                        event.seq
                    ));
                }
                if events.len() == max_events {
                    has_more = true;
                    break;
                }
                expected += 1;
                events.push(event);
            }
            Ok(dsh_session_persistence::SessionEventChunk {
                events,
                next_seq: has_more.then_some(expected),
            })
        })
    }

    async fn visit_event_chunks(
        &self,
        id: &SessionId,
        max_events: usize,
        visitor: Arc<dyn for<'a> Fn(&'a [SessionEvent]) -> Result<(), String> + Send + Sync>,
    ) -> Result<(), String> {
        if max_events == 0 {
            return Err("event chunk size must be positive".to_string());
        }
        self.ensure_ready().await?;
        self.with_db(|db| {
            db.execute_batch("BEGIN")
                .map_err(|error| error.to_string())?;
            let outcome = (|| -> Result<(), String> {
                if self.row_for(db, id)?.is_none() {
                    return Err(format!("session \"{}\" not found", id.as_str()));
                }
                let committed_end: Option<i64> = db
                    .query_row(
                        "SELECT MAX(seq) FROM events WHERE session_id = ?1 AND type = 'turn/end'",
                        [id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                let mut statement = db
                    .prepare(
                        "SELECT seq, type, time, data, source_event_seqs, surface_op, ignorable \
                         FROM events WHERE session_id = ?1 ORDER BY seq",
                    )
                    .map_err(|error| error.to_string())?;
                let rows = statement
                    .query_map([id.as_str()], |row| {
                        Ok(EventRow {
                            seq: row.get(0)?,
                            type_: row.get(1)?,
                            time: row.get(2)?,
                            data: row.get(3)?,
                            source_event_seqs: row.get(4)?,
                            surface_op: row.get(5)?,
                            ignorable: row.get(6)?,
                        })
                    })
                    .map_err(|error| error.to_string())?;
                let mut expected = 0_u64;
                let mut chunk = Vec::with_capacity(max_events);
                for row in rows {
                    let row = row.map_err(|error| error.to_string())?;
                    let parsed = row_to_event(&row);
                    let seq = u64::try_from(row.seq).ok();
                    let valid = parsed
                        .as_ref()
                        .ok()
                        .zip(seq)
                        .filter(|(event, seq)| event.seq == *seq && *seq == expected);
                    let Some((event, _)) = valid else {
                        if committed_end.is_some_and(|end| row.seq <= end) {
                            return Err(format!(
                                "corrupt session log at committed seq {}",
                                row.seq
                            ));
                        }
                        break;
                    };
                    chunk.push(event.clone());
                    expected += 1;
                    if chunk.len() == max_events {
                        visitor(&chunk)?;
                        chunk.clear();
                    }
                }
                if !chunk.is_empty() {
                    visitor(&chunk)?;
                }
                Ok(())
            })();
            match outcome {
                Ok(()) => db
                    .execute_batch("COMMIT")
                    .map_err(|error| error.to_string()),
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    async fn read_window(
        &self,
        id: &SessionId,
        request: SessionReadWindowRequest,
    ) -> Result<SessionReadWindowResult, String> {
        self.ensure_ready().await?;
        self.with_db(|db| {
            let Some(row) = self.row_for(db, id)? else {
                return Err(format!("session \"{}\" not found", id.as_str()));
            };
            let meta = row_to_meta(&row)?;
            let before_seq = match request.before_seq {
                Some(before) => before,
                None => {
                    let max_seq: Option<i64> = db
                        .query_row(
                            "SELECT MAX(seq) FROM events WHERE session_id = ?1",
                            [id.as_str()],
                            |row| row.get(0),
                        )
                        .map_err(|error| error.to_string())?;
                    max_seq
                        .map(|seq| u64::try_from(seq).map(|seq| seq.saturating_add(1)))
                        .transpose()
                        .map_err(|_| "stored session maximum seq is negative".to_string())?
                        .unwrap_or(0)
                }
            };
            let mut messages = request.max_messages.max(1);
            loop {
                let cut = self.history_window_cut(db, id, before_seq, messages)?;
                let event_count = self.history_event_count(db, id, cut, before_seq)?;
                if event_count <= request.max_events {
                    let rows = self.event_rows_range(db, id, cut, before_seq)?;
                    let ScanRowsResult { preserved, .. } = scan_rows(&rows, cut)?;
                    return Ok(SessionReadWindowResult {
                        meta,
                        events: preserved,
                        has_more: cut > 0,
                        oversized_event_count: None,
                    });
                }
                if messages == 1 {
                    if request.max_events > 0 {
                        let bounded_start = before_seq.saturating_sub(request.max_events as u64);
                        let rows = self.event_rows_range(db, id, bounded_start, before_seq)?;
                        let ScanRowsResult { preserved, .. } = scan_rows(&rows, bounded_start)?;
                        if preserved.last().is_some_and(|event| {
                            event.type_ == "assistant/message"
                                && event.surface_op.as_ref().is_some_and(|op| op.is_append())
                                && event
                                    .source_event_seqs
                                    .as_ref()
                                    .is_some_and(|seqs| !seqs.is_empty())
                        }) {
                            return Ok(SessionReadWindowResult {
                                meta,
                                events: preserved,
                                has_more: bounded_start > 0,
                                oversized_event_count: None,
                            });
                        }
                    }
                    return Ok(SessionReadWindowResult {
                        meta,
                        events: Vec::new(),
                        has_more: cut > 0,
                        oversized_event_count: Some(event_count),
                    });
                }
                let proportional = messages
                    .saturating_mul(request.max_events as u64)
                    .checked_div(event_count as u64)
                    .unwrap_or(1)
                    .max(1);
                messages = proportional.min(messages - 1);
            }
        })
    }

    async fn read_list_metadata(
        &self,
        id: &SessionId,
    ) -> Result<dsh_session_persistence::SessionListMetadata, String> {
        self.ensure_ready().await?;
        self.with_db(|db| {
            let Some(row) = self.row_for(db, id)? else {
                return Err(format!("session \"{}\" not found", id.as_str()));
            };
            let meta = row_to_meta(&row)?;
            let (last_seq, has_turn, last_user_time): (i64, bool, Option<i64>) = db
                .query_row(
                    "SELECT COALESCE(MAX(seq), -1),
                            EXISTS(SELECT 1 FROM events WHERE session_id = ?1 AND type = 'turn/start'),
                            (SELECT time FROM events WHERE session_id = ?1 AND type = 'user/message' ORDER BY seq DESC LIMIT 1)
                     FROM events WHERE session_id = ?1",
                    [id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|error| error.to_string())?;
            Ok(dsh_session_persistence::SessionListMetadata {
                updated_at: last_user_time.unwrap_or(meta.created_at as i64),
                meta,
                last_seq,
                blank: !has_turn,
            })
        })
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        self.ensure_ready().await?;
        self.with_db(|db| self.all_session_rows(db)?.iter().map(row_to_meta).collect())
    }

    async fn list_snapshots(
        &self,
    ) -> Result<Vec<dsh_session_persistence::SessionPersistenceSnapshot>, String> {
        self.ensure_ready().await?;
        let identity = self.store_identity();
        self.with_db(|db| {
            self.all_session_rows(db)?
                .iter()
                .map(|row| {
                    Ok(dsh_session_persistence::SessionPersistenceSnapshot {
                        header: row_to_meta(row)?,
                        revision: sqlite_revision(&identity, row),
                    })
                })
                .collect()
        })
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }
}

#[async_trait::async_trait]
impl PersistenceBackend<u64> for SqliteSessionPersistence {
    fn name(&self) -> &'static str {
        "session-persistence-sqlite"
    }

    fn seek_capable(&self) -> bool {
        true
    }

    async fn load_stored(&self, id: &SessionId) -> Result<Option<StoredPrefix<u64>>, String> {
        self.read_prefix(id).await
    }

    async fn read_stored_revision(
        &self,
        id: &SessionId,
    ) -> Result<Option<dsh_session_persistence::SessionPersistenceRevision>, String> {
        self.ensure_ready().await?;
        let identity = self.store_identity();
        self.with_db(move |db| {
            let Some(row) = self.row_for(db, id)? else {
                return Ok(None);
            };
            Ok(Some(sqlite_revision(&identity, &row)))
        })
    }

    async fn load_stored_from(
        &self,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<Option<StoredSuffix>, String> {
        self.ensure_ready().await?;
        self.with_db(|db| {
            let Some(row) = self.row_for(db, id)? else {
                return Ok(None);
            };
            let meta = row_to_meta(&row)?;
            let event_rows = self.event_rows_from(db, id, from_seq)?;
            let ScanRowsResult { preserved, .. } = scan_rows(&event_rows, from_seq)?;
            Ok(Some(StoredSuffix {
                meta,
                events: preserved,
            }))
        })
    }

    async fn append_batch(
        &self,
        meta: &SessionHeader,
        events: &[SessionEvent],
        is_materialized: bool,
    ) -> Result<(), String> {
        self.ensure_ready().await?;
        self.with_db(|db| {
            db.execute_batch("BEGIN")
                .map_err(|error| error.to_string())?;
            let outcome = (|| -> Result<(), String> {
                if !is_materialized {
                    self.write_row(db, meta)?;
                }
                for event in events {
                    self.insert_event(db, &meta.id, event)?;
                }
                db.execute(
                    "UPDATE sessions SET revision = revision + 1 WHERE id = ?1",
                    [meta.id.as_str()],
                )
                .map_err(|error| error.to_string())?;
                db.execute_batch("COMMIT")
                    .map_err(|error| error.to_string())?;
                Ok(())
            })();
            if outcome.is_err() {
                let _ = db.execute_batch("ROLLBACK");
            }
            outcome
        })
    }

    async fn commit_repair(
        &self,
        meta: &SessionHeader,
        torn_marker: Option<u64>,
        closers: &[SessionEvent],
    ) -> Result<(), String> {
        self.ensure_ready().await?;
        self.with_db(|db| {
            db.execute_batch("BEGIN")
                .map_err(|error| error.to_string())?;
            let outcome = (|| -> Result<(), String> {
                if let Some(torn) = torn_marker {
                    db.execute(
                        "DELETE FROM events WHERE session_id = ?1 AND seq >= ?2",
                        params![meta.id.as_str(), torn as i64],
                    )
                    .map_err(|error| error.to_string())?;
                }
                for event in closers {
                    self.insert_event(db, &meta.id, event)?;
                }
                if torn_marker.is_some() || !closers.is_empty() {
                    db.execute(
                        "UPDATE sessions SET revision = revision + 1 WHERE id = ?1",
                        [meta.id.as_str()],
                    )
                    .map_err(|error| error.to_string())?;
                }
                db.execute_batch("COMMIT")
                    .map_err(|error| error.to_string())?;
                Ok(())
            })();
            if outcome.is_err() {
                let _ = db.execute_batch("ROLLBACK");
            }
            outcome
        })
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        self.ensure_ready().await?;
        self.with_db(|db| self.all_session_rows(db)?.iter().map(row_to_meta).collect())
    }

    async fn close(&self) -> Result<(), String> {
        self.ensure_ready().await?;
        *self.db.lock() = None;
        Ok(())
    }
}

#[cfg(test)]
mod history_window_tests {
    use super::*;
    use dsh_session::{SESSION_FORMAT_VERSION, SurfaceOp, session_id};
    use dsh_session_persistence::SessionPersistenceApi;

    fn event(seq: u64, type_: &str, append_surface: bool) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time: seq as i64,
            data: serde_json::json!({}),
            ignorable: None,
            surface_op: append_surface.then_some(SurfaceOp::Append),
            source_event_seqs: None,
        }
    }

    #[tokio::test]
    async fn sqlite_reports_oversized_sparse_window_without_loading_event_bodies() {
        let ctx = Context::root();
        let backend = SqliteSessionPersistence::install(
            &ctx,
            SqliteConfig {
                path: ":memory:".to_string(),
                journal_mode: JournalMode::Delete,
                ..SqliteConfig::default()
            },
        )
        .expect("install sqlite persistence");
        let id = session_id("sqlite-bounded-window");
        SessionPersistenceApi::create(
            backend.as_ref(),
            SessionHeader {
                version: SESSION_FORMAT_VERSION,
                id: id.clone(),
                created_at: 1,
                cwd: Some("C:/workspace".to_string()),
                parent_session: None,
                seed_length: None,
                origin: None,
                delegation_depth: None,
                agent_preset: Some("standard".to_string()),
            },
        )
        .await
        .expect("create session");
        let mut events = Vec::with_capacity(2_002);
        events.push(event(0, "user/message", true));
        for seq in 1..=2_000 {
            events.push(event(seq, "assistant/chunk", false));
        }
        events.push(event(2_001, "assistant/message", true));
        SessionPersistenceApi::append(backend.as_ref(), &id, &events)
            .await
            .expect("append events");

        let window = SessionPersistenceApi::read_window(
            backend.as_ref(),
            &id,
            SessionReadWindowRequest {
                before_seq: None,
                max_messages: 2,
                max_events: 512,
            },
        )
        .await
        .expect("read bounded window");

        assert!(window.events.len() <= 512);
        assert_eq!(window.events.last().map(|event| event.seq), Some(2_001));
        assert_eq!(window.oversized_event_count, None);
        assert!(window.has_more);
        let metadata = SessionPersistenceApi::read_list_metadata(backend.as_ref(), &id)
            .await
            .expect("read list metadata");
        assert!(metadata.blank);
        assert_eq!(metadata.updated_at, 0);
        assert_eq!(metadata.last_seq, 2_001);

        let visited = Arc::new(Mutex::new(Vec::<Vec<u64>>::new()));
        let visitor = {
            let visited = visited.clone();
            Arc::new(move |chunk: &[SessionEvent]| {
                visited
                    .lock()
                    .push(chunk.iter().map(|event| event.seq).collect());
                Ok(())
            })
        };
        SessionPersistenceApi::visit_event_chunks(backend.as_ref(), &id, 1_000, visitor)
            .await
            .expect("visit sqlite event chunks");
        let visited = visited.lock();
        assert_eq!(
            visited.iter().map(Vec::len).collect::<Vec<_>>(),
            [1_000, 1_000, 2]
        );
        assert_eq!(
            visited.iter().flatten().copied().collect::<Vec<_>>(),
            (0..=2_001).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn sqlite_bounds_one_oversized_streaming_message_to_its_renderable_tail() {
        let ctx = Context::root();
        let backend = SqliteSessionPersistence::install(
            &ctx,
            SqliteConfig {
                path: ":memory:".to_string(),
                journal_mode: JournalMode::Delete,
                ..SqliteConfig::default()
            },
        )
        .expect("install sqlite persistence");
        let id = session_id("sqlite-oversized-streaming-message");
        SessionPersistenceApi::create(
            backend.as_ref(),
            SessionHeader {
                version: SESSION_FORMAT_VERSION,
                id: id.clone(),
                created_at: 1,
                cwd: Some("C:/workspace".to_string()),
                parent_session: None,
                seed_length: None,
                origin: None,
                delegation_depth: None,
                agent_preset: Some("standard".to_string()),
            },
        )
        .await
        .expect("create session");
        let mut events = Vec::with_capacity(4_097);
        for seq in 0..4_096 {
            events.push(event(seq, "assistant/chunk", false));
        }
        let mut message = event(4_096, "assistant/message", true);
        message.source_event_seqs = Some((0..4_096).collect());
        events.push(message);
        SessionPersistenceApi::append(backend.as_ref(), &id, &events)
            .await
            .expect("append events");

        let window = SessionPersistenceApi::read_window(
            backend.as_ref(),
            &id,
            SessionReadWindowRequest {
                before_seq: None,
                max_messages: 1,
                max_events: 4_096,
            },
        )
        .await
        .expect("read bounded window");

        assert_eq!(window.events.len(), 4_096);
        assert_eq!(window.events.first().map(|event| event.seq), Some(1));
        assert_eq!(window.events.last().map(|event| event.seq), Some(4_096));
        assert_eq!(window.oversized_event_count, None);
        assert!(window.has_more);
    }
}
