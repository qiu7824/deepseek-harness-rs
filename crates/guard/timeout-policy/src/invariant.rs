//! Package-owned invariant companion for
//! `@deepseek-ai/dsh-tool-call-timeout-policy`. Rust port of
//! `packages/guard/timeout-policy/src/invariant.ts`.
//!
//! No runtime invariant: this stateless policy plugin owns no package-local
//! event history or mutable data relation beyond the seam it intercepts (the
//! TS install is a no-op).

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-tool-call-timeout-policy";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "timeout-policy-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Build the no-op installer registered under [`PACKAGE_NAME`].
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
        .expect("the timeout-policy invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct TimeoutPolicyInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for TimeoutPolicyInvariantPlugin {
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
