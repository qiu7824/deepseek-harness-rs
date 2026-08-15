//! Agent-plane presentation selector: the row an agent preset carries to
//! say which form of its tools the model sees. Rust port of
//! `packages/core/agent-tool-presentation/src/index.ts`.
//!
//! # Deviations
//!
//! - `apply` returns `Result<(), String>` (the TS void function surfaces
//!   its failures through the cordis activation audit instead); the
//!   `codeRuntime` wait keeps the TS `ctx.inject` fiber shape.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc};
use dsh_tools::{ToolPresentationMode, ToolRuntime};

/// Cordis plugin name.
pub const NAME: &str = "tool-presentation";

/// Required services (`codeRuntime` is deliberately absent: a `native` row
/// must mount in a deployment that composes no runtime).
pub const INJECT: [&str; 1] = ["tools"];

/// Plugin config: the form this agent's model sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub mode: ToolPresentationMode,
}

fn tools_of(ctx: &Context) -> Result<Arc<ToolRuntime>, String> {
    ctx.get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|arc| arc.as_ref().clone())
        .ok_or_else(|| "dsh-agent-tool-presentation requires the tools service".to_string())
}

/// Declare the tool presentation for every agent this composition covers.
///
/// `presentAs` is itself the effect — it registers through the calling
/// context and hands back that exact disposer — so the declaration unwinds
/// with this row without a second wrapper owning it.
pub fn apply(ctx: &Context, config: Config) -> Result<(), String> {
    let tools = tools_of(ctx)?;
    if config.mode == ToolPresentationMode::Native {
        tools.present_as(ctx, ToolPresentationMode::Native)?;
        return Ok(());
    }
    // The wait is the loud failure: an entry still pending on
    // `codeRuntime` is what dsh-agent-presets reports as an unusable row.
    ctx.inject(
        InjectSpec::new(["codeRuntime"]),
        Arc::new(move |runtime_ctx: &Context, _config: ArcValue| {
            let runtime_ctx = runtime_ctx.clone();
            let mode = config.mode;
            Box::pin(async move {
                let tools =
                    tools_of(&runtime_ctx).map_err(|error| cordis::PluginError::new(arc(error)))?;
                tools
                    .present_as(&runtime_ctx, mode)
                    .map_err(|error| cordis::PluginError::new(arc(error)))?;
                Ok(())
            })
        }),
    );
    Ok(())
}

/// The Cordis plugin form (`name = "tool-presentation"`,
/// `inject = ["tools"]`).
pub struct ToolPresentationPlugin {
    pub config: Config,
}

#[async_trait::async_trait]
impl Plugin for ToolPresentationPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT.iter().copied())
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx, self.config).map_err(|error| PluginError::new(arc(error)))
    }
}
