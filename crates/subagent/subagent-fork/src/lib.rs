//! The in-process FORK subagent backend: registers a
//! [`dsh_subagent::SubagentProvider`] on `ctx.subagents` that runs each
//! child as a child Agent SEEDED with a prefix of the parent's session log
//! — so the child inherits the parent's conversation context instead of
//! starting fresh. The seed ends at the last `turn/end`. Rust port of
//! `packages/subagent/subagent-fork-in-process/src/index.ts`.
//!
//! # Deviations
//!
//! - Structured output is not ported: the provider advertises
//!   `output_schema: false` (the capture-tool runtime arrives later).

pub mod invariant;

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_agent::Agent;
use dsh_session::SessionEvent;
use dsh_subagent::{
    ContinuableCreateRequest, ContinuableCreateSpec, InProcessRunOptions, ResolvedSubagentStartRequest,
    SubagentCapabilities, SubagentError, SubagentProvider, SubagentRun,
    start_in_process_run,
};

/// Cordis plugin name.
pub const NAME: &str = "subagent-fork-in-process";

/// Services required before the provider can register.
pub const INJECT: [&str; 1] = ["subagents"];

/// Config: the registry name to register the provider under.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Provider name on `ctx.subagents` (default `fork`).
    pub provider_name: String,
}

/// The balanced completed-turn prefix of `parent`'s log: every event up to
/// and including the last `turn/end`.
pub fn completed_turn_prefix(parent: &dyn Agent) -> Vec<SessionEvent> {
    let events = parent.session().events();
    let Some(last_end) = events.iter().rposition(|event| event.type_ == "turn/end") else {
        return Vec::new();
    };
    events[..=last_end].to_vec()
}

/// The fork provider.
struct ForkInProcessProvider {
    name: String,
}

#[async_trait::async_trait]
impl SubagentProvider for ForkInProcessProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities {
            output_schema: false, // structured capture not ported yet
            depth_limit: true,
            tool_filter: true,
            persona: true,
        }
    }

    fn inherits_parent_context(&self) -> bool {
        true
    }

    async fn start(
        &self,
        request: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        let seed = completed_turn_prefix(request.request.parent.as_ref());
        start_in_process_run(
            &request,
            InProcessRunOptions {
                seed: (!seed.is_empty()).then_some(seed),
            },
        )
        .await
    }

    async fn prepare_continuable(
        &self,
        request: ContinuableCreateRequest,
    ) -> Result<ContinuableCreateSpec, SubagentError> {
        // The fork prefix is captured ONCE, at creation.
        let seed = completed_turn_prefix(request.parent.as_ref());
        Ok(ContinuableCreateSpec {
            seed: (!seed.is_empty()).then_some(seed),
        })
    }
}

/// Install Schedule-like function apply: register the fork provider (TS
/// `apply`).
pub fn apply(ctx: &Context, config: &Config) -> Result<(), SubagentError> {
    let runtime = ctx
        .get_typed::<Arc<dsh_subagent::SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| SubagentError::new("NO_PROVIDER", "subagents service is not mounted"))?;
    let provider_name = if config.provider_name.is_empty() {
        "fork".to_string()
    } else {
        config.provider_name.clone()
    };
    let provider: Arc<dyn SubagentProvider> = Arc::new(ForkInProcessProvider {
        name: provider_name,
    });
    runtime
        .register_provider(ctx, provider)
        .map(|_| ())
}

/// The Cordis plugin form.
pub struct ForkInProcessPlugin;

#[async_trait::async_trait]
impl Plugin for ForkInProcessPlugin {
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
