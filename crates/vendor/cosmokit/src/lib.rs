//! Common utilities: Rust port of `@deepseek-ai/cosmokit` v1.8.2.
//!
//! Module layout mirrors the upstream package:
//! - [`array`]: set/array helpers;
//! - [`types`]: binary (base64/hex), deep clone/equality helpers;
//! - [`misc`]: object/entry helpers;
//! - [`string`]: case and path formatting;
//! - [`time`]: duration constants, parsing, and formatting.
//!
//! # Deviations (JavaScript-only surface, no Rust equivalent)
//!
//! - Type-level helpers (`Dict`, `Get`, `Extract`, `MaybeArray`,
//!   `Promisify`, `Awaitable`, `Intersect`, type-level `camelize`/
//!   `hyphenate`) are dropped.
//! - `is<K>()` global-constructor predicates, `defineProperty()`, and
//!   `clone()` of arbitrary JS objects are dropped (`serde_json::Value`
//!   is already deep-cloned by `Clone`).
//! - `isNullable`/`isNonNullable` are replaced by Rust's `Option`; see
//!   [`misc::is_null`] for JSON null handling.

pub mod array;
pub mod misc;
pub mod string;
pub mod time;
pub mod types;

pub use array::*;
pub use misc::*;
pub use string::*;
pub use time::*;
pub use types::*;
