//! Package-owned invariant companion for `@deepseek-ai/dsh-credentials`:
//! the commit-event lifecycle contract. `credentials/updated` names a
//! committed provider-source change, so it can only fire while a credentials
//! service is live — an emission after disposal means a provider leaked work
//! past its teardown quiescence. Rust port of
//! `packages/credentials/credentials/src/invariant.ts`.

use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast};

use dsh_invariants::{InvariantInstaller, InvariantRegistry};

use crate::types::CredentialRef;

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-credentials";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "credentials-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Build the installer (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: None,
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let fail_for_listener = fail.clone();
                let ctx_for_listener = ctx.clone();
                let listener: Arc<Listener> = Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
                    let reference = args
                        .first()
                        .and_then(|value| downcast::<CredentialRef>(value))
                        .cloned();
                    let fail = fail_for_listener.clone();
                    let ctx = ctx_for_listener.clone();
                    Box::pin(async move {
                        let reference = reference.expect("credentials/updated reference argument");
                        if ctx.get("credentials", false).is_none() {
                            fail(&format!(
                                "credentials/updated for \"{reference}\" emitted without a live credentials service"
                            ));
                        }
                        None
                    })
                });
                ctx.on("credentials/updated", listener, EventOptions::default()).await;
            })
        }),
    }
}

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the credentials invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct CredentialsInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for CredentialsInvariantPlugin {
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
