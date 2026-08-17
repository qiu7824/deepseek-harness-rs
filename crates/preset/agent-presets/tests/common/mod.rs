//! Shared fixtures for the agent-presets integration tests: a cordis
//! runtime with the loader mounted, the TS fixture plugins registered under
//! their registry keys, and temp-directory helpers.
//!
//! The TS fixtures resolve rows through Node's ESM loader against relative
//! specifiers; the Rust loader resolves through its static registry, so the
//! composition files below name the registry keys directly.

use std::sync::Arc;

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_cordis_loader::LoaderService;
use dsh_scope::{CreateScopeOptions, Scope, ScopeKey, create_scope};

/// Process-unique counter for temp dirs (parallel tests never collide).
pub fn fastrand() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A fresh temp directory.
pub fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsh-preset-{label}-{}-{}",
        std::process::id(),
        fastrand()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// The TS `contribute` fixture: a row that runs and registers nothing
/// (the TS version registers a tool and a prompt section; the Rust audit
/// only needs a usable row, so the registrations are left to the domain
/// crates' own tests).
pub struct ContributePlugin;

#[async_trait::async_trait]
impl Plugin for ContributePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("contribute")
    }

    async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        Ok(())
    }
}

/// The TS `needs-missing` fixture: waits forever for a service the
/// composition never supplies, which only the mount audit can catch.
pub struct NeedsMissingPlugin;

#[async_trait::async_trait]
impl Plugin for NeedsMissingPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("needs-missing")
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(["serviceThatDoesNotExist"])
    }

    async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        Ok(())
    }
}

/// The TS `global-service` fixture: publishes a service with no `isolate`
/// realm, so it lands in the ROOT realm.
pub struct GlobalServicePlugin;

#[async_trait::async_trait]
impl Plugin for GlobalServicePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("global-service")
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = cordis::downcast::<serde_json::Value>(&config)
            .cloned()
            .unwrap_or_default();
        let service = config
            .get("service")
            .and_then(|value| value.as_str())
            .unwrap_or("fixtureService")
            .to_string();
        let label = config
            .get("label")
            .and_then(|value| value.as_str())
            .unwrap_or("FIXTURE")
            .to_string();
        ctx.reflect
            .provide(&ctx.clone(), &service, Some(cordis::arc(label)), None);
        Ok(())
    }
}

/// The TS `self-dispose` fixture: disposes itself once active. The loader
/// treats a self-disposing entry as a config change and writes the tree
/// back, which is the exact path that once truncated a preset file to `[]`.
pub struct SelfDisposePlugin;

#[async_trait::async_trait]
impl Plugin for SelfDisposePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("self-dispose")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let fiber = ctx.fiber.clone();
        tokio::spawn(async move {
            fiber.dispose().await;
        });
        Ok(())
    }
}

/// Boot a cordis runtime with the loader service and the fixture plugins in
/// its static registry (TS `roster` harness).
pub async fn boot() -> Context {
    let ctx = Context::root();
    let fiber = ctx.plugin(dsh_cordis_loader::plugin(), cordis::arc(()));
    fiber.settle().await.expect("loader plugin loads");
    let service = ctx
        .get_typed::<Arc<LoaderService>>("loader", true)
        .expect("loader service")
        .as_ref()
        .clone();
    service
        .core
        .register("contribute", Arc::new(ContributePlugin));
    service
        .core
        .register("needs-missing", Arc::new(NeedsMissingPlugin));
    service
        .core
        .register("global-service", Arc::new(GlobalServicePlugin));
    service
        .core
        .register("self-dispose", Arc::new(SelfDisposePlugin));
    ctx
}

/// Mint a scoped context like the agent factory's setup does (TS
/// `createScope`).
pub fn scoped(ctx: &Context) -> (Scope, ScopeKey) {
    let key = ScopeKey::new();
    let scope = create_scope(ctx, key.clone(), &CreateScopeOptions::default());
    (scope, key)
}

/// Write one preset directory (composition + optional metadata) and return
/// the [`dsh_agent_presets::AgentPreset`] discovery would produce for it.
pub async fn seed_preset(
    root: &std::path::Path,
    id: &str,
    composition: &str,
) -> dsh_agent_presets::AgentPreset {
    let dir = root.join(id);
    tokio::fs::create_dir_all(&dir)
        .await
        .expect("create preset dir");
    tokio::fs::write(dir.join(dsh_agent_presets::COMPOSITION_FILE), composition)
        .await
        .expect("write composition");
    dsh_agent_presets::AgentPreset {
        id: id.to_string(),
        trust: dsh_agent_presets::PresetTrust::User,
        path: dir
            .join(dsh_agent_presets::COMPOSITION_FILE)
            .to_string_lossy()
            .to_string(),
        name: None,
        description: None,
        order: None,
        broken: None,
    }
}
