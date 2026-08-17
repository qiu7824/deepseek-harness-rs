//! Package-owned invariant companion for `@deepseek-ai/dsh-fs-e2b`. Rust
//! port of `packages/e2b/fs-e2b/src/invariant.ts`.
//!
//! No runtime invariant: the backend owns no independent durable state;
//! remote state relations stay with the E2B sandbox.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-fs-e2b";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "fs-e2b-invariant";

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
        .expect("the fs-e2b invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct FsE2bInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for FsE2bInvariantPlugin {
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
