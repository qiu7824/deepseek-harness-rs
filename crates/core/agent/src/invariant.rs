//! Package-owned agent lifecycle invariants. Rust port of
//! `packages/core/agent/src/invariant.ts`.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use cordis::{ArcValue, BoxFuture, Context, Disposer, EventOptions, Listener, downcast};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use parking_lot::Mutex;

use crate::runtime_types::{Agent, AgentStatus, AgentStatusPayload};

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-agent";

/// Cordis companion plugin name.
pub const NAME: &str = "agent-invariant";

/// Services required before the companion can register.
pub const INJECT: [&str; 1] = ["invariants"];

/// Register the agent invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> BoxFuture<'static, Disposer> {
    let ctx = ctx.clone();
    Box::pin(async move {
        let invariants = ctx
            .get_typed::<Arc<InvariantRegistry>>("invariants", false)
            .expect("invariants service required by agent-invariant");
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
    // Status per live agent; weak identity guards recycled handles (the TS
    // `WeakMap` contract).
    let last_status: Arc<Mutex<HashMap<usize, (Weak<dyn Agent>, AgentStatus)>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
        let fail = Arc::clone(&fail);
        let last_status = Arc::clone(&last_status);
        Box::pin(async move {
            let Some(payload) = args
                .first()
                .and_then(|value| downcast::<AgentStatusPayload>(value).cloned())
            else {
                return None;
            };
            let agent = &payload.agent;
            let identity = Arc::as_ptr(&payload.agent) as *const () as usize;
            let mut table = last_status.lock();
            let repeated = match table.get(&identity) {
                Some((weak, previous)) => {
                    if weak.strong_count() == 0 {
                        table.remove(&identity);
                        false
                    } else if *previous == payload.status {
                        true
                    } else {
                        false
                    }
                }
                None => false,
            };
            if repeated {
                fail(&format!(
                    "agent/status repeated {} (no-op transition)",
                    payload.status.as_str()
                ));
            }
            let weak: Weak<dyn Agent> = Arc::downgrade(&payload.agent);
            table.insert(identity, (weak, payload.status));
            let _ = agent;
            None
        })
    });
    ctx.on(
        "agent/status",
        listener,
        EventOptions::default().global(true),
    )
    .await;
}
