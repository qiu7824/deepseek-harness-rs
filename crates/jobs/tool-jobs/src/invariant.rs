//! Package-owned invariant companion for `@deepseek-ai/dsh-tool-jobs`.
//! Rust port of `packages/jobs/tool-jobs/src/invariant.ts`.
//!
//! No runtime invariant: this model-facing adapter has no independent
//! lifecycle stream; execution relations are owned by the capability seam
//! it calls.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-tool-jobs";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "tool-jobs-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Build the no-op installer (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: None,
        install: Arc::new(|_ctx: &Context, _fail: Arc<dyn Fn(&str) + Send + Sync>| {
            Box::pin(async {})
        }),
    }
}

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the tool-jobs invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct ToolJobsInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for ToolJobsInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
