//! Package-owned invariant companion. Rust port of
//! `packages/session/session-projection/src/invariant.ts`.
//!
//! No runtime invariant: the registry's own contracts (duplicate-key and
//! stateVersion rejection, effect-tied removal, the reference-identity
//! change gate) are enforced synchronously inside the service and proven by
//! its spec.

use std::sync::Arc;

use cordis::{BoxFuture, Context, Disposer};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-session-projection";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "session-projection-invariant";

/// Services required before the companion can register (TS `inject`).
pub const INJECT: [&str; 1] = ["invariants"];

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> BoxFuture<'static, Disposer> {
    let ctx = ctx.clone();
    Box::pin(async move {
        let invariants = ctx
            .get_typed::<Arc<InvariantRegistry>>("invariants", false)
            .expect("invariants service required by session-projection-invariant");
        invariants.register(
            &ctx,
            PACKAGE_NAME,
            InvariantInstaller {
                install: Arc::new(|_ctx: &Context, _fail: Arc<dyn Fn(&str) + Send + Sync>| {
                    Box::pin(async move {})
                }),
                inject: None,
            },
        )
    })
}
