//! Read-only projection of the current Cordis Loader plugin entries.
//! Rust port of `packages/host/plugin-inventory`.

pub mod index;
pub mod invariant;
pub mod types;

pub use index::{NAME, PluginInventoryGateway, PluginInventoryGatewayPlugin, SERVICE_NAME};
pub use types::{PluginEntryId, PluginFiberPhase, PluginInventoryEntry, PluginInventorySnapshot};
