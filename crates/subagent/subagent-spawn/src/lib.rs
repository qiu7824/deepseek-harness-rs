//! The in-process SPAWN subagent backend: registers a
//! [`dsh_subagent::SubagentProvider`] on `ctx.subagents` that runs each
//! child as a fresh child Agent on the same cordis context (its own
//! session, own system prompt, zero parent context). Rust port of
//! `packages/subagent/subagent-spawn-in-process/src/index.ts`.
//!

pub mod invariant;

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_subagent::{
    ContinuableCreateRequest, ContinuableCreateSpec, InProcessRunOptions,
    ResolvedSubagentStartRequest, SubagentCapabilities, SubagentError, SubagentProvider,
    SubagentRun, start_in_process_run,
};

/// Cordis plugin name.
pub const NAME: &str = "subagent-spawn-in-process";

/// Services required before the provider can register.
pub const INJECT: [&str; 1] = ["subagents"];

/// Config: the registry name to register the provider under.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Provider name on `ctx.subagents` (default `spawn`).
    pub provider_name: String,
}

/// The spawn provider: a spawned child starts fresh — it never sees the
/// parent conversation.
struct SpawnInProcessProvider {
    name: String,
}

#[async_trait::async_trait]
impl SubagentProvider for SpawnInProcessProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities {
            output_schema: true,
            depth_limit: true,
            tool_filter: true,
            persona: true,
        }
    }

    fn inherits_parent_context(&self) -> bool {
        false
    }

    async fn start(
        &self,
        request: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        start_in_process_run(&request, InProcessRunOptions::default()).await
    }

    async fn prepare_continuable(
        &self,
        _request: ContinuableCreateRequest,
    ) -> Result<ContinuableCreateSpec, SubagentError> {
        // A spawned child starts fresh, so it contributes no seed.
        Ok(ContinuableCreateSpec::default())
    }
}

/// Register the spawn provider (TS `apply`).
pub fn apply(ctx: &Context, config: &Config) -> Result<(), SubagentError> {
    let runtime = ctx
        .get_typed::<Arc<dsh_subagent::SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| SubagentError::new("NO_PROVIDER", "subagents service is not mounted"))?;
    let provider_name = if config.provider_name.is_empty() {
        "spawn".to_string()
    } else {
        config.provider_name.clone()
    };
    let provider: Arc<dyn SubagentProvider> = Arc::new(SpawnInProcessProvider {
        name: provider_name,
    });
    runtime.register_provider(ctx, provider).map(|_| ())
}

/// The Cordis plugin form.
pub struct SpawnInProcessPlugin;

#[async_trait::async_trait]
impl Plugin for SpawnInProcessPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config.downcast_ref::<Config>().cloned().unwrap_or_default();
        apply(ctx, &config).map_err(|error| PluginError::from(anyhow::anyhow!(error.message)))
    }
}
