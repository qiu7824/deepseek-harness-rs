//! Rust port of `packages/core/agent-default-model/tests/agent-default-model.spec.ts`:
//! default Agent model settings layered over a real settings provider.

use std::sync::Arc;

use cordis::Context;
use dsh_agent_default_model::{AgentDefaultModelConfig, AgentDefaultModelConfigService};
use dsh_settings::{SettingsNamespace, SettingsProvider, SettingsStorage};
use indexmap::IndexMap;
use parking_lot::Mutex;
use schemastery::Data;

struct MemorySettings {
    doc: Mutex<IndexMap<String, Data>>,
}

#[async_trait::async_trait]
impl SettingsStorage for MemorySettings {
    fn writable(&self) -> bool {
        true
    }

    async fn load(&self) -> Result<IndexMap<String, Data>, String> {
        Ok(self.doc.lock().clone())
    }

    async fn persist(&self, ns: &SettingsNamespace, section: Data) -> Result<(), String> {
        self.doc.lock().insert(ns.as_str().to_string(), section);
        Ok(())
    }
}

/// A settings provider mounted on its own fiber so tests can detach it
/// (TS `boot()` returns `settingsFiber` for exactly this).
struct SettingsPlugin {
    storage: Arc<MemorySettings>,
}

#[async_trait::async_trait]
impl cordis::Plugin for SettingsPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("memory-settings")
    }

    async fn apply(&self, ctx: &Context, _config: cordis::ArcValue) -> Result<(), cordis::PluginError> {
        let provider = SettingsProvider::install(ctx, self.storage.clone());
        let _ = provider;
        Ok(())
    }
}

async fn boot() -> (
    Context,
    Arc<cordis::FiberCore>,
    Arc<AgentDefaultModelConfigService>,
) {
    let ctx = Context::root();
    let storage = Arc::new(MemorySettings {
        doc: Mutex::new(IndexMap::new()),
    });
    let settings_fiber = ctx.plugin(
        Arc::new(SettingsPlugin { storage }),
        cordis::arc(serde_json::Value::Null),
    );
    settings_fiber.settle().await.unwrap();
    let default_model = AgentDefaultModelConfigService::install(
        &ctx,
        AgentDefaultModelConfig {
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4-flash".to_string(),
        },
    );
    default_model.ready().await.unwrap();
    (ctx, settings_fiber, default_model)
}

fn selection(service: &Arc<AgentDefaultModelConfigService>) -> (String, String, Option<String>) {
    let value = service.current_selection();
    (
        value.provider,
        value.model,
        value.reasoning_effort.map(|effort| effort.as_str().to_string()),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn resolves_the_user_layer_over_the_composition_entry() {
    let (ctx, _settings_fiber, default_model) = boot().await;
    assert_eq!(
        selection(&default_model),
        ("deepseek-official".to_string(), "deepseek-v4-flash".to_string(), None)
    );

    let mut next = default_model.current_selection();
    next.provider = "acme-gateway".to_string();
    next.model = "acme-large".to_string();
    next.reasoning_effort = Some(dsh_llm::reasoning_effort_id("high"));
    default_model.save_selection(next).await.unwrap();
    assert_eq!(
        selection(&default_model),
        ("acme-gateway".to_string(), "acme-large".to_string(), Some("high".to_string()))
    );
    let _ = ctx;
}

#[tokio::test(flavor = "multi_thread")]
async fn clears_a_stored_effort_when_the_saved_selection_has_none() {
    let (_, _, default_model) = boot().await;
    let mut with_effort = default_model.current_selection();
    with_effort.provider = "acme-gateway".to_string();
    with_effort.model = "acme-large".to_string();
    with_effort.reasoning_effort = Some(dsh_llm::reasoning_effort_id("high"));
    default_model.save_selection(with_effort).await.unwrap();
    let mut plain = default_model.current_selection();
    plain.provider = "acme-gateway".to_string();
    plain.model = "acme-plain".to_string();
    plain.reasoning_effort = None;
    default_model.save_selection(plain).await.unwrap();
    assert_eq!(
        selection(&default_model),
        ("acme-gateway".to_string(), "acme-plain".to_string(), None)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn layers_a_hand_written_partial_section_over_the_entry() {
    let (ctx, _settings_fiber, default_model) = boot().await;
    let settings: Arc<Arc<SettingsProvider>> = ctx
        .get_typed::<Arc<SettingsProvider>>("settings", false)
        .expect("settings service");
    settings
        .replace(
            &dsh_agent_default_model::agent_default_model_settings_namespace(),
            serde_json::json!({"model": "deepseek-reasoner"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        selection(&default_model),
        ("deepseek-official".to_string(), "deepseek-reasoner".to_string(), None)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn falls_back_to_the_entry_when_the_settings_provider_detaches() {
    let (ctx, settings_fiber, default_model) = boot().await;
    let mut next = default_model.current_selection();
    next.provider = "acme-gateway".to_string();
    next.model = "acme-large".to_string();
    default_model.save_selection(next).await.unwrap();
    assert_eq!(selection(&default_model).0, "acme-gateway");

    // Dispose the settings fiber: the consumer falls back to its
    // composition entry (installSettingsSection's detach disposer).
    settings_fiber.dispose().await;
    assert_eq!(
        selection(&default_model),
        ("deepseek-official".to_string(), "deepseek-v4-flash".to_string(), None)
    );
    let _ = ctx;
}

#[tokio::test(flavor = "multi_thread")]
async fn keeps_the_composition_entry_when_no_settings_provider_is_mounted() {
    let ctx = Context::root();
    let default_model = AgentDefaultModelConfigService::install(
        &ctx,
        AgentDefaultModelConfig {
            provider: "p".to_string(),
            model: "m".to_string(),
        },
    );
    let mut next = default_model.current_selection();
    next.provider = "other".to_string();
    next.model = "other".to_string();
    default_model.save_selection(next).await.unwrap();
    // No settings service: saveSelection is a no-op and the entry stands.
    assert_eq!(
        selection(&default_model),
        ("p".to_string(), "m".to_string(), None)
    );
}
