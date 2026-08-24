//! Mount one preset composition under an agent's scope context, then prove
//! the result is usable before the agent is published.
//! Rust port of `src/mount.ts`.
//!
//! Two guards make the mount safe. A row that never reached a usable state
//! is rejected, because a directly-plugged subtree is absent from
//! `ctx.loader.entries()` and no boot audit covers it. A row that published
//! a service into the ROOT realm is rejected, because such a service is
//! process-global rather than per-session.
//!
//! # Deviations
//!
//! - The TS `PresetTree` subclass of `Include` collapses to the plain Rust
//!   [`Include`] marked readonly with a no-op write backend — the same two
//!   behaviors the subclass overrode.
//! - Bare module specifiers resolve through the Rust loader's static
//!   registry (see `dsh-cordis-loader`), so a preset row's `name` is a
//!   registry key rather than a filesystem-relative specifier.
//! - `pruneDisposedMounts` keys on `FiberState::Disposed` (TS `uid === null`).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cordis::{ArcValue, Context, FiberCore, FiberState, Plugin, PluginError, arc};
use dsh_cordis_include::include::{Include, IncludeConfig};
use dsh_cordis_loader::{EntryTree, LoaderService};
use dsh_scope::{ScopeKey, scope_of, scope_parent_of};
use parking_lot::Mutex;

use crate::preset::{AgentPreset, PresetMountError};

/// One preset composition currently installed under some agent.
#[derive(Clone)]
pub struct PresetMount {
    /// The preset the subtree was composed from.
    pub preset_id: String,
    /// The mounted subtree's fiber.
    pub fiber: Arc<FiberCore>,
    /// The standing scope key agents are parented to (`None` only in
    /// torn-down records).
    pub key: Option<ScopeKey>,
}

/// A live standing mount located through one agent already joined to it.
pub struct JoinedPresetMount {
    pub preset_id: String,
    pub fiber: Arc<FiberCore>,
    /// The standing key, definite because it is what the lookup matched on.
    pub key: ScopeKey,
}

static MOUNTS: Mutex<Vec<PresetMount>> = Mutex::new(Vec::new());

/// Drop every record whose subtree is gone (TS `pruneDisposedMounts`).
/// Records are pruned by observation rather than through a disposal hook
/// because a subtree can be torn down by its owning agent, by a failed
/// mount, or by the whole tree unloading.
fn prune_disposed_mounts() {
    MOUNTS
        .lock()
        .retain(|mount| mount.fiber.state() != FiberState::Disposed);
}

/// Every preset composition still installed, pruning fibers disposed since
/// the last read (TS `livePresetMounts`).
pub fn live_preset_mounts() -> Vec<PresetMount> {
    prune_disposed_mounts();
    MOUNTS.lock().clone()
}

/// Whether `fiber` is `root` itself or is mounted anywhere inside its
/// subtree. Membership is object identity (TS `withinFiber`).
fn within_fiber(fiber: &Arc<FiberCore>, root: &Arc<FiberCore>) -> bool {
    let mut current = Some(fiber.clone());
    while let Some(fiber) = current {
        if Arc::ptr_eq(&fiber, root) {
            return true;
        }
        let parent = fiber.parent.lock().as_ref().map(|ctx| ctx.fiber.clone());
        match parent {
            Some(parent) => {
                if Arc::ptr_eq(&parent, &fiber) {
                    return false;
                }
                current = Some(parent);
            }
            None => return false,
        }
    }
    false
}

/// Service names the mounted subtree published into the root realm
/// (TS `leakedServices`). A provider without an `isolate` realm stores its
/// implementation under the root's label for that name; a provider inside an
/// `isolate` realm stores under a realm-private label and is correctly
/// absent here.
pub fn leaked_services(ctx: &Context, mount: &Arc<FiberCore>) -> Vec<String> {
    let store = ctx.reflect.store.lock();
    let root_ctx = ctx.root_context();
    let mut leaked: Vec<String> = Vec::new();
    for (label, implementation) in store.iter() {
        if implementation.fiber.state() == FiberState::Disposed {
            continue;
        }
        if !within_fiber(&implementation.fiber, mount) {
            continue;
        }
        if let Some(root_label) = root_ctx.isolate_label(&implementation.name) {
            if root_label == *label {
                leaked.push(implementation.name.clone());
            }
        }
    }
    leaked.sort();
    leaked
}

/// The standing composition one agent is joined to (TS `standingMountFor`).
/// The agent's own key is parented to its preset's standing key, so the
/// mount is found by matching that parent rather than by walking up from the
/// agent. An agent that joined no preset has no parent link and resolves to
/// `None`.
pub fn standing_mount_for(agent_ctx: &Context) -> Option<JoinedPresetMount> {
    let agent_key = scope_of(agent_ctx)?;
    let standing_key = scope_parent_of(&agent_key)?;
    live_preset_mounts()
        .into_iter()
        .find(|candidate| {
            candidate
                .key
                .as_ref()
                .is_some_and(|key| *key == standing_key)
        })
        .map(|candidate| JoinedPresetMount {
            preset_id: candidate.preset_id,
            fiber: candidate.fiber,
            key: standing_key,
        })
}

/// One agent's instance of a service its preset mounted
/// (TS `serviceForAgent`). Read addressing for a caller that already holds
/// the agent: a preset publishes services behind `isolate` realms, which are
/// invisible outside the group that declares them — including to the host.
pub fn service_for_agent<T: Send + Sync + 'static>(
    ctx: &Context,
    agent_ctx: &Context,
    name: &str,
) -> Option<Arc<T>> {
    let mount = standing_mount_for(agent_ctx)?;
    let store = ctx.reflect.store.lock();
    for implementation in store.values() {
        if implementation.fiber.state() == FiberState::Disposed {
            continue;
        }
        if implementation.name != name {
            continue;
        }
        if within_fiber(&implementation.fiber, &mount.fiber) {
            if let Some(value) = &implementation.value {
                if let Some(typed) = cordis::downcast_arc::<T>(value) {
                    return Some(typed);
                }
            }
        }
    }
    None
}

/// Rows that did not reach a usable state, each rendered as one diagnostic
/// line (TS `inactiveRows`).
pub fn inactive_rows(tree: &Arc<EntryTree>) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for entry in tree.entries() {
        if entry.disabled().unwrap_or(false) {
            continue;
        }
        let fiber = entry.fiber.lock().clone();
        let options = entry.options.lock().clone();
        let Some(fiber) = fiber else {
            lines.push(format!("{} ({}): never started", options.id, options.name));
            continue;
        };
        let missing: Vec<&str> = fiber
            .inject
            .keys()
            .filter(|name| fiber.ctx().and_then(|ctx| ctx.get(name, true)).is_none())
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            lines.push(format!(
                "{} ({}): waiting for {}",
                options.id,
                options.name,
                missing.join(", ")
            ));
        }
    }
    lines
}

/// The reportable text of a mount failure, flattening one
/// `AggregateError`-style message into a single-line-per-cause description
/// (TS `mountDetail`).
fn mount_detail(error: &str) -> String {
    // The Rust loader reports several failed rows as one aggregate error;
    // split on newlines and indent the causes.
    let mut lines = error.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let causes: Vec<String> = lines.map(|line| format!("- {line}")).collect();
    if causes.is_empty() {
        first.to_string()
    } else {
        std::iter::once(first.to_string())
            .chain(causes)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The plugin wrapper that carries one preset subtree (TS `PresetTree`
/// extends `Include`). Its fiber IS the mounted subtree's fiber.
struct PresetTreePlugin {
    include: Mutex<Option<Arc<Include>>>,
}

#[async_trait::async_trait]
impl Plugin for PresetTreePlugin {
    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(["loader"])
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let loader = ctx
            .get_typed::<Arc<LoaderService>>("loader", true)
            .map(|double_arc| (*double_arc).clone())
            .ok_or_else(|| PluginError::new(arc("loader service is not available".to_string())))?;
        let config_value = cordis::downcast::<serde_json::Value>(&config)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let include_config: IncludeConfig =
            serde_json::from_value(config_value).map_err(|error| {
                PluginError::new(arc(format!("invalid preset include config: {error}")))
            })?;
        let include = Include::new(ctx.clone(), loader.core.clone(), include_config)
            .map_err(|error| PluginError::new(arc(error.to_string())))?;
        // A preset is an input, never a persistence target (TS `write()`
        // override): suppress the write-back so a self-disposing entry can
        // never truncate a shipped composition.
        include.readonly.store(true, Ordering::SeqCst);
        include.tree.set_write_backend(Arc::new(|| {}));
        include
            .init()
            .await
            .map_err(|error| PluginError::new(arc(error.to_string())))?;
        include
            .tree
            .await_ready()
            .await
            .map_err(|error| PluginError::new(arc(error.to_string())))?;
        *self.include.lock() = Some(include);
        Ok(())
    }
}

/// Mount `preset` under `agentCtx` and return only once every row is
/// usable (TS `mountPreset`). The subtree is owned by `agentCtx`'s fiber, so
/// it unwinds with the agent and the caller receives no disposer. A
/// rejection leaves nothing mounted.
pub async fn mount_preset(
    agent_ctx: &Context,
    preset: &AgentPreset,
) -> Result<(), PresetMountError> {
    let scope = scope_of(agent_ctx).ok_or_else(|| {
        PresetMountError::new(
            &preset.id,
            format!(
                "refusing to mount preset \"{}\" into an unscoped context; \
                 its registrations would apply to every agent in the process",
                preset.id
            ),
        )
    })?;
    // Prune records of torn-down runtimes before this mount adds one.
    prune_disposed_mounts();
    let plugin = Arc::new(PresetTreePlugin {
        include: Mutex::new(None),
    });
    let config = serde_json::json!({ "path": preset.path });
    let handle = agent_ctx.plugin(plugin.clone(), arc(config));
    let settled = handle.settle().await;
    let error_message = match settled {
        Ok(()) => {
            let Some(include) = plugin.include.lock().clone() else {
                let _ = handle.dispose().await;
                return Err(PresetMountError::new(
                    &preset.id,
                    "mounted subtree did not publish its entry tree",
                ));
            };
            let tree = include.tree.clone();
            let fiber = handle.clone();
            let unusable = inactive_rows(&tree);
            if !unusable.is_empty() {
                let _ = handle.dispose().await;
                return Err(PresetMountError::new(
                    &preset.id,
                    format!(
                        "{} row(s) did not activate:\n{}",
                        unusable.len(),
                        unusable.join("\n")
                    ),
                ));
            }
            let leaked = leaked_services(agent_ctx, &fiber);
            if !leaked.is_empty() {
                let _ = handle.dispose().await;
                return Err(PresetMountError::new(
                    &preset.id,
                    format!(
                        "row(s) published process-global service(s) [{}]; a preset \
                         service must sit behind an `isolate` realm or move to the \
                         host composition",
                        leaked.join(", ")
                    ),
                ));
            }
            MOUNTS.lock().push(PresetMount {
                preset_id: preset.id.clone(),
                fiber,
                key: Some(scope),
            });
            return Ok(());
        }
        Err(error) => error.message(),
    };
    let _ = handle.dispose().await;
    Err(PresetMountError::new(
        &preset.id,
        format!("{} ({})", mount_detail(&error_message), preset.path),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis::{InjectSpec, Service};
    use dsh_cordis_loader::EntryOptions;

    struct TypedService;
    impl Service for TypedService {
        fn service_name(&self) -> &'static str {
            "typedService"
        }
    }

    struct NeedsTypedService;
    #[async_trait::async_trait]
    impl Plugin for NeedsTypedService {
        fn inject(&self) -> InjectSpec {
            InjectSpec::new(["typedService"])
        }

        async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn inactive_rows_accepts_any_live_typed_service() {
        let ctx = Context::root();
        ctx.register_service(Arc::new(TypedService));
        let loader = LoaderService::new(&ctx).await;
        loader
            .core
            .register("needs-typed", Arc::new(NeedsTypedService));
        loader
            .tree
            .create(
                EntryOptions {
                    id: "probe".to_string(),
                    name: "needs-typed".to_string(),
                    ..EntryOptions::default()
                },
                None,
                None,
            )
            .await
            .expect("create typed-service consumer");
        loader.tree.await_ready().await.expect("consumer ready");
        assert!(inactive_rows(&loader.tree).is_empty());
    }
}
