//! Package-owned invariant companion for the plugin-inventory gateway.
//! Rust port of `packages/host/plugin-inventory/src/invariant.ts`.
//!
//! No runtime invariant: the gateway reads the Loader directly on every
//! call, and the Loader owns the authoritative entry lifecycle.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-host-plugin-inventory";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "host-plugin-inventory-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Build the no-op installer (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: None,
        install: Arc::new(
            |_ctx: &Context, _fail: Arc<dyn Fn(&str) + Send + Sync>| Box::pin(async {}),
        ),
    }
}

/// Register the plugin-inventory invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the plugin-inventory invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct PluginInventoryInvariantPlugin;

#[async_trait]
impl Plugin for PluginInventoryInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(_ctx);
        Ok(())
    }
}
