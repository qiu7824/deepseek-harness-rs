//! Package-owned invariant companion. Rust port of
//! `packages/session/session-persistence-sqlite/src/invariant.ts`.
//!
//! No runtime invariant: persistence correctness requires backend
//! round-trip and crash-tail tests; this package exposes no continuously
//! observable in-process relation.

use std::sync::Arc;

use cordis::{BoxFuture, Context, Disposer};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-session-persistence-sqlite";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "session-persistence-sqlite-invariant";

/// Services required before the companion can register (TS `inject`).
pub const INJECT: [&str; 1] = ["invariants"];

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> BoxFuture<'static, Disposer> {
    let ctx = ctx.clone();
    Box::pin(async move {
        let invariants = ctx
            .get_typed::<Arc<InvariantRegistry>>("invariants", false)
            .expect("invariants service required by session-persistence-sqlite-invariant");
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
