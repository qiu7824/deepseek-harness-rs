//! Package-owned invariant companion for `@deepseek-ai/dsh-host-frontend-static`.
//! No runtime invariant: the only owned relation is the single fallback seat,
//! and its register/release symmetry is covered by the real-composition test.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-host-frontend-static";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "host-frontend-static-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Build the no-op installer.
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
        .expect("the frontend-static invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct FrontendStaticInvariantPlugin;

#[async_trait]
impl Plugin for FrontendStaticInvariantPlugin {
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
