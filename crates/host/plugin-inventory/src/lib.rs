//! Read-only projection of the current Cordis Loader plugin entries.
//! Rust port of `packages/host/plugin-inventory`.

pub mod index;
pub mod invariant;
pub mod types;

pub use index::{
    NAME, PluginInventoryGateway, PluginInventoryGatewayPlugin, SERVICE_NAME, composition_inventory,
};
pub use types::{
    PluginEntryId, PluginFiberPhase, PluginInventoryEntry, PluginInventoryPreset,
    PluginInventoryPresetRow, PluginInventorySnapshot, PluginSetEnabledRequest,
    PluginSetEnabledResult,
};
