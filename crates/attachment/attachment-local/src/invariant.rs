//! Package-owned invariant companion for
//! `@deepseek-ai/dsh-attachment-local`. Rust port of
//! `packages/attachment/attachment-local/src/invariant.ts`.
//!
//! No runtime invariant: immutable writes and verified reads are enforced
//! directly at the backend boundary.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-attachment-local";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "attachment-local-invariant";

/// Services required before package ownership can be reserved.
pub const INJECT: [&str; 2] = ["invariants", "attachments"];

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
        .expect("the attachment-local invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct AttachmentLocalInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for AttachmentLocalInvariantPlugin {
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
