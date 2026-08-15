//! Change-event vocabulary of the domain data form. Rust port of
//! `packages/storage/storage-domain/src/events.ts`. Every durable write
//! emits one event after the backend resolves durability, carrying the new
//! snapshot and an operation discriminant — never the old value.

use serde_json::Value as JsonValue;

/// One durable domain change; a closed union — switch on the operation (TS
/// `DomainChanged`).
#[derive(Debug, Clone, PartialEq)]
pub enum DomainChanged {
    Put {
        /// Owning domain name.
        domain: String,
        /// Table name; `''` for a global-singleton write.
        table: String,
        /// Record key; `''` for a global-singleton write.
        key: String,
        /// The new snapshot.
        value: JsonValue,
    },
    Deleted {
        domain: String,
        table: String,
        key: String,
    },
}
