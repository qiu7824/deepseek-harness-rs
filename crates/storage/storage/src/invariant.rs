//! Package-owned invariant companion for `@deepseek-ai/dsh-storage`: a
//! no-runtime-check registration (the hub is a pure registration table
//! whose consistency is fully enforced at the call sites; it owns no event
//! stream or mutable medium to cross-check).

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

use crate::{INVARIANT_INJECT, INVARIANT_NAME, PACKAGE_NAME};

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
        .expect("the storage invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct StorageInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for StorageInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(INVARIANT_NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INVARIANT_INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
