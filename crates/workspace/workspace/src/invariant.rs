//! Package-owned invariant companion for `@deepseek-ai/dsh-workspace`: the
//! registry's entity cache mirrors the workspace domain's durable table.
//! Every `domain/changed` for the `workspaces` table must name a record the
//! cache already holds an entity for (the registry caches before the
//! durable put); a delete is valid only after the registry has removed the
//! entity.

use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
};
use dsh_invariants::InvariantRegistry;
use dsh_storage_domain::DomainChanged;

use crate::types::workspace_id;

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-workspace";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "workspace-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// The pure check over one change (exported for the unit spec).
pub fn check_change(
    change: &DomainChanged,
    registry_has: &dyn Fn(&str) -> bool,
    fail: &dyn Fn(&str),
) {
    match change {
        DomainChanged::Put {
            domain, table, key, ..
        }
        | DomainChanged::Deleted { domain, table, key } => {
            if domain != "workspace" || table != "workspaces" {
                return;
            }
            let operation = match change {
                DomainChanged::Put { .. } => "put",
                DomainChanged::Deleted { .. } => "deleted",
            };
            if operation == "deleted" {
                if registry_has(key) {
                    fail(&format!(
                        "workspace record '{key}' was deleted while the registry cache still \
                         publishes it — some write path bypassed ctx.workspaceRegistry"
                    ));
                }
                return;
            }
            if !registry_has(key) {
                fail(&format!(
                    "workspace record '{key}' landed durably but the registry cache holds \
                     no entity for it — the cache and the domain table have diverged"
                ));
            }
        }
    }
}

/// Build the installer registered under [`PACKAGE_NAME`].
pub fn installer() -> dsh_invariants::InvariantInstaller {
    dsh_invariants::InvariantInstaller {
        inject: Some(InjectSpec::new(["workspaceRegistry"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let registry = ctx
                    .get_typed::<Arc<crate::index::WorkspaceRegistry>>("workspaceRegistry", false)
                    .expect("workspaceRegistry service required");
                let listener_fail = fail.clone();
                let listener: Arc<Listener> =
                    Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
                        let change = args
                            .first()
                            .and_then(|value| downcast::<DomainChanged>(value))
                            .cloned();
                        let registry = registry.clone();
                        let fail = listener_fail.clone();
                        Box::pin(async move {
                            let change = change?;
                            check_change(
                                &change,
                                &|key| registry.get(&workspace_id(key)).is_some(),
                                &|message| fail(message),
                            );
                            None
                        })
                    });
                ctx.on(
                    "domain/changed",
                    listener,
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
        .expect("the workspace invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct WorkspaceInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for WorkspaceInvariantPlugin {
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
