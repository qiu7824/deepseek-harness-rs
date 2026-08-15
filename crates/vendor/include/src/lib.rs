//! Include files in cordis configurations: Rust port of
//! `@deepseek-ai/cordis-plugin-include` v1.0.6.
//!
//! Mounts a file-backed loader subtree (`Include extends EntryTree` in TS):
//! reads a YAML/JSON entry list, applies runtime patches, and debounces
//! write-backs into the same file with a temp-file + rename dance.
//!
//! # Deviations
//!
//! - Non-JSON/YAML config files are rejected (TS additionally supports JS
//!   module imports; those belong to the embedded-JS-runtime milestone).
//! - YAML `!!js` scalars parse into `{ "__jsExpr": ... }` nodes like TS, but
//!   dumps emit the long tag form `!<tag:yaml.org,2002:js>` instead of the
//!   `!!js` shorthand (semantically identical; evaluation itself is
//!   unsupported until the JS runtime lands).
//! - The `%C` printf-style warn formatting collapses to plain messages.

pub mod include;
pub mod patch;
pub mod yaml;

pub use include::{Include, IncludeConfig, IncludePlugin, plugin};
pub use patch::{PatchOptions, apply_entry_patches};
pub use yaml::{json_to_yaml, yaml_to_json};
