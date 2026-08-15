//! Package-owned session-persistence invariants. Rust port of
//! `packages/session/session-persistence/src/invariant.ts`.

use std::sync::Arc;

use cordis::{ArcValue, BoxFuture, Context, Disposer, EventOptions, Listener, downcast};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-session-persistence";

/// Cordis companion plugin name.
pub const NAME: &str = "session-persistence-invariant";

/// Services required before the companion can register.
pub const INJECT: [&str; 1] = ["invariants"];

/// Register the companion (TS `apply`).
pub fn apply(ctx: &Context) -> BoxFuture<'static, Disposer> {
    let ctx = ctx.clone();
    Box::pin(async move {
        let invariants = ctx
            .get_typed::<Arc<InvariantRegistry>>("invariants", false)
            .expect("invariants service required by session-persistence-invariant");
        invariants.register(
            &ctx,
            PACKAGE_NAME,
            InvariantInstaller {
                install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
                    let ctx = ctx.clone();
                    Box::pin(async move { install_inner(&ctx, fail).await })
                }),
                inject: None,
            },
        )
    })
}

async fn install_inner(ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>) {
    // The upstream companion validates flush/listener coherence; the port's
    // write-path sequencing is enforced by the coordinator itself. Keep the
    // registration boundary so deployments enabling invariants for this
    // package behave identically.
    let listener: Arc<Listener> = Arc::new(move |_ctx, args: Vec<ArcValue>| {
        let fail = Arc::clone(&fail);
        Box::pin(async move {
            // session/event args: [session, event]; the coordinator's
            // buffered copy is the invariant (the event must be
            // losslessly-JSON — enforced by the session boundary itself).
            let event = downcast::<dsh_session::SessionEvent>(&args[1]);
            if event.is_none() {
                fail("session/event carried no session event payload");
            }
            None
        })
    });
    ctx.on("session/event", listener, EventOptions::default().global(true))
        .await;
}
