//! Type driven schema validator: Rust port of `@deepseek-ai/schemastery`
//! v3.18.1.
//!
//! The TS version exports a single callable `Schema` object with 20+ node
//! types, a [`StandardSchemaV1`]-style `~standard` prop, recursive/lazy
//! schemas, i18n, and JSON serialization. The Rust port keeps the same node
//! set and validation semantics on a [`Data`] model that extends
//! `serde_json::Value` with `Date`/`RegExp`/binary/instance variants.
//!
//! # Deviations
//!
//! - `Schema` is a value (not callable): build with `Schema::object(...)`,
//!   validate with `Schema::validate(&schema, data)` or the
//!   [`StandardSchemaV1`] trait.
//! - `Schema.is(Constructor)` (JS class identity) is reduced to
//!   `Schema::is(name)` checking [`Data::Instance`] names.
//! - `transform` callbacks are Rust closures; the TS `new Function(...)`
//!   string-callback deserialization path has no equivalent.
//! - `toJSON`/`fromJSON` schema serialization is not yet implemented
//!   (needed by settings persistence in M2).
//! - JS regex dialect differences (lookbehind/backreferences) surface as
//!   `ValidationError`; `u/g/y` flags are ignored (they do not affect
//!   `test()` semantics).
//! - `parseDate` string fallbacks are RFC3339-based (JS `Date` parsing is
//!   implementation-defined).

pub mod data;
pub mod error;
pub mod meta;
pub mod schema;

pub use data::Data;
pub use error::{Issue, StandardResult, ValidationError};
pub use meta::{Badge, Desc, Meta, Options, PathSeg, Pattern};
pub use schema::{Node, Schema, StandardSchemaV1};
