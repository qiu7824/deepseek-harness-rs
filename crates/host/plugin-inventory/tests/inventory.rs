//! Rust port of `packages/host/plugin-inventory/tests/inventory.spec.ts`:
//! current non-group Loader entries projected without a second cache, in
//! Loader order.
//!
//! # Deviations
//!
//! - The typert remote-method publication assertion (`remoteMethods`) is
//!   deferred with the typert/projection integration milestone; the gateway
//!   is exercised as the `pluginInventory` service directly.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_cordis_loader::{EntryOptions, LoaderService};
use dsh_host_plugin_inventory::{
    PluginFiberPhase, PluginInventoryGateway,
};

struct ActivePlugin;

#[async_trait]
impl Plugin for ActivePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("cordis:active")
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new([])
    }

    async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        Ok(())
    }
}

struct PendingPlugin;

#[async_trait]
impl Plugin for PendingPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("cordis:pending")
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["neverReady"])
    }

    async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        Ok(())
    }
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

#[test]
fn projects_current_non_group_loader_entries_without_a_second_cache() {
    run(async {
        let ctx = Context::root();
        let loader = LoaderService::new(&ctx).await;
        ctx.register_service(loader.clone());
        // TS `loader.builtins.active = ...` registers under the bare key;
        // `import` strips the `cordis:` prefix before the lookup.
        loader.core.register("active", Arc::new(ActivePlugin));
        loader.core.register("pending", Arc::new(PendingPlugin));
        let gateway = PluginInventoryGateway::install(&ctx).expect("gateway");

        let active_id = loader
            .tree
            .create(
                EntryOptions {
                    name: "cordis:active".to_string(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .expect("create active");
        let pending_id = loader
            .tree
            .create(
                EntryOptions {
                    name: "cordis:pending".to_string(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .expect("create pending");
        let disabled_id = loader
            .tree
            .create(
                EntryOptions {
                    name: "cordis:not-installed".to_string(),
                    disabled: Some(serde_json::Value::Bool(true)),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .expect("create disabled");
        loader
            .tree
            .create(
                EntryOptions {
                    name: "cordis:active".to_string(),
                    group: Some(true),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .expect("create group");

        let snapshot = gateway.list();
        assert_eq!(snapshot.entries.len(), 3, "group entry is skipped");
        let find = |entry_id: &str| {
            snapshot
                .entries
                .iter()
                .find(|entry| entry.entry_id.as_str() == entry_id)
                .expect("entry present")
        };

        let active = find(&active_id);
        assert_eq!(active.module_name, "cordis:active");
        assert!(active.enabled);
        assert_eq!(active.fiber_phase, Some(PluginFiberPhase::Active));

        let pending = find(&pending_id);
        assert_eq!(pending.module_name, "cordis:pending");
        assert!(pending.enabled);
        assert_eq!(pending.fiber_phase, Some(PluginFiberPhase::Pending));

        let disabled = find(&disabled_id);
        assert_eq!(disabled.module_name, "cordis:not-installed");
        assert!(!disabled.enabled);
        assert_eq!(disabled.fiber_phase, None);

        // Disabling a live entry disposes its fiber: enabled false, phase
        // null.
        let mut patch = indexmap::IndexMap::new();
        patch.insert(
            "disabled".to_string(),
            serde_json::Value::Bool(true),
        );
        loader
            .tree
            .resolve(&active_id)
            .expect("resolve active")
            .update(patch, false)
            .await
            .expect("update active");

        let snapshot = gateway.list();
        let active = snapshot
            .entries
            .iter()
            .find(|entry| entry.entry_id.as_str() == active_id)
            .expect("entry present");
        assert!(!active.enabled);
        assert_eq!(active.fiber_phase, None);

        // Removing an entry drops it from the projection.
        loader
            .tree
            .remove(&pending_id)
            .await
            .expect("remove pending");
        let snapshot = gateway.list();
        assert!(
            snapshot
                .entries
                .iter()
                .all(|entry| entry.entry_id.as_str() != pending_id),
            "removed entry leaves the inventory"
        );
    });
}
