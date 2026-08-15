//! SQLite schema for the disposable session full-text read model. Rust port
//! of `packages/session-query/session-query-sqlite/src/schema.ts`.
//!
//! # Deviations
//!
//! - Owner-only file creation (`0o600` / `0o700` directories) is applied on
//!   POSIX filesystems only; Windows has no equivalent mode bit and retains
//!   its default ACLs (mirrors the TS tests skipping the mode assertions on
//!   win32).

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::JournalMode;

/// Current derived-index schema version. Incompatible versions reset in place.
pub const SESSION_QUERY_SQLITE_SCHEMA_VERSION: u32 = 8;

/// SQLite application id protecting unrelated databases from derived resets.
pub const SESSION_QUERY_SQLITE_APPLICATION_ID: u32 = 0x4453_4851;

/// The recognized derived user tables (TS `DERIVED_USER_TABLES`).
const DERIVED_USER_TABLES: &[&str] = &[
    "search_state",
    "persisted_sessions",
    "persisted_docs",
    "persisted_docs_data",
    "persisted_docs_idx",
    "persisted_docs_content",
    "persisted_docs_docsize",
    "persisted_docs_config",
];

/// Exclusively create a missing database file with owner-only permissions.
/// Existing files retain their modes, and errors other than already-exists
/// propagate.
fn create_database_file(path: &Path) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(handle) => {
            drop(handle);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Open, validate, and initialize persistent and connection-local schemas.
pub fn open_search_database(
    path: &str,
    journal_mode: JournalMode,
) -> Result<Connection, String> {
    let actual: PathBuf = if path == ":memory:" {
        PathBuf::from(path)
    } else {
        // Node `resolve()` collapses dot segments against the CWD.
        std::path::absolute(Path::new(path)).map_err(|error| error.to_string())?
    };
    if actual != Path::new(":memory:") {
        let parent = actual.parent().ok_or_else(|| "database path has no parent directory".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        create_database_file(&actual)?;
    }
    let db = Connection::open(&actual).map_err(|error| error.to_string())?;
    match initialize(&db, &actual) {
        Ok(()) => {
            let journal = match journal_mode {
                JournalMode::Wal => "WAL",
                JournalMode::Delete => "DELETE",
                JournalMode::Truncate => "TRUNCATE",
                JournalMode::Persist => "PERSIST",
            };
            if let Err(error) = db.query_row(
                &format!("PRAGMA journal_mode = {journal}"),
                [],
                |row| row.get::<_, String>(0),
            ) {
                let _ = db.close();
                return Err(error.to_string());
            }
            Ok(db)
        }
        Err(error) => {
            let _ = db.close();
            Err(error)
        }
    }
}

fn initialize(db: &Connection, actual: &Path) -> Result<(), String> {
    let application_id: i64 = db
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let user_version: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let user_tables = list_user_tables(db)?;
    if application_id != 0 && application_id != SESSION_QUERY_SQLITE_APPLICATION_ID as i64 {
        return Err(format!(
            "session-search database at \"{}\" belongs to another application",
            actual.display()
        ));
    }
    if application_id == 0 && !user_tables.is_empty() {
        return Err(format!(
            "session-search database at \"{}\" is not an empty or recognized derived index",
            actual.display()
        ));
    }
    if application_id == SESSION_QUERY_SQLITE_APPLICATION_ID as i64 {
        assert_derived_user_tables(actual, &user_tables)?;
        if user_version != SESSION_QUERY_SQLITE_SCHEMA_VERSION as i64 {
            reset_derived_schema(db, &user_tables)?;
        }
    }
    ensure_persistent_schema(db)?;
    ensure_temporary_schema(db)?;
    Ok(())
}

fn list_user_tables(db: &Connection) -> Result<Vec<String>, String> {
    let mut statement = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT GLOB 'sqlite_*' ORDER BY name")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row.map_err(|error| error.to_string())?);
    }
    Ok(names)
}

fn assert_derived_user_tables(path: &Path, user_tables: &[String]) -> Result<(), String> {
    let unknown: Vec<&str> = user_tables
        .iter()
        .map(String::as_str)
        .filter(|name| !DERIVED_USER_TABLES.contains(name))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "session-search database at \"{}\" has unrecognized user tables: {}",
            path.display(),
            unknown.join(", ")
        ));
    }
    Ok(())
}

fn reset_derived_schema(db: &Connection, user_tables: &[String]) -> Result<(), String> {
    for name in user_tables {
        db.execute_batch(&format!("DROP TABLE IF EXISTS {}", quote_identifier(name)))
            .map_err(|error| error.to_string())?;
    }
    db.execute_batch("PRAGMA user_version = 0")
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn ensure_persistent_schema(db: &Connection) -> Result<(), String> {
    let batch = format!(
        "
        PRAGMA application_id = {SESSION_QUERY_SQLITE_APPLICATION_ID};
        CREATE TABLE IF NOT EXISTS search_state (
          singleton         INTEGER PRIMARY KEY CHECK (singleton = 1),
          global_generation INTEGER NOT NULL
        ) STRICT;
        INSERT OR IGNORE INTO search_state (singleton, global_generation) VALUES (1, 0);
        CREATE TABLE IF NOT EXISTS persisted_sessions (
          id             TEXT PRIMARY KEY,
          version        INTEGER NOT NULL,
          created_at     INTEGER NOT NULL,
          cwd            TEXT,
          parent_session TEXT,
          seed_length    INTEGER,
          delegation_depth INTEGER,
          agent_preset  TEXT,
          revision       TEXT NOT NULL,
          generation     INTEGER NOT NULL
        ) STRICT;
        CREATE VIRTUAL TABLE IF NOT EXISTS persisted_docs USING fts5(
          text,
          session_id UNINDEXED,
          seq UNINDEXED,
          type UNINDEXED,
          time UNINDEXED,
          surface UNINDEXED,
          codepoint_length UNINDEXED,
          tokenize = 'unicode61'
        );
        PRAGMA user_version = {SESSION_QUERY_SQLITE_SCHEMA_VERSION};
        "
    );
    db.execute_batch(&batch).map_err(|error| error.to_string())
}

fn ensure_temporary_schema(db: &Connection) -> Result<(), String> {
    let batch = "
        CREATE TEMP TABLE IF NOT EXISTS live_sessions (
          id             TEXT PRIMARY KEY,
          version        INTEGER NOT NULL,
          created_at     INTEGER NOT NULL,
          cwd            TEXT,
          parent_session TEXT,
          seed_length    INTEGER,
          delegation_depth INTEGER,
          agent_preset  TEXT,
          fingerprint    TEXT NOT NULL,
          persisted      INTEGER NOT NULL CHECK (persisted IN (0, 1)),
          generation     INTEGER NOT NULL
        ) STRICT;
        CREATE VIRTUAL TABLE IF NOT EXISTS temp.live_docs USING fts5(
          text,
          session_id UNINDEXED,
          seq UNINDEXED,
          type UNINDEXED,
          time UNINDEXED,
          surface UNINDEXED,
          codepoint_length UNINDEXED,
          tokenize = 'unicode61'
        );
    ";
    db.execute_batch(batch).map_err(|error| error.to_string())
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
