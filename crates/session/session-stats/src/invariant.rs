//! Package-owned invariant companion. Rust port of
//! `packages/session/session-stats/src/invariant.ts`.
//!
//! No runtime invariant: the package owns a single pure projection fold
//! whose wire payload is schema-validated by the projection registry at
//! every snapshot and change-feed emission.

use std::sync::Arc;

use cordis::{BoxFuture, Context, Disposer};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-session-stats";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "session-stats-invariant";

/// Services required before the companion can register (TS `inject`).
pub const INJECT: [&str; 1] = ["invariants"];

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> BoxFuture<'static, Disposer> {
    let ctx = ctx.clone();
    Box::pin(async move {
        let invariants = ctx
            .get_typed::<Arc<InvariantRegistry>>("invariants", false)
            .expect("invariants service required by session-stats-invariant");
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
