//! `@deepseek-ai/dsh-host-plugin-inventory` — Read-only projection of the
//! current Cordis Loader plugin entries.
//!
//! # Deviations
//!
//! - TS extends `TypertRemoteService` and publishes `list` via the `@Remote`
//!   annotation; the Rust side installs the `pluginInventory` service with
//!   a direct `list()` method. Typert remote-method publication is wired
//!   with the typert/projection integration milestone (same deferral as the
//!   goal crate's remote units, round 49).
//! - A `disabled()` evaluation error (a `__jsExpr` disabled predicate, which
//!   the Rust loader cannot evaluate yet) projects as `enabled: false` —
//!   fail-closed, since the entry cannot run in Rust either.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, Service, arc};
use dsh_agent_presets::{AgentPreset, AgentPresets, PresetTrust};
use dsh_cordis_loader::LoaderService;

use crate::types::{
    PluginEntryId, PluginFiberPhase, PluginInventoryEntry, PluginInventoryPreset,
    PluginInventoryPresetRow, PluginInventorySnapshot, PresetPluginConditional,
    PresetPluginEnablement,
};

fn json_bool(value: &serde_json::Value) -> Option<bool> {
    value.as_bool()
}

fn flatten_preset_rows(rows: &[serde_json::Value], output: &mut Vec<PluginInventoryPresetRow>) {
    for value in rows {
        let Some(row) = value.as_object() else {
            continue;
        };
        let key = |name: &str| row.get(name);
        let name = key("name").and_then(serde_json::Value::as_str);
        let group = key("group").and_then(json_bool).unwrap_or(false);
        if group || name == Some("cordis:group") {
            if let Some(children) = key("config").and_then(serde_json::Value::as_array) {
                flatten_preset_rows(children, output);
            }
            continue;
        }
        let Some(module_name) = name else {
            continue;
        };
        let disabled = key("disabled");
        let (enabled, condition) = match disabled {
            None => (PresetPluginEnablement::Enabled(true), None),
            Some(value) if value.as_bool().is_some() => (
                PresetPluginEnablement::Enabled(!value.as_bool().unwrap()),
                None,
            ),
            Some(value) => (
                PresetPluginEnablement::Conditional(PresetPluginConditional::Conditional),
                Some(value.to_string()),
            ),
        };
        output.push(PluginInventoryPresetRow {
            module_name: module_name.to_string(),
            entry_id: key("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            enabled,
            condition,
            fiber_phase: None,
        });
    }
}

async fn preset_inventory_row(preset: AgentPreset, default_id: &str) -> PluginInventoryPreset {
    let mut rows = Vec::new();
    let mut broken = preset.broken.clone();
    if broken.is_none() {
        match tokio::fs::read_to_string(&preset.path).await {
            Ok(text) => match dsh_cordis_include::yaml::parse_yaml(&text) {
                Ok(value) => {
                    if let Some(sequence) = value.as_array() {
                        flatten_preset_rows(sequence, &mut rows);
                    } else {
                        broken = Some("composition root must be a list".to_string());
                    }
                }
                Err(error) => broken = Some(format!("parse preset {}: {error}", preset.id)),
            },
            Err(error) => broken = Some(format!("read preset {}: {error}", preset.id)),
        }
    }
    PluginInventoryPreset {
        is_default: preset.id == default_id,
        id: preset.id,
        trust: match preset.trust {
            PresetTrust::System => "system",
            PresetTrust::User => "user",
        }
        .to_string(),
        name: preset.name,
        broken,
        rows,
    }
}

/// Read the exact Agent Preset roster composed by the production Host.
pub async fn composition_inventory(presets: &AgentPresets) -> Vec<PluginInventoryPreset> {
    let default_id = presets.default_id();
    let discovered = match presets.list().await {
        Ok(discovered) => discovered,
        Err(error) => {
            tracing::warn!("plugin-inventory: {error}");
            return Vec::new();
        }
    };
    let mut inventory = Vec::with_capacity(discovered.len());
    for preset in discovered {
        inventory.push(preset_inventory_row(preset, &default_id).await);
    }
    inventory.sort_by(|left, right| {
        right
            .is_default
            .cmp(&left.is_default)
            .then(left.id.cmp(&right.id))
    });
    inventory
}

/// Convert a pre-discovered roster into the public inventory model.
pub async fn composition_inventory_from_presets(
    presets: Vec<AgentPreset>,
    default_id: &str,
) -> Vec<PluginInventoryPreset> {
    let mut inventory = Vec::with_capacity(presets.len());
    for preset in presets {
        inventory.push(preset_inventory_row(preset, default_id).await);
    }
    inventory.sort_by(|left, right| {
        right
            .is_default
            .cmp(&left.is_default)
            .then(left.id.cmp(&right.id))
    });
    inventory
}

/// The service name under which the gateway is exposed.
pub const SERVICE_NAME: &str = "pluginInventory";

/// Cordis plugin name (the Rust static-registry equivalent of the TS package
/// entry name `@deepseek-ai/dsh-host-plugin-inventory`).
pub const NAME: &str = "host-plugin-inventory";

/// Runtime mirror of cordis `FiberState` to the public phase vocabulary
/// (TS `FIBER_STATE` + `FIBER_PHASE`; `disposed` collapses to `None`).
fn fiber_phase(state: cordis::FiberState) -> Option<PluginFiberPhase> {
    match state {
        cordis::FiberState::Pending => Some(PluginFiberPhase::Pending),
        cordis::FiberState::Loading => Some(PluginFiberPhase::Loading),
        cordis::FiberState::Active => Some(PluginFiberPhase::Active),
        cordis::FiberState::Failed => Some(PluginFiberPhase::Failed),
        cordis::FiberState::Unloading => Some(PluginFiberPhase::Unloading),
        cordis::FiberState::Disposed => None,
    }
}

/// Remote-only service exposing the Loader's current non-group entry state.
pub struct PluginInventoryGateway {
    loader: Arc<LoaderService>,
    presets: Option<Arc<AgentPresets>>,
}

impl Service for PluginInventoryGateway {
    fn service_name(&self) -> &'static str {
        SERVICE_NAME
    }
}

impl PluginInventoryGateway {
    /// Construct and register as `ctx.pluginInventory` (TS constructor +
    /// `super(ctx, 'pluginInventory')`).
    pub fn install(ctx: &Context) -> Result<Arc<Self>, String> {
        let loader = ctx
            .get_typed::<Arc<LoaderService>>("loader", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "plugin-inventory requires the loader service".to_string())?;
        let presets = ctx
            .get_typed::<Arc<AgentPresets>>("agentPresets", false)
            .map(|slot| slot.as_ref().clone());
        let gateway = Arc::new(Self { loader, presets });
        ctx.register_service(gateway.clone());
        Ok(gateway)
    }

    /// Read the Loader directly on every call. Cordis's internal
    /// plugin/status events already maintain `Entry.fiber` and
    /// `Fiber.state`, so a second cache would only add another lifecycle
    /// truth to keep synchronized.
    ///
    /// Returns current non-group Loader entries in Loader order.
    pub async fn list(&self) -> PluginInventorySnapshot {
        let mut entries: Vec<PluginInventoryEntry> = Vec::new();
        for entry in self.loader.tree.entries() {
            let options = entry.options.lock().clone();
            if options.group.unwrap_or(false) {
                continue;
            }
            let enabled = match entry.disabled() {
                Ok(disabled) => !disabled,
                Err(_) => false,
            };
            let fiber_phase = entry
                .fiber
                .lock()
                .as_ref()
                .and_then(|fiber| fiber_phase(fiber.state()));
            entries.push(PluginInventoryEntry {
                entry_id: PluginEntryId::new(entry.id()),
                module_name: options.name,
                enabled,
                fiber_phase,
            });
        }
        let agent_presets = match self.presets.as_ref() {
            Some(presets) => composition_inventory(presets).await,
            None => Vec::new(),
        };
        PluginInventorySnapshot {
            entries,
            agent_presets,
        }
    }
}

/// The Cordis plugin form.
pub struct PluginInventoryGatewayPlugin;

#[async_trait]
impl Plugin for PluginInventoryGatewayPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["loader"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        PluginInventoryGateway::install(ctx)
            .map(|_| ())
            .map_err(|error| PluginError::new(arc(error)))
    }
}
