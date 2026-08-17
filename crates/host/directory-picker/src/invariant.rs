//! Package-owned invariant companion for the directory-picker seam.
//! Rust port of `packages/host/directory-picker/src/invariant.ts`.
//!
//! No runtime invariant: this stateless Service Definition owns the
//! capability vocabulary, while backends and the RPC consumer own
//! observations.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-host-directory-picker";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "host-directory-picker-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Build the no-op installer (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: None,
        install: Arc::new(
            |_ctx: &Context, _fail: Arc<dyn Fn(&str) + Send + Sync>| Box::pin(async {}),
        ),
    }
}

/// Register the directory-picker invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the directory-picker invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct DirectoryPickerInvariantPlugin;

#[async_trait]
impl Plugin for DirectoryPickerInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(_ctx);
        Ok(())
    }
}
