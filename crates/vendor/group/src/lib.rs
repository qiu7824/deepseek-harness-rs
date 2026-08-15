//! Nested entry groups for the cordis loader: Rust port of
//! `@deepseek-ai/cordis-plugin-group`.
//!
//! The TS package is a one-line re-export of the loader's `Group` plugin
//! (`export default Group`); the Rust loader embeds the same implementation,
//! so this crate is the registry alias. The loader core registers it under
//! the builtin name `"group"` automatically.

use std::sync::Arc;

pub use dsh_cordis_loader::{EntryGroup, GroupPlugin};

/// Build the group plugin for a loader core (the core registers it under
/// the builtin name `"group"` in `LoaderService::new`).
pub fn plugin(core: Arc<dsh_cordis_loader::LoaderCore>) -> Arc<dyn cordis::Plugin> {
    Arc::new(GroupPlugin { core })
}
