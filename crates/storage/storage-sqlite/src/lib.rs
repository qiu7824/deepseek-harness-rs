//! SQLite storage backend for the storage hub: one database file hosts
//! every routed unit, document-per-row. Rust port of
//! `@deepseek-ai/dsh-storage-sqlite`.

pub mod index;
pub mod invariant;
pub mod schema;
pub mod unit;

pub use index::{Config, INJECT, NAME, SqliteStorageBackend, SqliteStoragePlugin, apply};
pub use schema::{JournalMode, STORAGE_SQLITE_SCHEMA_VERSION, open_database, record_table_name};
