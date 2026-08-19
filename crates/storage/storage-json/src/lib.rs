//! JSON storage backend: one human-readable file per unit, published by
//! atomic whole-file rewrite. Rust port of `@deepseek-ai/dsh-storage-json`.

pub mod atomic;
pub mod format;
pub mod index;
pub mod invariant;
pub mod unit;

pub use format::{UnitState, parse, serialize};
pub use index::{Config, INJECT, JsonStorageBackend, JsonStoragePlugin, NAME, apply, root_of};
