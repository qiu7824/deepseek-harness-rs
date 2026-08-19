//! Package-owned invariant companion for the settings seam. Rust port of
//! `packages/settings/settings/src/invariant.ts`.

use std::sync::Arc;

use cordis::{ArcValue, BoxFuture, Context, Disposer, EventOptions, Listener, downcast};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use schemastery::Data;

use crate::index::{SettingsProvider, deep_equal_json};
use crate::types::SETTINGS_UPDATED;

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-settings";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "settings-invariant";

/// Services required before the companion can register (TS `inject`).
pub const INJECT: [&str; 1] = ["invariants"];

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> BoxFuture<'static, Disposer> {
    let ctx = ctx.clone();
    Box::pin(async move {
        let invariants = ctx
            .get_typed::<Arc<InvariantRegistry>>("invariants", false)
            .expect("invariants service required by settings-invariant");
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
    // The commit-event contract: `settings/updated` fires only for a
    // currently registered namespace, only when the resolved value changed,
    // and only with the service's authoritative resolved value.
    let listener: Arc<Listener> = Arc::new(move |listener_ctx: &Context, args: Vec<ArcValue>| {
        let fail = Arc::clone(&fail);
        let listener_ctx = listener_ctx.clone();
        Box::pin(async move {
            let ns = downcast::<crate::index::SettingsNamespace>(&args[0])
                .cloned()
                .expect("settings/updated namespace arg");
            let next = downcast::<Data>(&args[1])
                .cloned()
                .expect("settings/updated next arg");
            let prev = downcast::<Data>(&args[2])
                .cloned()
                .expect("settings/updated prev arg");
            let settings = listener_ctx
                .get_typed::<Arc<SettingsProvider>>("settings", false)
                .expect("settings service");
            let current = settings.get(&ns);
            if current.is_none() {
                fail(&format!(
                    "settings/updated for \"{}\" emitted while the namespace is unregistered",
                    ns.as_str()
                ));
                return None;
            }
            let current = current.expect("checked");
            if !deep_equal_json(&current, &next) {
                fail(&format!(
                    "settings/updated for \"{}\" does not match the authoritative resolved value",
                    ns.as_str()
                ));
            }
            if deep_equal_json(&next, &prev) {
                fail(&format!(
                    "settings/updated for \"{}\" emitted without a resolved-value change",
                    ns.as_str()
                ));
            }
            None
        })
    });
    let _ =
        futures::executor::block_on(ctx.on(SETTINGS_UPDATED, listener, EventOptions::default()));
}
