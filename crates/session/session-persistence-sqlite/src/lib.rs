//! SQLite durable session-persistence backend for the DeepSeek Harness.
//! Rust port of `@deepseek-ai/dsh-session-persistence-sqlite`.

pub mod index;
pub mod invariant;
pub mod schema;

pub use index::{SqliteConfig, SqliteSessionPersistence, parse_config};
pub use schema::{JournalMode, SCHEMA_VERSION, SESSION_PERSISTENCE_SQLITE_APPLICATION_ID};
