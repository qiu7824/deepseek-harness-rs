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
use dsh_cordis_loader::LoaderService;

use crate::types::{
    PluginEntryId, PluginFiberPhase, PluginInventoryEntry, PluginInventorySnapshot,
};

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

fn known_description(module_name: &str) -> Option<String> {
    match module_name {
        "dsh-context-jump" => {
            Some("左侧用户消息刻度导航；无需预加载完整历史，点击时只载入目标页。".to_string())
        }
        "dsh-task-manager" => {
            Some("会话任务清单：创建、更新、完成和查看当前任务进度。".to_string())
        }
        "dsh-web-preview-rs" => Some("Rust 原生网页与文档预览入口。".to_string()),
        _ => None,
    }
}

/// Remote-only service exposing the Loader's current non-group entry state.
pub struct PluginInventoryGateway {
    loader: Arc<LoaderService>,
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
        let gateway = Arc::new(Self { loader });
        ctx.register_service(gateway.clone());
        Ok(gateway)
    }

    /// Read the Loader directly on every call. Cordis's internal
    /// plugin/status events already maintain `Entry.fiber` and
    /// `Fiber.state`, so a second cache would only add another lifecycle
    /// truth to keep synchronized.
    ///
    /// Returns current non-group Loader entries in Loader order.
    pub fn list(&self) -> PluginInventorySnapshot {
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
            let description = known_description(&options.name);
            entries.push(PluginInventoryEntry {
                entry_id: PluginEntryId::new(entry.id()),
                module_name: options.name,
                description,
                enabled,
                fiber_phase,
            });
        }
        PluginInventorySnapshot { entries }
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
