//! Loader service for cordis: Rust port of `@deepseek-ai/cordis-plugin-loader`.
//!
//! Owns a tree of plugin entries ([`EntryTree`] / [`Entry`] /
//! [`EntryGroup`]), imports configured plugins, applies per-entry
//! `intercept`/`isolate` scopes, and persists through a pluggable `write`
//! backend (the `include` plugin supplies the file-backed tree).
//!
//! # Deviations
//!
//! - **Module resolution is a static registry**: TS dynamically imports ESM
//!   modules or `cordis:` builtins; Rust looks `name` up in a process-wide
//!   plugin registry (dynamic JS modules belong to the embedded-JS-runtime
//!   milestone).
//! - **`!!js` expressions** (`{ "__jsExpr": ... }` in configs) are detected
//!   but evaluation is not yet implemented: `interpolate` fails with a clear
//!   error instead of silently passing raw nodes.
//! - **isolate/intercept updates replace the fiber** instead of transferring
//!   live service implementations between isolation labels (the TS prototype
//!   swap + impl transfer is an optimization; end state is equivalent).
//! - Node internal module-loader hooks (`ModuleLoader`) are HMR-only and not
//!   ported.

pub mod entry;
pub mod group;
pub mod isolate;
pub mod loader;
pub mod tree;
pub mod utils;

pub use entry::{Entry, EntryOptions};
pub use group::{EntryGroup, GroupPlugin};
pub use isolate::{GlobalRealm, LocalRealm, Realm};
pub use loader::{LoaderCore, LoaderError, LoaderService, plugin};
pub use tree::EntryTree;
pub use utils::{JsExpr, interpolate, is_js_expr};
