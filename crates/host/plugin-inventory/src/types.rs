//! Point-in-time inventory types. Rust port of
//! `packages/host/plugin-inventory/src/types.ts`.

use dsh_brand::Branded;
use serde::{Deserialize, Serialize};

/// Brand marker for stable Loader-tree entry identities.
#[doc(hidden)]
pub enum PluginEntryIdTag {}

/// Stable Loader-tree identity of one configured plugin entry.
pub type PluginEntryId = Branded<PluginEntryIdTag>;

/// Lifecycle state of an entry's root Fiber, or `None` when it has no live
/// root Fiber. `disposed` maps to `None` (the TS `FIBER_PHASE` table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginFiberPhase {
    Pending,
    Loading,
    Active,
    Failed,
    Unloading,
}

/// One non-group Loader entry exposed to trusted clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInventoryEntry {
    pub entry_id: PluginEntryId,
    /// Exact module specifier imported by the Loader entry.
    pub module_name: String,
    /// Effective Loader enablement, including disabled ancestor groups.
    pub enabled: bool,
    /// `None` when the entry has no live root Fiber (wire value `null`).
    pub fiber_phase: Option<PluginFiberPhase>,
}

/// One plugin row declared by an Agent Preset composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInventoryPresetRow {
    pub module_name: String,
    pub entry_id: Option<String>,
    pub enabled: PresetPluginEnablement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    pub fiber_phase: Option<PluginFiberPhase>,
}

/// Effective enablement of one preset composition row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PresetPluginEnablement {
    Enabled(bool),
    Conditional(PresetPluginConditional),
}

/// Wire literal for a predicate that only a mounted Loader can evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetPluginConditional {
    #[serde(rename = "conditional")]
    Conditional,
}

/// One discovered Agent Preset and its flattened non-group plugin rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInventoryPreset {
    pub id: String,
    pub trust: String,
    pub is_default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broken: Option<String>,
    pub rows: Vec<PluginInventoryPresetRow>,
}

/// Point-in-time inventory returned by the plugin inventory Remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInventorySnapshot {
    pub entries: Vec<PluginInventoryEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_presets: Vec<PluginInventoryPreset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSetEnabledRequest {
    pub entry_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSetEnabledResult {
    pub entry: PluginInventoryEntry,
}
