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
    PersistenceCoordinatorOptions, StoredPrefix, StoredSuffix,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use crate::schema::{
    EventRow, JournalMode, ScanRowsResult, SessionRow, open_database, row_to_meta, scan_rows,
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
    if write_batch_max_delay_ms < 1 || write_batch_max_delay_ms > MAX_WRITE_BATCH_DELAY_MS {
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
        let result = self.with_db(|db| {
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
        });
        result
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
                event.time as i64,
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
mod tests {
    use super::*;
    use dsh_session::{SessionStore, SurfaceIntent, session_id};
    use dsh_session_persistence::SessionPersistenceApi;

    fn fresh_db_path(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir()
            .join(format!(
                "dsh-sqlite-{tag}-{}",
                uuid::Uuid::new_v4().simple()
            ))
            .join("sessions.db");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        path
    }

    fn header(id: &str, cwd: Option<&str>) -> SessionHeader {
        SessionHeader {
            version: dsh_session::SESSION_FORMAT_VERSION,
            id: session_id(id),
            created_at: 1000,
            cwd: cwd.map(str::to_string),
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }
    }

    fn one_turn_log() -> Vec<SessionEvent> {
        vec![
            SessionEvent {
                type_: "turn/start".to_string(),
                seq: 0,
                time: 1,
                data: serde_json::json!({"turn": 1}),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
            SessionEvent {
                type_: "user/message".to_string(),
                seq: 1,
                time: 2,
                data: serde_json::json!({
                    "id": "one-turn-user", "role": "user",
                    "content": [{"type": "text", "text": "hi"}], "source": {"kind": "user"},
                }),
                ignorable: None,
                surface_op: Some(dsh_session::SurfaceOp::Append),
                source_event_seqs: None,
            },
            SessionEvent {
                type_: "step/start".to_string(),
                seq: 2,
                time: 3,
                data: serde_json::json!({"turn": 1, "step": 1}),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
            SessionEvent {
                type_: "assistant/message".to_string(),
                seq: 3,
                time: 4,
                data: serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "id": "one-turn-assistant", "role": "assistant",
                        "content": [{"type": "text", "text": "hello"}],
                        "source": {"kind": "model", "provider": "mock", "model": "mock"},
                    },
                }),
                ignorable: None,
                surface_op: Some(dsh_session::SurfaceOp::Append),
                source_event_seqs: None,
            },
            SessionEvent {
                type_: "step/end".to_string(),
                seq: 4,
                time: 5,
                data: serde_json::json!({"turn": 1, "step": 1}),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
            SessionEvent {
                type_: "turn/end".to_string(),
                seq: 5,
                time: 6,
                data: serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
        ]
    }

    async fn backend(path: &str) -> (Context, Arc<SqliteSessionPersistence>) {
        let ctx = Context::root();
        let _store = SessionStore::install(&ctx);
        let backend = SqliteSessionPersistence::install(
            &ctx,
            SqliteConfig {
                path: path.to_string(),
                ..Default::default()
            },
        )
        .unwrap();
        (ctx, backend)
    }

    fn probe_rows(path: &std::path::Path, id: &str) -> Vec<(i64, String)> {
        let db = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap();
        let rows = {
            let mut statement = db
                .prepare("SELECT seq, type FROM events WHERE session_id = ?1 ORDER BY seq")
                .unwrap();
            statement
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        drop(db);
        rows
    }

    fn probe_pragma(path: &std::path::Path, name: &str) -> String {
        let db = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap();
        let value = db
            .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .unwrap();
        drop(db);
        value
    }

    #[test]
    fn config_parse_defaults_and_validation() {
        let parsed = parse_config(&serde_json::json!({"path": "x"})).unwrap();
        assert_eq!(parsed.journal_mode, JournalMode::Wal);
        assert_eq!(parsed.prepared_session_cache_size, 5);
        assert_eq!(parsed.write_batch_max_delay_ms, 200);

        let parsed = parse_config(&serde_json::json!({
            "path": "x", "journalMode": "persist",
            "preparedSessionCacheSize": 1, "writeBatchMaxDelayMs": 1,
        }))
        .unwrap();
        assert_eq!(parsed.journal_mode, JournalMode::Persist);
        assert_eq!(parsed.prepared_session_cache_size, 1);
        assert_eq!(parsed.write_batch_max_delay_ms, 1);

        assert!(parse_config(&serde_json::json!({})).is_err());
        assert!(parse_config(&serde_json::json!({"path": "x", "journalMode": "off"})).is_err());
        assert!(
            parse_config(&serde_json::json!({"path": "x", "preparedSessionCacheSize": 0})).is_err()
        );
        assert!(
            parse_config(&serde_json::json!({"path": "x", "writeBatchMaxDelayMs": 0})).is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn memory_lifecycle_round_trips() {
        let (ctx, backend) = backend(":memory:").await;
        assert!(!backend.supports_raw_artifacts());
        assert!(SessionPersistenceApi::locate(backend.as_ref(), &header("s1", None)).is_none());

        // Lazy materialization: no row until the first append.
        backend.create(header("s1", None)).await.unwrap();
        assert!(
            SessionPersistenceApi::list(backend.as_ref())
                .await
                .unwrap()
                .is_empty()
        );

        backend
            .append(&session_id("s1"), &one_turn_log())
            .await
            .unwrap();
        let inspection = backend.load(&session_id("s1")).await.unwrap();
        assert_eq!(
            inspection.events,
            one_turn_log(),
            "balanced log loads verbatim"
        );
        assert_eq!(
            SessionPersistenceApi::list(backend.as_ref())
                .await
                .unwrap()
                .len(),
            1
        );

        // seek-capable readFrom returns only the suffix.
        let suffix = backend.read_from(&session_id("s1"), 4).await.unwrap();
        assert_eq!(suffix.events.len(), 2);
        assert_eq!(suffix.events[0].seq, 4);

        let snapshots = backend.list_snapshots().await.unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].header.id.as_str(), "s1");
        let _ = ctx;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn interrupted_turn_is_preserved_and_closed_durably() {
        let path = fresh_db_path("crash");
        let m = header("crash", None);
        {
            let (_ctx, backend) = backend(path.to_str().unwrap()).await;
            backend.create(m.clone()).await.unwrap();
            backend.append(&m.id, &one_turn_log()).await.unwrap();
            backend
                .append(
                    &m.id,
                    &[
                        SessionEvent {
                            type_: "turn/start".to_string(),
                            seq: 6,
                            time: 7,
                            data: serde_json::json!({"turn": 2}),
                            ignorable: None,
                            surface_op: None,
                            source_event_seqs: None,
                        },
                        SessionEvent {
                            type_: "step/start".to_string(),
                            seq: 7,
                            time: 8,
                            data: serde_json::json!({"turn": 2, "step": 1}),
                            ignorable: None,
                            surface_op: None,
                            source_event_seqs: None,
                        },
                    ],
                )
                .await
                .unwrap();
            let _ = backend.close().await;
        }

        let (_ctx, backend) = backend(path.to_str().unwrap()).await;
        let loaded = backend.load(&m.id).await.unwrap();
        assert_eq!(
            loaded
                .events
                .iter()
                .map(|event| event.type_.as_str())
                .collect::<Vec<_>>(),
            vec![
                "turn/start",
                "user/message",
                "step/start",
                "assistant/message",
                "step/end",
                "turn/end",
                "turn/start",
                "step/start",
                "step/end",
                "turn/end",
            ]
        );
        assert_eq!(
            loaded
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        let last = loaded.events.last().unwrap();
        assert_eq!(last.type_, "turn/end");
        assert_eq!(
            last.data.get("reason"),
            Some(&serde_json::json!({"kind": "interrupted"}))
        );

        // load() is mutating: the synthetic closers MUST be on disk.
        let stored = probe_rows(&path, "crash");
        assert_eq!(
            stored.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        assert_eq!(stored.last().unwrap().1, "turn/end");

        // The balanced cursor continues and reloads identically.
        backend
            .append(
                &m.id,
                &[
                    SessionEvent {
                        type_: "turn/start".to_string(),
                        seq: 10,
                        time: 9,
                        data: serde_json::json!({"turn": 3}),
                        ignorable: None,
                        surface_op: None,
                        source_event_seqs: None,
                    },
                    SessionEvent {
                        type_: "turn/end".to_string(),
                        seq: 11,
                        time: 10,
                        data: serde_json::json!({"turn": 3, "reason": {"kind": "completed"}}),
                        ignorable: None,
                        surface_op: None,
                        source_event_seqs: None,
                    },
                ],
            )
            .await
            .unwrap();
        let reloaded = backend.load(&m.id).await.unwrap();
        assert_eq!(reloaded.events.len(), 12);
        let _ = backend.close().await;
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn corrupt_torn_tail_is_discarded_on_load() {
        let path = fresh_db_path("corrupt-tail");
        let m = header("corrupt-tail", None);
        {
            let (_ctx, backend) = backend(path.to_str().unwrap()).await;
            backend.create(m.clone()).await.unwrap();
            backend.append(&m.id, &one_turn_log()).await.unwrap();
            let _ = backend.close().await;
        }
        // A torn row after the committed turn with invalid JSON.
        {
            let db = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap();
            db.execute(
                "INSERT INTO events (session_id, seq, type, time, data) VALUES (?1, 6, 'turn/start', 7, '{not valid json')",
                [m.id.as_str()],
            )
            .unwrap();
        }

        let (_ctx, backend) = backend(path.to_str().unwrap()).await;
        let loaded = backend.load(&m.id).await.unwrap();
        assert_eq!(
            loaded.events,
            one_turn_log(),
            "torn tail discarded, committed intact"
        );

        // The torn row was physically deleted: a fresh append continues at 6.
        backend
            .append(
                &m.id,
                &[
                    SessionEvent {
                        type_: "turn/start".to_string(),
                        seq: 6,
                        time: 8,
                        data: serde_json::json!({"turn": 2}),
                        ignorable: None,
                        surface_op: None,
                        source_event_seqs: None,
                    },
                    SessionEvent {
                        type_: "turn/end".to_string(),
                        seq: 7,
                        time: 9,
                        data: serde_json::json!({"turn": 2, "reason": {"kind": "completed"}}),
                        ignorable: None,
                        surface_op: None,
                        source_event_seqs: None,
                    },
                ],
            )
            .await
            .unwrap();
        let reloaded = backend.load(&m.id).await.unwrap();
        assert_eq!(
            reloaded
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5, 6, 7]
        );
        let _ = backend.close().await;
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn append_rolls_back_whole_batch_on_mid_batch_seq_collision() {
        let (ctx, backend) = backend(":memory:").await;
        let m = header("rollback", None);
        backend.create(m.clone()).await.unwrap();
        backend.append(&m.id, &one_turn_log()).await.unwrap();

        let error = backend.append(&m.id, &one_turn_log()).await.unwrap_err();
        assert!(error.contains("seq"), "{error}");
        let loaded = backend.load(&m.id).await.unwrap();
        assert_eq!(loaded.events, one_turn_log(), "stored log unchanged");
        let _ = ctx;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_instances_unique_constraint_rolls_back() {
        let path = fresh_db_path("interleave");
        let m = header("interleave", None);
        let (ctx1, b1) = backend(path.to_str().unwrap()).await;
        let (ctx2, b2) = backend(path.to_str().unwrap()).await;
        b1.create(m.clone()).await.unwrap();
        b1.append(&m.id, &one_turn_log()).await.unwrap();
        // b2 adopts the committed cursor 6 into its own in-memory state.
        b2.load(&m.id).await.unwrap();

        let turn2 = vec![
            SessionEvent {
                type_: "turn/start".to_string(),
                seq: 6,
                time: 7,
                data: serde_json::json!({"turn": 2}),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
            SessionEvent {
                type_: "turn/end".to_string(),
                seq: 7,
                time: 8,
                data: serde_json::json!({"turn": 2, "reason": {"kind": "completed"}}),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
        ];
        b1.append(&m.id, &turn2).await.unwrap();
        // b2's contiguity check passes but its INSERT hits the UNIQUE
        // constraint mid-transaction → ROLLBACK + rethrow.
        let error = b2.append(&m.id, &turn2).await.unwrap_err();
        assert!(error.contains("UNIQUE"), "{error}");

        let loaded = b1.load(&m.id).await.unwrap();
        assert_eq!(
            loaded
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5, 6, 7]
        );
        let _ = b1.close().await;
        let _ = b2.close().await;
        let _ = (ctx1, ctx2);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn persists_across_backend_instances_over_the_same_file() {
        let path = fresh_db_path("persist");
        let m = header("persist", Some("C:\\proj"));
        {
            let (_ctx, backend) = backend(path.to_str().unwrap()).await;
            backend.create(m.clone()).await.unwrap();
            backend.append(&m.id, &one_turn_log()).await.unwrap();
            let _ = backend.close().await;
        }
        let (_ctx, backend) = backend(path.to_str().unwrap()).await;
        assert_eq!(
            SessionPersistenceApi::list(backend.as_ref())
                .await
                .unwrap()
                .iter()
                .map(|meta| meta.id.as_str())
                .collect::<Vec<_>>(),
            vec!["persist"]
        );
        let loaded = backend.load(&m.id).await.unwrap();
        assert_eq!(loaded.meta.cwd.as_deref(), Some("C:\\proj"));
        assert_eq!(loaded.events, one_turn_log());
        let _ = backend.close().await;
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn revisions_are_source_qualified_and_reincarnate() {
        let path_a = fresh_db_path("rev-a");
        let path_b = fresh_db_path("rev-b");
        let m = header("revision-source", None);
        let revision_a;
        {
            let (_ctx, backend) = backend(path_a.to_str().unwrap()).await;
            backend.create(m.clone()).await.unwrap();
            backend.append(&m.id, &one_turn_log()).await.unwrap();
            revision_a = backend.list_snapshots().await.unwrap()[0].revision.clone();
            let _ = backend.close().await;
        }
        // Reopening the SAME file keeps the revision.
        {
            let (_ctx, backend) = backend(path_a.to_str().unwrap()).await;
            assert_eq!(
                backend.list_snapshots().await.unwrap()[0].revision,
                revision_a
            );
            let _ = backend.close().await;
        }
        // A DIFFERENT store yields a different revision (distinct store ids).
        {
            let (_ctx, backend) = backend(path_b.to_str().unwrap()).await;
            backend.create(m.clone()).await.unwrap();
            backend.append(&m.id, &one_turn_log()).await.unwrap();
            let revision_b = backend.list_snapshots().await.unwrap()[0].revision.clone();
            assert_ne!(revision_b, revision_a);
            assert!(revision_b.as_str().ends_with(":revision:1"));
            let _ = backend.close().await;
        }
        assert!(revision_a.as_str().ends_with(":revision:1"));

        // Deleting the session and re-materializing mints a new incarnation.
        {
            let db = open_database(path_a.to_str().unwrap(), JournalMode::Wal).unwrap();
            db.execute("DELETE FROM sessions WHERE id = ?1", [m.id.as_str()])
                .unwrap();
        }
        {
            let (_ctx, backend) = backend(path_a.to_str().unwrap()).await;
            backend.create(m.clone()).await.unwrap();
            backend.append(&m.id, &one_turn_log()).await.unwrap();
            let revision = backend.list_snapshots().await.unwrap()[0].revision.clone();
            assert_ne!(revision, revision_a);
            assert!(revision.as_str().ends_with(":revision:1"));
            let _ = backend.close().await;
        }
        let _ = std::fs::remove_dir_all(path_a.parent().unwrap());
        let _ = std::fs::remove_dir_all(path_b.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_repair_keeps_revision_stable() {
        let (_ctx, backend) = backend(":memory:").await;
        let m = header("empty-repair", None);
        backend.create(m.clone()).await.unwrap();
        backend.append(&m.id, &one_turn_log()).await.unwrap();
        let before = backend.list_snapshots().await.unwrap();
        backend.commit_repair(&m, None, &[]).await.unwrap();
        let after = backend.list_snapshots().await.unwrap();
        assert_eq!(after, before);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rejects_legacy_event_vocabulary_on_load() {
        let path = fresh_db_path("legacy");
        let m = header("legacy-header-delta", None);
        {
            let db = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap();
            db.execute(
                "INSERT INTO sessions (id, version, created_at, cwd, parent_session, seed_length, delegation_depth, incarnation, revision) VALUES (?1, 0, 1000, NULL, NULL, NULL, NULL, 'legacy', 1)",
                [m.id.as_str()],
            )
            .unwrap();
            db.execute(
                "INSERT INTO events (session_id, seq, type, time, data) VALUES (?1, 0, 'turn/start', 1, ?2)",
                params![m.id.as_str(), "{\"turn\": 1}"],
            )
            .unwrap();
            db.execute(
                "INSERT INTO events (session_id, seq, type, time, data) VALUES (?1, 1, 'request/header-delta', 2, ?2)",
                params![m.id.as_str(), "{\"config\": {\"model\": \"legacy\"}}"],
            )
            .unwrap();
            db.execute(
                "INSERT INTO events (session_id, seq, type, time, data) VALUES (?1, 2, 'turn/end', 3, ?2)",
                params![m.id.as_str(), "{\"turn\": 1, \"reason\": {\"kind\": \"completed\"}}"],
            )
            .unwrap();
        }
        let (_ctx, backend) = backend(path.to_str().unwrap()).await;
        let error = backend.load(&m.id).await.unwrap_err();
        assert!(
            error.contains("unsupported legacy request/header-delta event at seq 1"),
            "{error}"
        );
        let _ = backend.close().await;
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invalid_store_identity_rejects_reads() {
        let path = fresh_db_path("identity");
        {
            let db = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap();
            db.execute_batch("UPDATE persistence_state SET store_id = '' WHERE singleton = 1")
                .unwrap();
        }
        let (_ctx, backend) = backend(path.to_str().unwrap()).await;
        let error = backend.list_snapshots().await.unwrap_err();
        assert!(error.contains("no valid store identity"), "{error}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn journal_mode_config_reaches_the_database() {
        // Default: wal.
        let wal_path = fresh_db_path("jm-wal");
        {
            let (_ctx, backend) = backend(wal_path.to_str().unwrap()).await;
            backend.create(header("jm-wal", None)).await.unwrap();
            let _ = backend.close().await;
        }
        assert_eq!(probe_pragma(&wal_path, "journal_mode"), "wal");

        // delete mode: no -wal sidecar after writes.
        let delete_path = fresh_db_path("jm-delete");
        {
            let ctx = Context::root();
            let _store = SessionStore::install(&ctx);
            let backend = SqliteSessionPersistence::install(
                &ctx,
                SqliteConfig {
                    path: delete_path.to_str().unwrap().to_string(),
                    journal_mode: JournalMode::Delete,
                    ..Default::default()
                },
            )
            .unwrap();
            let m = header("jm-delete", None);
            backend.create(m.clone()).await.unwrap();
            backend.append(&m.id, &one_turn_log()).await.unwrap();
            let _ = backend.close().await;
        }
        let mut wal_sidecar = delete_path.clone();
        wal_sidecar.set_file_name(format!(
            "{}-wal",
            wal_sidecar.file_name().unwrap().to_string_lossy()
        ));
        assert!(!wal_sidecar.exists());
        let _ = std::fs::remove_dir_all(wal_path.parent().unwrap());
        let _ = std::fs::remove_dir_all(delete_path.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn live_session_surface_fields_round_trip_through_sqlite() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let backend = SqliteSessionPersistence::install(
            &ctx,
            SqliteConfig {
                path: ":memory:".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
        let session = store
            .create(
                &ctx,
                Some(session_id("roundtrip-surface")),
                Some(dsh_session::CreateSessionOptions::default()),
            )
            .await
            .unwrap();
        session
            .append("turn/start", serde_json::json!({"turn": 1}), None)
            .unwrap();
        session
            .append(
                "user/message",
                serde_json::json!({
                    "id": "m1", "role": "user",
                    "content": [{"type": "text", "text": "hi"}], "source": {"kind": "user"},
                }),
                Some(SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        session
            .append(
                "assistant/message",
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "id": "m2", "role": "assistant", "content": [],
                        "source": {"kind": "model", "provider": "mock", "model": "mock"},
                    },
                }),
                Some(SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: Some(vec![1]),
                }),
            )
            .unwrap();
        session
            .append(
                "turn/end",
                serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
                None,
            )
            .unwrap();
        assert!(store.flush(&session).await.unwrap());

        let loaded = backend
            .load(&session_id("roundtrip-surface"))
            .await
            .unwrap();
        assert_eq!(loaded.events.len(), 4);
        assert_eq!(
            loaded.events[1].surface_op,
            Some(dsh_session::SurfaceOp::Append)
        );
        assert_eq!(loaded.events[1].source_event_seqs, None);
        assert_eq!(
            loaded.events[2].surface_op,
            Some(dsh_session::SurfaceOp::Append)
        );
        assert_eq!(loaded.events[2].source_event_seqs, Some(vec![1]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hmr_collision_with_materialized_id_rejects() {
        let path = fresh_db_path("hmr");
        {
            let ctx = Context::root();
            let store = SessionStore::install(&ctx);
            let backend = SqliteSessionPersistence::install(
                &ctx,
                SqliteConfig {
                    path: path.to_str().unwrap().to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
            let session = store
                .create(
                    &ctx,
                    Some(session_id("hmr-collide")),
                    Some(dsh_session::CreateSessionOptions::default()),
                )
                .await
                .unwrap();
            for event in one_turn_log() {
                session
                    .append(
                        &event.type_,
                        event.data,
                        event.surface_op.as_ref().map(|op| SurfaceIntent {
                            surface_op: op.clone(),
                            source_event_seqs: event.source_event_seqs.clone(),
                        }),
                    )
                    .unwrap();
            }
            assert!(store.flush(&session).await.unwrap());
            let _ = backend.close().await;
        }

        // A fresh context with an UNRELATED live session reusing the id meets
        // a materialized row that is not a prefix of its events → reject.
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let session = store
            .create(
                &ctx,
                Some(session_id("hmr-collide")),
                Some(dsh_session::CreateSessionOptions::default()),
            )
            .await
            .unwrap();
        session
            .append("turn/start", serde_json::json!({"turn": 1}), None)
            .unwrap();
        let _backend = SqliteSessionPersistence::install(
            &ctx,
            SqliteConfig {
                path: path.to_str().unwrap().to_string(),
                ..Default::default()
            },
        )
        .unwrap();
        let error = store.flush(&session).await.unwrap_err();
        assert!(error.contains("id collision"), "{error}");
        let _ = _backend.close().await;
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
