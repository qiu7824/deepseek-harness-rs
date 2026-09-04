//! Function plugin registering the `sessionStats` projection unit. Rust port
//! of `packages/session/session-stats/src/index.ts`.

use std::sync::Arc;

use cordis::{ArcValue, Context};
use dsh_session_projection::SessionProjectionRegistry;

use crate::projection::session_stats_projection_definition;

/// Cordis plugin name (TS `name`).
pub const NAME: &str = "session-stats";

/// The projection registry is the plugin's whole purpose; without it the
/// fiber stays pending (TS `inject`).
pub const INJECT: [&str; 1] = ["sessionProjections"];

/// Register the `sessionStats` unit; the registration is an effect on this
/// plugin's fiber, so unloading removes the key (TS `apply`).
pub fn apply(ctx: &Context) -> Result<(), String> {
    let registry: Arc<Arc<SessionProjectionRegistry>> = ctx
        .get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
        .ok_or_else(|| "sessionProjections service is not configured".to_string())?;
    registry.register(ctx, session_stats_projection_definition())?;
    registry.register(
        ctx,
        crate::insights::context_insights_projection_definition(),
    )?;
    Ok(())
}

/// Cordis plugin entrypoint carrying the function plugin's namespace
/// (`inject = ['sessionProjections']`; no default export).
pub struct StatsPlugin;

#[async_trait::async_trait]
impl cordis::Plugin for StatsPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), cordis::PluginError> {
        apply(ctx).map_err(|error| cordis::PluginError::new(cordis::arc(error)))
    }
}
