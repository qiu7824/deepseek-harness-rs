//! Package-owned invariant companion for `@deepseek-ai/dsh-host-webserver`.
//! On every plugin teardown it probes that route disposers really removed
//! their registrations: a stale route would keep serving a disposed plugin's
//! handler.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::{ArcValue, Context, EventOptions, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

use crate::index::{WebHandlerError, WebRoute, WebRouteKind, WebServer, WebUpgradeRoute};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-host-webserver";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "host-webserver-invariant";

/// Service required before the companion can register.
pub const INJECT: [&str; 1] = ["invariants"];

fn route_probe(server: &Arc<WebServer>) -> Result<(), ()> {
    let probe = WebRoute {
        kind: WebRouteKind::Exact,
        path: "/__dsh_invariant_probe__".to_string(),
        handler: Arc::new(|_request| {
            Box::pin(async {
                Err(WebHandlerError::new(
                    "webserver invariant probe handler should never run",
                ))
            })
        }),
    };
    // Each register+dispose cycle must leave the table clean. A leftover from
    // the first cycle makes the second register panic.
    (server.register(probe.clone()))();
    (server.register(probe))();

    let upgrade_probe = WebUpgradeRoute {
        path: "/__dsh_invariant_upgrade_probe__".to_string(),
        handler: Arc::new(|_request, _socket| Box::pin(async { Ok(()) })),
    };
    (server.register_upgrade(upgrade_probe.clone()))();
    (server.register_upgrade(upgrade_probe))();
    Ok(())
}

/// Build the route-table symmetry installer (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: None,
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let _ = ctx
                        .on(
                            "internal/plugin",
                            Arc::new(move |ctx: &Context, _args: Vec<ArcValue>| {
                                let slot = ctx.get_typed::<Arc<WebServer>>(
                                    crate::index::SERVICE_NAME,
                                    false,
                                );
                                let fail = fail.clone();
                                Box::pin(async move {
                                    if let Some(slot) = slot {
                                        let server = slot.as_ref().clone();
                                        let outcome = std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| {
                                                route_probe(&server)
                                            }),
                                        );
                                        match outcome {
                                            Ok(Ok(())) => {}
                                            Ok(Err(())) | Err(_) => {
                                                fail("webServer route disposer left a route registered — route tables and fiber lifecycles diverged");
                                            }
                                        }
                                    }
                                    None
                                })
                            }),
                            EventOptions::default().global(true),
                        )
                        .await;
            })
        }),
    }
}

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the webserver invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct WebServerInvariantPlugin;

#[async_trait]
impl Plugin for WebServerInvariantPlugin {
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
