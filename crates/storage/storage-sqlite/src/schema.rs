//! Schema + open-time helpers for the SQLite storage backend. Rust port of
//! `packages/storage/storage-sqlite/src/schema.ts`: the physical layout
//! version, the database open/configure sequence (permissions, pragmas,
//! version stamp/reject), and the unit metadata tables.
//!
//! # Deviations
//!
//! - The blocking open runs synchronously; callers wrap it in
//!   `spawn_blocking` (the Node async-fs threadpool equivalent).

use std::path::Path;

use rusqlite::Connection;

use dsh_storage::{StorageError, StorageErrorCode};

/// The on-disk physical layout version, stored in `PRAGMA user_version` (TS
/// `STORAGE_SQLITE_SCHEMA_VERSION`). Orthogonal to each unit's own `version`.
pub const STORAGE_SQLITE_SCHEMA_VERSION: i64 = 1;

/// Journal modes the backend will run under (TS `JournalMode`). `wal` is
/// the default; the rollback-journal modes exist for filesystems where
/// WAL's shared-memory files do not work. `memory`/`off` are excluded:
/// dropping journal durability silently contradicts the durability clause
/// of the KV backend contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalMode {
    #[default]
    Wal,
    Delete,
    Truncate,
    Persist,
}

impl JournalMode {
    pub fn pragma(&self) -> &'static str {
        match self {
            JournalMode::Wal => "WAL",
            JournalMode::Delete => "DELETE",
            JournalMode::Truncate => "TRUNCATE",
            JournalMode::Persist => "PERSIST",
        }
    }
}

/// Physical table name for one unit table (TS `recordTableName`). Both
/// segments are validated against `UNIT_NAME_RE` before reaching this, so
/// the result is safe to interpolate into DDL and prepared-statement text.
pub fn record_table_name(unit: &str, table: &str) -> String {
    format!("u_{unit}_{table}")
}

/// Exclusively create a missing database file with owner-only permissions
/// (TS `createDatabaseFile`). Existing files retain their modes; errors
/// other than `EEXIST` propagate.
fn create_database_file(path: &Path) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(handle) => {
                drop(handle);
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    }
    #[cfg(not(unix))]
    {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(handle) => {
                drop(handle);
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// Open the database and apply its schema and pragmas (TS `openDatabase`).
/// Missing directories and database files are created owner-only
/// (`:memory:` skips filesystem setup). A zero `user_version` is stamped
/// with [`STORAGE_SQLITE_SCHEMA_VERSION`]; every other non-current version
/// rejects rather than being migrated in place.
pub fn open_database(path: &str, journal_mode: JournalMode) -> Result<Connection, StorageError> {
    let actual = if path == ":memory:" {
        path.to_string()
    } else {
        let resolved = std::path::absolute(path)
            .map_err(|error| {
                StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!("sqlite backend: invalid path: {error}"),
                )
            })?
            .to_string_lossy()
            .to_string();
        if let Some(parent) = Path::new(&resolved).parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                StorageError::new(
                    StorageErrorCode::MalformedMedium,
                    format!("sqlite backend: failed to create directory: {error}"),
                )
            })?;
        }
        create_database_file(Path::new(&resolved)).map_err(|error| {
            StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!("sqlite backend: failed to create database file: {error}"),
            )
        })?;
        resolved
    };
    let db = Connection::open(&actual).map_err(|error| {
        StorageError::new(
            StorageErrorCode::MalformedMedium,
            format!("sqlite backend: failed to open database: {error}"),
        )
    })?;
    if let Err(error) = configure_database(&db, &actual, journal_mode) {
        let _ = db.close();
        return Err(error);
    }
    Ok(db)
}

fn configure_database(
    db: &Connection,
    path: &str,
    journal_mode: JournalMode,
) -> Result<(), StorageError> {
    db.execute_batch("PRAGMA foreign_keys = ON")
        .map_err(sql_error)?;
    // The validated enum is safe to interpolate into a non-bindable PRAGMA.
    db.execute_batch(&format!("PRAGMA journal_mode = {}", journal_mode.pragma()))
        .map_err(sql_error)?;
    let on_disk: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(sql_error)?;
    if on_disk != 0 && on_disk != STORAGE_SQLITE_SCHEMA_VERSION {
        return Err(StorageError::new(
            StorageErrorCode::VersionMismatch,
            format!(
                "storage database at \"{path}\" has schema version {on_disk}, incompatible with this build ({STORAGE_SQLITE_SCHEMA_VERSION})"
            ),
        ));
    }
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS units (
            name    TEXT PRIMARY KEY,
            version INTEGER NOT NULL
        ) STRICT",
    )
    .map_err(sql_error)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS unit_globals (
            unit  TEXT PRIMARY KEY REFERENCES units(name),
            value TEXT NOT NULL
        ) STRICT",
    )
    .map_err(sql_error)?;
    if on_disk == 0 {
        // Stamp fresh databases LAST: the stamp asserts the layout is
        // complete, so a failure above must leave the medium unstamped.
        db.execute_batch(&format!(
            "PRAGMA user_version = {STORAGE_SQLITE_SCHEMA_VERSION}"
        ))
        .map_err(sql_error)?;
    }
    Ok(())
}

fn sql_error(error: rusqlite::Error) -> StorageError {
    StorageError::new(
        StorageErrorCode::MalformedMedium,
        format!("sqlite backend: {error}"),
    )
}
