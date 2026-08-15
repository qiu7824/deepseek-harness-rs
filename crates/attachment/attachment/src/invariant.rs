//! Package-owned invariant companion for `@deepseek-ai/dsh-attachment`.
//! Rust port of `packages/attachment/attachment/src/invariant.ts`.
//!
//! No runtime invariant: this stateless seam owns types while implementations
//! enforce immutable-store checks.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-attachment";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "attachment-invariant";

/// Service required before package ownership can be reserved.
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
        .expect("the attachment invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct AttachmentInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for AttachmentInvariantPlugin {
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
