//! Schema + load-time helpers for the SQLite session-persistence backend.
//! Rust port of `packages/session/session-persistence-sqlite/src/schema.ts`.
//!
//! # Deviations
//!
//! - The TS `rowToMeta` fractional-`created_at` rejection is inexpressible:
//!   SQLite `INTEGER` values deserialize into `i64` here, so the value is
//!   always a safe integer. The non-negative check is retained.
//! - The last-`turn/end` cut (`scanRows`) is identical, including the
//!   `base + row index` expected-seq formula and the raw-row `type` column
//!   used to locate the last committed boundary.

use dsh_session::{SessionEvent, SessionHeader, SurfaceOp, session_id};
use rusqlite::Connection;

/// The on-disk schema version (TS `SCHEMA_VERSION`).
pub const SCHEMA_VERSION: i64 = 15;

/// SQLite application id protecting unrelated databases from persistence
/// writes (TS `SESSION_PERSISTENCE_SQLITE_APPLICATION_ID`).
pub const SESSION_PERSISTENCE_SQLITE_APPLICATION_ID: i64 = 0x44534850;

/// Journal modes the backend accepts (TS `JournalMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalMode {
    Wal,
    Delete,
    Truncate,
    Persist,
}

impl JournalMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            JournalMode::Wal => "wal",
            JournalMode::Delete => "delete",
            JournalMode::Truncate => "truncate",
            JournalMode::Persist => "persist",
        }
    }
}

fn sqlite_error(error: rusqlite::Error) -> String {
    error.to_string()
}

/// A row of the `sessions` table (TS `SessionRow`).
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub version: i64,
    pub created_at: i64,
    pub cwd: Option<String>,
    pub parent_session: Option<String>,
    pub seed_length: Option<i64>,
    pub origin: Option<String>,
    pub incarnation: String,
    pub revision: i64,
    pub delegation_depth: Option<i64>,
    pub agent_preset: Option<String>,
}

/// An `events` table row (TS `EventRow`).
#[derive(Debug, Clone)]
pub struct EventRow {
    pub seq: i64,
    pub type_: String,
    pub time: i64,
    pub data: String,
    pub source_event_seqs: Option<String>,
    pub surface_op: Option<String>,
    pub ignorable: Option<i64>,
}

/// The DDL applied to every owned database (TS `configureDatabase` body).
const DDL: &str = "
      CREATE TABLE IF NOT EXISTS persistence_state (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        store_id  TEXT NOT NULL
      ) STRICT;

      CREATE TABLE IF NOT EXISTS sessions (
        id               TEXT PRIMARY KEY,
        version          INTEGER NOT NULL,
        created_at       INTEGER NOT NULL,
        cwd              TEXT,
        parent_session   TEXT,
        seed_length      INTEGER,
        origin           TEXT,
        delegation_depth INTEGER,
        agent_preset    TEXT,
        incarnation      TEXT NOT NULL,
        revision         INTEGER NOT NULL
      ) STRICT;

      CREATE TABLE IF NOT EXISTS events (
        session_id        TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        seq               INTEGER NOT NULL,
        type              TEXT NOT NULL,
        time              INTEGER NOT NULL,
        data              TEXT NOT NULL,
        source_event_seqs TEXT,
        surface_op        TEXT,
        ignorable         INTEGER,
        PRIMARY KEY (session_id, seq)
      ) STRICT
";

/// Open the database, apply its schema and pragmas, and stamp ownership.
/// An empty unversioned database is initialized at [`SCHEMA_VERSION`]; a
/// nonempty unversioned database and every other non-current version reject
/// (TS `openDatabase`).
pub fn open_database(path: &str, journal_mode: JournalMode) -> Result<Connection, String> {
    let db = Connection::open(path).map_err(sqlite_error)?;
    match configure_database(&db, path, journal_mode) {
        Ok(()) => Ok(db),
        Err(error) => {
            drop(db);
            Err(error)
        }
    }
}

fn configure_database(
    db: &Connection,
    path: &str,
    journal_mode: JournalMode,
) -> Result<(), String> {
    db.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_error)?;
    let mut began = false;
    let result = (|| -> Result<(), String> {
        db.execute_batch("BEGIN IMMEDIATE").map_err(sqlite_error)?;
        began = true;
        // Validate while holding the write lock so no other connection can
        // change schema ownership between inspection and initialization.
        let on_disk: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        let application_id: i64 = db
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        let user_object_count: i64 = db
            .query_row(
                "SELECT COUNT(*) AS count FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_error)?;
        if on_disk == 0 && (application_id != 0 || user_object_count > 0) {
            return Err(format!(
                "session database at \"{path}\" has an unversioned schema or application identity"
            ));
        }
        if on_disk != 0 && on_disk != SCHEMA_VERSION {
            return Err(format!(
                "session database at \"{path}\" has schema version {on_disk}, incompatible with this build ({SCHEMA_VERSION})"
            ));
        }
        if on_disk == SCHEMA_VERSION && application_id != SESSION_PERSISTENCE_SQLITE_APPLICATION_ID
        {
            return Err(format!(
                "session database at \"{path}\" has application id {application_id}, expected {SESSION_PERSISTENCE_SQLITE_APPLICATION_ID}"
            ));
        }
        db.execute_batch(DDL).map_err(sqlite_error)?;
        db.execute(
            "INSERT OR IGNORE INTO persistence_state (singleton, store_id) VALUES (1, ?1)",
            [uuid::Uuid::new_v4().simple().to_string()],
        )
        .map_err(sqlite_error)?;
        if on_disk == 0 {
            db.pragma_update(
                None,
                "application_id",
                SESSION_PERSISTENCE_SQLITE_APPLICATION_ID,
            )
            .map_err(sqlite_error)?;
            db.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(sqlite_error)?;
        }
        db.execute_batch("COMMIT").map_err(sqlite_error)?;
        began = false;
        Ok(())
    })();
    if let Err(error) = result {
        if began {
            // Preserve the original schema failure if rollback also refuses.
            let _ = db.execute_batch("ROLLBACK");
        }
        return Err(error);
    }
    // The validated union is safe to interpolate into a non-bindable PRAGMA.
    // Apply it only after ownership validation and initialization commit.
    db.pragma_update(None, "journal_mode", journal_mode.as_str())
        .map_err(sqlite_error)?;
    Ok(())
}

/// Reconstruct a [`SessionHeader`] from a `sessions` row (TS `rowToMeta`).
pub fn row_to_meta(row: &SessionRow) -> Result<SessionHeader, String> {
    if row.created_at < 0 {
        return Err("stored session createdAt must be a non-negative safe integer".to_string());
    }
    let version = u64::try_from(row.version).map_err(|_| {
        format!(
            "stored session version must be a non-negative integer, got {}",
            row.version
        )
    })?;
    let created_at = row.created_at as u64;
    let seed_length = row
        .seed_length
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                format!("stored session seedLength must be a non-negative integer, got {value}")
            })
        })
        .transpose()?;
    let delegation_depth = row
        .delegation_depth
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                format!(
                    "stored session delegationDepth must be a non-negative integer, got {value}"
                )
            })
        })
        .transpose()?;
    Ok(SessionHeader {
        version,
        id: session_id(row.id.clone()),
        created_at,
        cwd: row.cwd.clone(),
        parent_session: row.parent_session.as_deref().map(session_id),
        seed_length,
        origin: row.origin.clone(),
        delegation_depth,
        agent_preset: row.agent_preset.clone(),
    })
}

/// Reconstruct a [`SessionEvent`] from an `events` row (TS `rowToEvent`).
pub fn row_to_event(row: &EventRow) -> Result<SessionEvent, String> {
    let data: serde_json::Value = serde_json::from_str(&row.data)
        .map_err(|error| format!("stored session event data is not valid JSON: {error}"))?;
    let source_event_seqs = row
        .source_event_seqs
        .as_deref()
        .map(|text| {
            serde_json::from_str::<Vec<u64>>(text)
                .map_err(|error| format!("stored sourceEventSeqs is not valid JSON: {error}"))
        })
        .transpose()?;
    let surface_op = row
        .surface_op
        .as_deref()
        .map(|text| {
            serde_json::from_str::<SurfaceOp>(text)
                .map_err(|error| format!("stored surfaceOp is not valid JSON: {error}"))
        })
        .transpose()?;
    let seq = u64::try_from(row.seq).map_err(|_| {
        format!(
            "stored session event seq must be a non-negative integer, got {}",
            row.seq
        )
    })?;
    Ok(SessionEvent {
        type_: row.type_.clone(),
        seq,
        time: row.time,
        data,
        ignorable: (row.ignorable == Some(1)).then_some(true),
        surface_op,
        source_event_seqs,
    })
}

/// `scanRows` output: the preserved prefix plus the seq the physical delete
/// starts at when a torn tail exists.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanRowsResult {
    pub preserved: Vec<SessionEvent>,
    pub torn_from: Option<u64>,
}

/// Find the preserved prefix of ordered event rows (TS `scanRows`).
///
/// Fully written rows in an interrupted final turn remain in the prefix. The
/// first unparsable row or seq gap after the last `turn/end` marks a
/// tolerated torn tail; the same hole in the committed region rejects.
pub fn scan_rows(rows: &[EventRow], base: u64) -> Result<ScanRowsResult, String> {
    // Pass 1: parse each row's data; a row whose data is not valid JSON is a
    // hole. (The seq/type COLUMNS are always present even when `data` is
    // corrupt.)
    let parsed: Vec<Option<SessionEvent>> = rows.iter().map(|row| row_to_event(row).ok()).collect();

    // The last index that is a valid `turn/end` — holes through a closed
    // turn are always committed corruption.
    let mut last_turn_end: Option<usize> = None;
    for index in (0..rows.len()).rev() {
        if parsed[index].is_some() && rows[index].type_ == "turn/end" {
            last_turn_end = Some(index);
            break;
        }
    }

    // Preserve the contiguous prefix, including a complete interrupted turn;
    // holes through the last committed boundary throw, while later holes
    // stop.
    let mut preserved: Vec<SessionEvent> = Vec::new();
    for index in 0..rows.len() {
        let Some(event) = &parsed[index] else {
            if last_turn_end.is_some_and(|last| index <= last) {
                return Err(format!(
                    "corrupt session log: unparsable committed event at seq {}",
                    rows[index].seq
                ));
            }
            break; // torn tail fragment after the last turn/end — stop, tolerate
        };
        let expected = base + index as u64;
        if event.seq != expected {
            if last_turn_end.is_some_and(|last| index <= last) {
                return Err(format!(
                    "corrupt session log: seq gap in committed region (expected {expected}, got {})",
                    event.seq
                ));
            }
            break; // gap after the last turn/end — torn tail, stop
        }
        preserved.push(event.clone());
    }

    // Any rows past the preserved prefix are a never-committed torn tail;
    // their first seq is the deletion point for load's physical repair.
    let torn_from = (preserved.len() < rows.len()).then(|| base + preserved.len() as u64);
    Ok(ScanRowsResult {
        preserved,
        torn_from,
    })
}
