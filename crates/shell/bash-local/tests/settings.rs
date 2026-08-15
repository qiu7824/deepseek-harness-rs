//! Rust port of the TS `settings.spec.ts` suite for `dsh-bash-local`: the
//! `bash` settings section layered over the executor's composition entry —
//! user-layer resolution, write validation, stored-section reads, provider
//! detach fallback, provider-less entry, and namespace release on unload.

use std::sync::Arc;

use cordis::{Context, FiberCore, Plugin};
use dsh_bash_local::{Config, LocalBashExecutor};
use dsh_schemastery::Data;
use dsh_settings::{SettingsNamespace, SettingsProvider, SettingsStorage};
use dsh_shell::ShellExecutor;
use dsh_subprocess_local::LocalSubprocessRuntime;
use indexmap::IndexMap;
use parking_lot::Mutex;

fn ns() -> SettingsNamespace {
    dsh_shell::shell_settings_namespace().clone()
}

/// In-memory provider fixture (TS `MemorySettings`).
struct MemorySettings {
    doc: Mutex<IndexMap<String, Data>>,
    writable_flag: bool,
}

#[async_trait::async_trait]
impl SettingsStorage for MemorySettings {
    fn writable(&self) -> bool {
        self.writable_flag
    }

    async fn load(&self) -> Result<IndexMap<String, Data>, String> {
        Ok(self.doc.lock().clone())
    }

    async fn persist(&self, ns: &SettingsNamespace, section: Data) -> Result<(), String> {
        self.doc.lock().insert(ns.as_str().to_string(), section);
        Ok(())
    }
}

/// The settings-provider plugin form (the TS `ctx.plugin(MemorySettings)`).
struct MemorySettingsPlugin {
    storage: Arc<MemorySettings>,
}

#[async_trait::async_trait]
impl Plugin for MemorySettingsPlugin {
    async fn apply(&self, ctx: &Context, _config: cordis::ArcValue) -> Result<(), cordis::PluginError> {
        let provider = SettingsProvider::install(ctx, self.storage.clone());
        provider
            .ready()
            .await
            .map_err(|error| cordis::PluginError::new(cordis::arc(error)))?;
        Ok(())
    }
}

/// The bash-executor plugin form with a composition entry. The installed
/// concrete handle rides a slot (the TS `ctx.shell as LocalBashExecutor`
/// cast: the seam registers the erased service, so the concrete handle comes
/// from the installer).
struct BashPlugin {
    config: Config,
    slot: Arc<Mutex<Option<Arc<LocalBashExecutor>>>>,
}

#[async_trait::async_trait]
impl Plugin for BashPlugin {
    async fn apply(&self, ctx: &Context, _config: cordis::ArcValue) -> Result<(), cordis::PluginError> {
        let executor = LocalBashExecutor::install(ctx, self.config.clone());
        executor.ready().await.map_err(|error| error)?;
        *self.slot.lock() = Some(executor);
        Ok(())
    }
}

struct Bench {
    ctx: Context,
    settings_fiber: Option<Arc<FiberCore>>,
    executor_fiber: Arc<FiberCore>,
    provider: Option<Arc<SettingsProvider>>,
    bash: Arc<LocalBashExecutor>,
}

async fn boot(config: Config, with_settings: bool) -> Bench {
    let ctx = Context::root();
    LocalSubprocessRuntime::install(&ctx);
    let mut provider = None;
    let settings_fiber = if with_settings {
        let storage = Arc::new(MemorySettings {
            doc: Mutex::new(IndexMap::new()),
            writable_flag: true,
        });
        let fiber = ctx.plugin(
            Arc::new(MemorySettingsPlugin { storage }),
            cordis::arc(()),
        );
        fiber.settle().await.expect("settings provider loads");
        provider = ctx
            .get_typed::<Arc<SettingsProvider>>("settings", false)
            .map(|slot| slot.as_ref().clone());
        Some(fiber)
    } else {
        None
    };
    let executor_slot = Arc::new(Mutex::new(None));
    let executor_fiber = ctx.plugin(
        Arc::new(BashPlugin {
            config,
            slot: executor_slot.clone(),
        }),
        cordis::arc(()),
    );
    executor_fiber.settle().await.expect("executor loads");
    let bash = executor_slot.lock().take().expect("executor installed");
    Bench {
        ctx,
        settings_fiber,
        executor_fiber,
        provider,
        bash,
    }
}

fn entry(timeout_ms: u64) -> Config {
    Config {
        timeout_ms: Some(timeout_ms),
        ..Default::default()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn resolves_the_user_layer_over_the_composition_entry() {
    let bench = boot(entry(60_000), true).await;
    assert_eq!(bench.bash.config().timeout_ms, 60_000);

    bench
        .provider
        .as_ref()
        .expect("provider")
        .update(&ns(), serde_json::json!({ "timeoutMs": 5_000 }), None)
        .await
        .expect("update");

    assert_eq!(bench.bash.config().timeout_ms, 5_000);
}

#[tokio::test(flavor = "current_thread")]
async fn refuses_a_stored_value_the_constructor_would_have_rejected() {
    let bench = boot(entry(60_000), true).await;
    let error = bench
        .provider
        .as_ref()
        .expect("provider")
        .update(&ns(), serde_json::json!({ "timeoutMs": 0 }), None)
        .await
        .err()
        .expect("rejected write");
    assert!(error.contains("positive finite"), "{error}");
    assert_eq!(bench.bash.config().timeout_ms, 60_000);
}

#[tokio::test(flavor = "current_thread")]
async fn refuses_a_grace_period_longer_than_a_timer_can_carry() {
    let bench = boot(entry(60_000), true).await;
    let error = bench
        .provider
        .as_ref()
        .expect("provider")
        .update(&ns(), serde_json::json!({ "graceMs": u64::MAX }), None)
        .await
        .err()
        .expect("rejected write");
    assert!(error.contains("graceMs must be no greater than"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn serves_the_stored_section_to_every_later_read() {
    let bench = boot(entry(60_000), true).await;
    bench
        .provider
        .as_ref()
        .expect("provider")
        .update(
            &ns(),
            serde_json::json!({ "maxOutputBytes": 1_024, "cwd": "/tmp" }),
            None,
        )
        .await
        .expect("update");

    let spec = bench.bash.resolve(dsh_shell::ShellExecRequest::new("true"));
    assert_eq!(spec.stdout_max_bytes, 1_024);
    assert_eq!(spec.workdir, "/tmp");
}

#[tokio::test(flavor = "current_thread")]
async fn falls_back_to_the_composition_entry_when_the_settings_provider_detaches() {
    let bench = boot(entry(60_000), true).await;
    bench
        .provider
        .as_ref()
        .expect("provider")
        .update(&ns(), serde_json::json!({ "timeoutMs": 5_000 }), None)
        .await
        .expect("update");
    assert_eq!(bench.bash.config().timeout_ms, 5_000);

    // Disposing the settings provider fiber unregisters the service; the
    // section wiring falls back to the composition entry.
    bench
        .settings_fiber
        .as_ref()
        .expect("settings fiber")
        .dispose()
        .await;
    tokio::task::yield_now().await;

    assert_eq!(bench.bash.config().timeout_ms, 60_000);
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_composition_entry_when_no_settings_provider_is_mounted() {
    let bench = boot(entry(1_234), false).await;
    assert_eq!(bench.bash.config().timeout_ms, 1_234);
    let _ = bench.ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn releases_the_namespace_when_the_executor_unloads() {
    let bench = boot(entry(60_000), true).await;
    let described: Vec<String> = bench
        .provider
        .as_ref()
        .expect("provider")
        .describe(dsh_settings::SettingsDescribeOptions::default())
        .into_iter()
        .map(|row| row.ns.as_str().to_string())
        .collect();
    assert!(described.contains(&"shell".to_string()), "{described:?}");

    bench.executor_fiber.dispose().await;
    tokio::task::yield_now().await;

    let described: Vec<String> = bench
        .provider
        .as_ref()
        .expect("provider")
        .describe(dsh_settings::SettingsDescribeOptions::default())
        .into_iter()
        .map(|row| row.ns.as_str().to_string())
        .collect();
    assert!(!described.contains(&"shell".to_string()), "{described:?}");
}
