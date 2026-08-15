//! Default model selection for an Agent without a session-specific
//! selection. Rust port of
//! `packages/core/agent-default-model/src/index.ts`.

use std::sync::Arc;

use cordis::{Context, Service};
use dsh_agent::model_selection::ModelSelection;
use dsh_settings::{SettingsNamespace, install_settings_section, settings_namespace};
use indexmap::IndexMap;
use parking_lot::Mutex;
use schemastery::{Data, Schema};

/// Settings namespace carrying the default model selection for future
/// Agents.
pub fn agent_default_model_settings_namespace() -> SettingsNamespace {
    settings_namespace("agent-default-model").expect("valid namespace")
}

/// Stored and composed default model selection.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentDefaultModelSettings {
    /// Registered provider route.
    pub provider: String,
    /// Provider-owned model id.
    pub model: String,
    /// Adapter-owned reasoning effort, or provider/default behavior when
    /// absent.
    pub reasoning_effort: Option<String>,
}

impl AgentDefaultModelSettings {
    fn from_data(value: &Data) -> Option<Self> {
        let Data::Object(object) = value else {
            return None;
        };
        let provider = match object.get("provider") {
            Some(Data::String(value)) => value.clone(),
            _ => return None,
        };
        let model = match object.get("model") {
            Some(Data::String(value)) => value.clone(),
            _ => return None,
        };
        let reasoning_effort = match object.get("reasoningEffort") {
            Some(Data::String(value)) => Some(value.clone()),
            _ => None,
        };
        Some(Self {
            provider,
            model,
            reasoning_effort,
        })
    }

    fn to_json(&self) -> serde_json::Value {
        let mut object = serde_json::Map::new();
        object.insert("provider".to_string(), serde_json::json!(self.provider));
        object.insert("model".to_string(), serde_json::json!(self.model));
        if let Some(effort) = &self.reasoning_effort {
            object.insert("reasoningEffort".to_string(), serde_json::json!(effort));
        }
        serde_json::Value::Object(object)
    }
}

/// Composition entry for the default model selection.
#[derive(Debug, Clone)]
pub struct AgentDefaultModelConfig {
    pub provider: String,
    pub model: String,
}

/// The schema of the default Agent model settings section.
pub fn agent_default_model_settings_schema() -> Schema {
    let mut properties = IndexMap::new();
    properties.insert("provider".to_string(), Schema::string().required(true));
    properties.insert("model".to_string(), Schema::string().required(true));
    properties.insert("reasoningEffort".to_string(), Schema::string());
    Schema::object(properties)
}

/// Project stored settings onto the Agent-facing selection type.
fn selection(settings: &AgentDefaultModelSettings) -> ModelSelection {
    ModelSelection {
        provider: settings.provider.clone(),
        model: settings.model.clone(),
        reasoning_effort: settings
            .reasoning_effort
            .as_deref()
            .map(dsh_llm::reasoning_effort_id),
    }
}

/// Owns the default model selection independently of any Host or transport.
/// The composition entry remains usable without a settings provider; when
/// one is mounted, its user layer is read live.
pub struct AgentDefaultModelConfigService {
    ctx: Context,
    source: Arc<Mutex<Arc<dyn Fn() -> AgentDefaultModelSettings + Send + Sync>>>,
    /// The settings-wiring inject fiber; `ready()` awaits its settle (TS
    /// plugin-load timing completes the wiring synchronously).
    wiring: Mutex<Option<Arc<cordis::FiberCore>>>,
}

impl Service for AgentDefaultModelConfigService {
    fn service_name(&self) -> &'static str {
        "agentDefaultModel"
    }
}

impl AgentDefaultModelConfigService {
    /// Create the service, register it as `ctx.agentDefaultModel`, and wire
    /// the optional settings section (TS constructor).
    pub fn install(ctx: &Context, config: AgentDefaultModelConfig) -> Arc<Self> {
        let entry = AgentDefaultModelSettings {
            provider: config.provider,
            model: config.model,
            reasoning_effort: None,
        };
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            source: Arc::new(Mutex::new(Arc::new(move || entry.clone()))),
            wiring: Mutex::new(None),
        });
        ctx.register_service(service.clone());
        let entry_for_section = service_source_entry(&service);
        let source_for_hook = Arc::clone(&service.source);
        let wiring = install_settings_section(
            ctx,
            agent_default_model_settings_namespace(),
            agent_default_model_settings_schema(),
            entry_for_section,
            dsh_settings::SettingsSectionHooks {
                set_source: Arc::new(move |source| {
                    // Every consumer reads through currentSelection(), so no
                    // registration-level fact needs rebuilding when the
                    // settings document changes. The seam hands out `Data`;
                    // adapt it back into the typed settings shape.
                    *source_for_hook.lock() = Arc::new(move || {
                        AgentDefaultModelSettings::from_data(&source()).expect(
                            "agent-default-model section must resolve to {provider, model}",
                        )
                    });
                }),
                on_change: Arc::new(|| {}),
                validate: None,
            },
        );
        *service.wiring.lock() = Some(wiring);
        service
    }

    /// Await the settings-section wiring (the inject fiber must settle
    /// before the first `saveSelection`/`replace` can find the
    /// registration).
    pub async fn ready(&self) -> Result<(), String> {
        let wiring = self
            .wiring
            .lock()
            .as_ref()
            .expect("wiring installed")
            .clone();
        wiring
            .settle()
            .await
            .map_err(|error| error.message())
    }

    /// Read the current default model selection (detached).
    pub fn current_selection(&self) -> ModelSelection {
        selection(&(self.source.lock())())
    }

    /// Save the complete default model selection. A deployment without a
    /// settings provider keeps its composition entry.
    pub async fn save_selection(&self, next: ModelSelection) -> Result<(), String> {
        let Some(settings) = self
            .ctx
            .get_typed::<Arc<dsh_settings::SettingsProvider>>("settings", false)
        else {
            return Ok(());
        };
        let mut section = serde_json::Map::new();
        section.insert("provider".to_string(), serde_json::json!(next.provider));
        section.insert("model".to_string(), serde_json::json!(next.model));
        if let Some(effort) = &next.reasoning_effort {
            section.insert("reasoningEffort".to_string(), serde_json::json!(effort.as_str()));
        }
        settings
            .replace(
                &agent_default_model_settings_namespace(),
                serde_json::Value::Object(section),
                None,
            )
            .await
    }
}

/// The entry source thunk used as the settings `base` layer.
fn service_source_entry(service: &Arc<AgentDefaultModelConfigService>) -> Data {
    let settings = (service.source.lock())();
    let json = settings.to_json();
    data_from_json(&json)
}

fn data_from_json(value: &serde_json::Value) -> Data {
    match value {
        serde_json::Value::Null => Data::Null,
        serde_json::Value::Bool(value) => Data::Bool(*value),
        serde_json::Value::Number(value) => Data::Number(value.as_f64().unwrap()),
        serde_json::Value::String(value) => Data::String(value.clone()),
        serde_json::Value::Array(array) => Data::Array(array.iter().map(data_from_json).collect()),
        serde_json::Value::Object(object) => {
            let mut entries = IndexMap::new();
            for (key, value) in object {
                entries.insert(key.clone(), data_from_json(value));
            }
            Data::Object(entries)
        }
    }
}

/// The Agent-facing read for tests and consumers.
pub fn selection_from_data(value: &Data) -> Option<ModelSelection> {
    AgentDefaultModelSettings::from_data(value).map(|settings| selection(&settings))
}
