//! Pure-type outlet of the session-projection seam (TS `types.ts`).
//!
//! # Deviation
//!
//! - The TS `SessionProjectionMap` is an empty declaration-merge table that
//!   domain packages extend at compile time. Rust has no declaration
//!   merging, so the table is replaced by the wire-JSON value type the table
//!   constrains: every key is a `String` and every value is lossless
//!   `serde_json::Value` validated by the registering unit's schema.

/// The whole-value type shared by every projection key (TS
/// `SessionProjectionMap[K]` values are wire-JSON whole values).
pub type ProjectionValue = serde_json::Value;
