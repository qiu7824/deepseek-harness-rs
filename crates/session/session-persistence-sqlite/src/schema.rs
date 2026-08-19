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

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::{SessionEvent, SurfaceOp};
    use rusqlite::OptionalExtension;

    fn event(type_: &str, seq: u64, time: i64, data: serde_json::Value) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time,
            data,
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    fn one_turn_log() -> Vec<SessionEvent> {
        vec![
            event("turn/start", 0, 1, serde_json::json!({"turn": 1})),
            event(
                "user/message",
                1,
                2,
                serde_json::json!({
                    "id": "one-turn-user", "role": "user",
                    "content": [{"type": "text", "text": "hi"}], "source": {"kind": "user"},
                }),
            ),
            event(
                "step/start",
                2,
                3,
                serde_json::json!({"turn": 1, "step": 1}),
            ),
            event(
                "assistant/message",
                3,
                4,
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "id": "one-turn-assistant", "role": "assistant",
                        "content": [{"type": "text", "text": "hello"}],
                        "source": {"kind": "model", "provider": "mock", "model": "mock"},
                    },
                }),
            ),
            event("step/end", 4, 5, serde_json::json!({"turn": 1, "step": 1})),
            event(
                "turn/end",
                5,
                6,
                serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
            ),
        ]
    }

    /// Build EventRows from SessionEvents (TS test helper `rows`).
    fn rows(events: &[SessionEvent]) -> Vec<EventRow> {
        events
            .iter()
            .map(|event| EventRow {
                seq: event.seq as i64,
                type_: event.type_.clone(),
                time: event.time,
                data: serde_json::to_string(&event.data).unwrap(),
                source_event_seqs: event
                    .source_event_seqs
                    .as_ref()
                    .map(|seqs| serde_json::to_string(seqs).unwrap()),
                surface_op: event
                    .surface_op
                    .as_ref()
                    .map(|op| serde_json::to_string(op).unwrap()),
                ignorable: event.ignorable.and_then(|ignorable| ignorable.then_some(1)),
            })
            .collect()
    }

    #[test]
    fn scan_rows_preserves_full_log_ending_on_turn_end() {
        let log = one_turn_log();
        let scan = scan_rows(&rows(&log), 0).unwrap();
        assert_eq!(scan.preserved, log);
        assert_eq!(scan.torn_from, None);
    }

    #[test]
    fn scan_rows_preserves_real_events_of_interrupted_turn() {
        let mut with_open_turn = one_turn_log();
        with_open_turn.push(event("turn/start", 6, 7, serde_json::json!({"turn": 2})));
        with_open_turn.push(event(
            "step/start",
            7,
            8,
            serde_json::json!({"turn": 2, "step": 1}),
        ));
        let scan = scan_rows(&rows(&with_open_turn), 0).unwrap();
        assert_eq!(
            scan.preserved
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(scan.torn_from, None);
    }

    #[test]
    fn scan_rows_flags_torn_tail_at_seq_gap() {
        let gapped = vec![
            event("turn/start", 0, 1, serde_json::json!({"turn": 1})),
            event(
                "step/start",
                2,
                2,
                serde_json::json!({"turn": 1, "step": 1}),
            ), // seq 1 missing
        ];
        let scan = scan_rows(&rows(&gapped), 0).unwrap();
        assert_eq!(scan.preserved.len(), 1);
        assert_eq!(scan.preserved[0].seq, 0);
        assert_eq!(scan.torn_from, Some(1));
    }

    #[test]
    fn scan_rows_empty_log_has_no_torn_tail() {
        let scan = scan_rows(&[], 0).unwrap();
        assert_eq!(scan.preserved, Vec::new());
        assert_eq!(scan.torn_from, None);
    }

    #[test]
    fn scan_rows_rejects_seq_gap_in_committed_region() {
        let gapped = vec![
            event("turn/start", 0, 1, serde_json::json!({"turn": 1})),
            event(
                "step/start",
                2,
                2,
                serde_json::json!({"turn": 1, "step": 1}),
            ), // seq 1 missing
            event(
                "turn/end",
                3,
                3,
                serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
            ),
        ];
        let error = scan_rows(&rows(&gapped), 0).unwrap_err();
        assert!(error.contains("seq gap in committed region"), "{error}");
    }

    #[test]
    fn scan_rows_rejects_unparsable_committed_row() {
        let corrupt = vec![
            EventRow {
                seq: 0,
                type_: "turn/start".to_string(),
                time: 1,
                data: "{not json".to_string(),
                source_event_seqs: None,
                surface_op: None,
                ignorable: None,
            },
            EventRow {
                seq: 1,
                type_: "turn/end".to_string(),
                time: 2,
                data: serde_json::to_string(
                    &serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
                )
                .unwrap(),
                source_event_seqs: None,
                surface_op: None,
                ignorable: None,
            },
        ];
        let error = scan_rows(&corrupt, 0).unwrap_err();
        assert!(error.contains("unparsable committed event"), "{error}");
    }

    #[test]
    fn scan_rows_tolerates_unparsable_torn_tail() {
        let mut corrupt_tail = rows(&one_turn_log());
        corrupt_tail.push(EventRow {
            seq: 6,
            type_: "turn/start".to_string(),
            time: 7,
            data: "{not json".to_string(),
            source_event_seqs: None,
            surface_op: None,
            ignorable: None,
        });
        let scan = scan_rows(&corrupt_tail, 0).unwrap();
        assert_eq!(scan.preserved, one_turn_log());
        assert_eq!(scan.torn_from, Some(6));
    }

    #[test]
    fn scan_rows_with_surface_columns_reconstructs_surface_fields() {
        let rows = vec![
            EventRow {
                seq: 0,
                type_: "user/message".to_string(),
                time: 1,
                data: serde_json::to_string(&serde_json::json!({"content": [{"type": "text", "text": "hi"}], "source": {"kind": "user"}})).unwrap(),
                source_event_seqs: None,
                surface_op: Some("{\"op\":\"replace\",\"start\":0,\"end\":0}".to_string()),
                ignorable: None,
            },
            EventRow {
                seq: 1,
                type_: "turn/end".to_string(),
                time: 2,
                data: serde_json::to_string(&serde_json::json!({"turn": 1, "reason": {"kind": "completed"}})).unwrap(),
                source_event_seqs: None,
                surface_op: None,
                ignorable: Some(1),
            },
        ];
        let scan = scan_rows(&rows, 0).unwrap();
        assert_eq!(scan.preserved.len(), 2);
        assert_eq!(
            scan.preserved[0].surface_op,
            Some(SurfaceOp::Replace { start: 0, end: 0 })
        );
        assert_eq!(scan.preserved[0].source_event_seqs, None);
        assert_eq!(scan.preserved[1].surface_op, None);
        assert_eq!(scan.preserved[1].ignorable, Some(true));
    }

    #[test]
    fn row_to_event_parses_surface_fields() {
        let row = EventRow {
            seq: 0,
            type_: "assistant/message".to_string(),
            time: 1,
            data: "{\"turn\":1,\"step\":1,\"content\":[]}".to_string(),
            source_event_seqs: Some("[3,5]".to_string()),
            surface_op: Some("\"append\"".to_string()),
            ignorable: None,
        };
        let event = row_to_event(&row).unwrap();
        assert_eq!(event.source_event_seqs, Some(vec![3, 5]));
        assert_eq!(event.surface_op, Some(SurfaceOp::Append));
    }

    #[test]
    fn row_to_meta_restores_optional_fields_and_rejects_negative_created_at() {
        let base = SessionRow {
            id: "with-origin".to_string(),
            version: 0,
            created_at: 1,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: Some("subagent".to_string()),
            incarnation: "i".to_string(),
            revision: 1,
            delegation_depth: None,
            agent_preset: None,
        };
        let meta = row_to_meta(&base).unwrap();
        assert_eq!(meta.origin.as_deref(), Some("subagent"));

        let mut composed = base.clone();
        composed.agent_preset = Some("minimal".to_string());
        composed.origin = None;
        assert_eq!(
            row_to_meta(&composed).unwrap().agent_preset.as_deref(),
            Some("minimal")
        );

        let mut negative = base.clone();
        negative.created_at = -1;
        let error = row_to_meta(&negative).unwrap_err();
        assert!(error.contains("non-negative safe integer"), "{error}");
    }

    fn fresh_db_path(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir()
            .join(format!(
                "dsh-sqlite-schema-{tag}-{}",
                uuid::Uuid::new_v4().simple()
            ))
            .join("sessions.db");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        path
    }

    fn pragma(db: &Connection, name: &str) -> i64 {
        db.query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn open_database_stamps_identity_and_version() {
        let path = fresh_db_path("stamp");
        {
            let db = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap();
            assert_eq!(pragma(&db, "user_version"), SCHEMA_VERSION);
            assert_eq!(
                pragma(&db, "application_id"),
                SESSION_PERSISTENCE_SQLITE_APPLICATION_ID
            );
            let store_id: String = db
                .query_row(
                    "SELECT store_id FROM persistence_state WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!store_id.is_empty());
        }
        // Reopen: same stamps, no re-initialization failure.
        let db = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap();
        assert_eq!(pragma(&db, "user_version"), SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn open_database_rejects_other_schema_versions() {
        let path = fresh_db_path("versions");
        {
            let db = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap();
            db.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        let error = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap_err();
        assert!(error.contains("incompatible with this build"), "{error}");

        let older = fresh_db_path("older");
        {
            let db = open_database(older.to_str().unwrap(), JournalMode::Wal).unwrap();
            db.pragma_update(None, "user_version", SCHEMA_VERSION - 1)
                .unwrap();
        }
        let error = open_database(older.to_str().unwrap(), JournalMode::Wal).unwrap_err();
        assert!(error.contains("incompatible with this build"), "{error}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let _ = std::fs::remove_dir_all(older.parent().unwrap());
    }

    #[test]
    fn open_database_rejects_unversioned_user_databases_without_mutation() {
        let path = fresh_db_path("unversioned");
        {
            let legacy = Connection::open(path.to_str().unwrap()).unwrap();
            legacy
                .execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY)")
                .unwrap();
        }
        let error = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap_err();
        assert!(
            error.contains("unversioned schema or application identity"),
            "{error}"
        );

        let unchanged = Connection::open(path.to_str().unwrap()).unwrap();
        assert_eq!(pragma(&unchanged, "user_version"), 0);
        let journal: String = unchanged
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "delete");
        let name: String = unchanged
            .query_row(
                "SELECT name FROM sqlite_schema WHERE type = 'table' AND name = 'sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "sessions");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn open_database_counts_sqlite_named_tables_as_user_owned() {
        let path = fresh_db_path("sqliteX");
        {
            let unrelated = Connection::open(path.to_str().unwrap()).unwrap();
            unrelated
                .execute_batch("CREATE TABLE sqliteX (value TEXT)")
                .unwrap();
            unrelated
                .execute_batch("INSERT INTO sqliteX VALUES ('safe')")
                .unwrap();
        }
        let error = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap_err();
        assert!(
            error.contains("unversioned schema or application identity"),
            "{error}"
        );

        let unchanged = Connection::open(path.to_str().unwrap()).unwrap();
        let value: String = unchanged
            .query_row("SELECT value FROM sqliteX", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "safe");
        assert_eq!(pragma(&unchanged, "application_id"), 0);
        assert_eq!(pragma(&unchanged, "user_version"), 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn open_database_rejects_foreign_application_identity() {
        let path = fresh_db_path("foreign");
        {
            let foreign = Connection::open(path.to_str().unwrap()).unwrap();
            foreign
                .pragma_update(None, "application_id", 12345)
                .unwrap();
            foreign
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .unwrap();
        }
        let error = open_database(path.to_str().unwrap(), JournalMode::Wal).unwrap_err();
        assert!(error.contains("has application id 12345"), "{error}");

        let unchanged = Connection::open(path.to_str().unwrap()).unwrap();
        assert_eq!(pragma(&unchanged, "application_id"), 12345);
        assert_eq!(pragma(&unchanged, "user_version"), SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn open_database_rolls_back_failed_initialization() {
        let path = fresh_db_path("rollback");
        {
            let conflicting = Connection::open(path.to_str().unwrap()).unwrap();
            conflicting
                .pragma_update(
                    None,
                    "application_id",
                    SESSION_PERSISTENCE_SQLITE_APPLICATION_ID,
                )
                .unwrap();
            conflicting
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .unwrap();
            conflicting
                .execute_batch(
                    "CREATE VIEW persistence_state AS SELECT 1 AS singleton, 'foreign' AS store_id",
                )
                .unwrap();
        }
        assert!(open_database(path.to_str().unwrap(), JournalMode::Wal).is_err());

        let unchanged = Connection::open(path.to_str().unwrap()).unwrap();
        let view: Option<String> = unchanged
            .query_row(
                "SELECT type FROM sqlite_schema WHERE name = 'persistence_state'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(view.as_deref(), Some("view"));
        for table in ["sessions", "events"] {
            let row: Option<String> = unchanged
                .query_row(
                    "SELECT type FROM sqlite_schema WHERE name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .optional()
                .unwrap();
            assert_eq!(row, None, "{table} must not exist after rollback");
        }
        assert_eq!(
            pragma(&unchanged, "application_id"),
            SESSION_PERSISTENCE_SQLITE_APPLICATION_ID
        );
        assert_eq!(pragma(&unchanged, "user_version"), SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
