//! The DeepSeek Harness Host boot spine (M6): compose the ported service
//! stack — sessions, agents, system prompt, tools, JSONL persistence,
//! SQLite FTS5 session search, schedule, commands, and user questions —
//! run the package-owned invariant companions, and expose a boot report
//! with a real end-to-end durability + search probe.
//!
//! The M6 shell upgrade composes the web face on top: the webserver route
//! service, the SPA dist server (fallback seat), the directory-picker seam
//! (browse backend), the plugin inventory, and the apiproxy gateway.

use std::sync::Arc;

use futures::FutureExt;

mod client_plugins;
mod web_preview;

#[cfg(windows)]
static ALLOCATOR_COLLECT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(windows)]
thread_local! {
    static LAST_ALLOCATOR_COLLECT_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(windows)]
pub fn request_allocator_collect() {
    use std::sync::atomic::Ordering;
    ALLOCATOR_COLLECT_EPOCH.fetch_add(1, Ordering::AcqRel);
}

#[cfg(windows)]
pub fn collect_allocator_on_park() {
    use std::sync::atomic::Ordering;
    let epoch = ALLOCATOR_COLLECT_EPOCH.load(Ordering::Acquire);
    LAST_ALLOCATOR_COLLECT_EPOCH.with(|last| {
        if last.get() == epoch {
            return;
        }
        unsafe { libmimalloc_sys::mi_collect(true) };
        last.set(epoch);
    });
}

struct CollectAfterBytesInner<F: FnOnce()> {
    bytes: Vec<u8>,
    collect: parking_lot::Mutex<Option<F>>,
}

impl<F: FnOnce()> AsRef<[u8]> for CollectAfterBytesInner<F> {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl<F: FnOnce()> Drop for CollectAfterBytesInner<F> {
    fn drop(&mut self) {
        if let Some(collect) = self.collect.get_mut().take() {
            collect();
        }
    }
}

fn bytes_then_collect_stream<F>(
    bytes: Vec<u8>,
    collect: F,
) -> impl futures::Stream<Item = Result<axum::body::Bytes, std::convert::Infallible>>
where
    F: FnOnce() + Send + 'static,
{
    let bytes = axum::body::Bytes::from_owner(CollectAfterBytesInner {
        bytes,
        collect: parking_lot::Mutex::new(Some(collect)),
    });
    futures::stream::once(async move { Ok(bytes) })
}

#[cfg(windows)]
fn collect_allocator_after_response() {
    // The response byte buffer has reached body EOS or was dropped. Collect
    // the current HTTP worker, notify every Tokio worker through the park
    // epoch, and also run one collection on the blocking pool where JSONL /
    // SQLite work may have allocated transient pages.
    unsafe { libmimalloc_sys::mi_collect(true) };
    request_allocator_collect();
}

#[cfg(test)]
mod allocator_response_lifecycle_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::StreamExt;

    #[tokio::test]
    async fn history_body_requests_collection_only_after_end_of_stream() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let mut stream = Box::pin(super::bytes_then_collect_stream(vec![1, 2, 3], move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));

        let bytes = stream.next().await.unwrap().unwrap();
        assert_eq!(bytes.as_ref(), &[1, 2, 3]);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(stream.next().await.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(bytes);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dropped_history_body_still_requests_collection() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let mut stream = Box::pin(super::bytes_then_collect_stream(vec![1, 2, 3], move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));

        let bytes = stream.next().await.unwrap().unwrap();
        drop(stream);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(bytes);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

fn packaged_resource(relative: &str) -> std::path::PathBuf {
    let adjacent = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(relative)))
        .filter(|path| path.exists());
    adjacent.unwrap_or_else(|| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../")
            .join(relative)
    })
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiCompatibleModelConfig {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    max_tokens: Option<u64>,
    /// Hermes-style stable effort ids mapped to exact provider wire values.
    /// `off: null` means disabling reasoning is represented by omission.
    #[serde(default)]
    reasoning_efforts: Option<ReasoningEffortsConfig>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
enum ReasoningEffortsConfig {
    Disabled(bool),
    Levels(indexmap::IndexMap<String, Option<String>>),
}

fn inferred_gpt_reasoning_efforts(
    model_id: &str,
) -> Option<indexmap::IndexMap<String, Option<String>>> {
    let normalized = model_id.to_ascii_lowercase();
    if !normalized.starts_with("gpt-5") {
        return None;
    }
    Some(indexmap::IndexMap::from([
        ("off".to_string(), None),
        ("minimal".to_string(), Some("minimal".to_string())),
        ("low".to_string(), Some("low".to_string())),
        ("medium".to_string(), Some("medium".to_string())),
        ("high".to_string(), Some("high".to_string())),
        // Keep the stable DSH/Hermes maximum id used by existing settings,
        // while dispatching the exact OpenAI GPT wire spelling.
        ("max".to_string(), Some("xhigh".to_string())),
    ]))
}

fn resolved_reasoning_efforts(
    model: &OpenAiCompatibleModelConfig,
) -> Option<indexmap::IndexMap<String, Option<String>>> {
    match model.reasoning_efforts.as_ref() {
        Some(ReasoningEffortsConfig::Levels(levels)) => Some(levels.clone()),
        Some(ReasoningEffortsConfig::Disabled(false)) => None,
        Some(ReasoningEffortsConfig::Disabled(true)) => None,
        None => inferred_gpt_reasoning_efforts(&model.id),
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiCompatibleProviderConfig {
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    keyless: bool,
    #[serde(default)]
    display_name: Option<String>,
    api: String,
    #[serde(rename = "baseURL")]
    base_url: String,
    models: Vec<OpenAiCompatibleModelConfig>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct OpenAiCompatibleSettings {
    #[serde(default)]
    providers: indexmap::IndexMap<String, OpenAiCompatibleProviderConfig>,
}

struct OpenAiCompatibleAdapter {
    profiles: Arc<parking_lot::Mutex<indexmap::IndexMap<String, OpenAiCompatibleProviderConfig>>>,
    credentials: Arc<dsh_credentials_local::LocalCredentialProvider>,
    attachment_ctx: Context,
}

fn openai_compatible_schema() -> dsh_schemastery::Schema {
    use dsh_schemastery::{Data, Schema};

    let mut model = indexmap::IndexMap::new();
    model.insert("id".to_string(), Schema::string().required(true));
    model.insert("name".to_string(), Schema::string());
    model.insert(
        "contextWindow".to_string(),
        Schema::number().min(1.0).step(1.0),
    );
    model.insert("maxTokens".to_string(), Schema::number().min(1.0).step(1.0));
    model.insert(
        "reasoningEfforts".to_string(),
        Schema::union(vec![
            Schema::constant(Data::Bool(false)),
            Schema::dict(
                Schema::union(vec![Schema::string(), Schema::constant(Data::Null)]),
                None,
            ),
        ]),
    );

    let mut profile = indexmap::IndexMap::new();
    profile.insert(
        "apiKeyEnv".to_string(),
        Schema::string().role("credential-ref", None),
    );
    profile.insert(
        "keyless".to_string(),
        Schema::boolean().default(Data::Bool(false)),
    );
    profile.insert("displayName".to_string(), Schema::string());
    profile.insert(
        "api".to_string(),
        Schema::union(vec![
            Schema::constant(Data::String("openai-completions".to_string())),
            Schema::constant(Data::String("openai-responses".to_string())),
        ])
        .required(true),
    );
    profile.insert("baseURL".to_string(), Schema::string().required(true));
    profile.insert(
        "models".to_string(),
        Schema::array(Schema::object(model)).required(true),
    );

    let mut root = indexmap::IndexMap::new();
    root.insert(
        "providers".to_string(),
        Schema::dict(Schema::object(profile), None)
            .default(Data::Object(indexmap::IndexMap::new())),
    );
    Schema::object(root)
}

fn discovery_models_url(base_url: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(base_url)
        .map_err(|error| format!("invalid provider baseURL: {error}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| "provider baseURL has no host".to_string())?;
    if url.scheme() != "https"
        && !(url.scheme() == "http" && matches!(host, "127.0.0.1" | "localhost" | "::1"))
    {
        return Err(
            "model discovery requires HTTPS (loopback HTTP is allowed for testing)".to_string(),
        );
    }
    let path = url.path().trim_end_matches('/');
    url.set_path(&format!("{path}/models"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

async fn discover_openai_compatible_models(
    request: dsh_llm::LlmModelDiscoveryRequest,
) -> Result<Vec<dsh_llm::LlmDiscoveredModel>, String> {
    if request
        .api
        .as_deref()
        .is_some_and(|api| api != "openai-completions" && api != "openai-responses")
    {
        return Err(
            "model discovery only supports openai-completions or openai-responses".to_string(),
        );
    }
    let base_url = request
        .base_url
        .as_deref()
        .ok_or_else(|| "model discovery needs a baseURL".to_string())?;
    let url = discovery_models_url(base_url)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("model discovery client failed: {error}"))?;
    let mut builder = client.get(url);
    if let Some(api_key) = request.api_key.as_deref().filter(|key| !key.is_empty()) {
        builder = builder.bearer_auth(api_key);
    }
    let response = if let Some(signal) = request.signal.as_ref() {
        tokio::select! {
            response = builder.send() => response,
            _ = async {
                while !signal() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            } => return Err("model discovery aborted".to_string()),
        }
    } else {
        builder.send().await
    }
    .map_err(|error| format!("model discovery request failed: {error}"))?;
    let status = response.status();
    let mut response = response;
    let mut bytes = Vec::new();
    loop {
        if request.signal.as_ref().is_some_and(|signal| signal()) {
            return Err("model discovery aborted".to_string());
        }
        let chunk = response
            .chunk()
            .await
            .map_err(|error| format!("model discovery response failed: {error}"))?;
        let Some(chunk) = chunk else {
            break;
        };
        if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
            return Err("model discovery response exceeded 1 MiB".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(format!("model discovery returned HTTP {status}"));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("model discovery returned invalid JSON: {error}"))?;
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "model discovery response needs a data array".to_string())?;
    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();
    for item in data {
        let Some(id) = item.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        models.push(dsh_llm::LlmDiscoveredModel {
            id: id.to_string(),
            name: item
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            context_window: item
                .get("context_window")
                .and_then(serde_json::Value::as_u64),
            max_tokens: item
                .get("max_tokens")
                .and_then(serde_json::Value::as_u64)
                .or_else(|| dsh_llm_deepseek::inferred_model_max_tokens(id)),
        });
    }
    Ok(models)
}

fn openai_profiles(value: &dsh_schemastery::Data) -> Result<OpenAiCompatibleSettings, String> {
    let json = value
        .to_json()
        .ok_or_else(|| "llm-pi-ai settings are not JSON-compatible".to_string())?;
    let settings: OpenAiCompatibleSettings =
        serde_json::from_value(json).map_err(|error| format!("llm-pi-ai: {error}"))?;
    for (provider, profile) in &settings.providers {
        if provider.is_empty()
            || !provider.chars().enumerate().all(|(index, ch)| {
                ch.is_ascii_lowercase() || ch.is_ascii_digit() || (index > 0 && ch == '-')
            })
        {
            return Err(format!("llm-pi-ai: invalid provider route \"{provider}\""));
        }
        if profile.api != "openai-completions" && profile.api != "openai-responses" {
            return Err(format!(
                "llm-pi-ai: provider \"{provider}\" must use openai-completions or openai-responses"
            ));
        }
        if !(profile.base_url.starts_with("https://")
            || profile.base_url.starts_with("http://127.0.0.1")
            || profile.base_url.starts_with("http://localhost"))
        {
            return Err(format!(
                "llm-pi-ai: provider \"{provider}\" needs an HTTPS baseURL (loopback HTTP is allowed for testing)"
            ));
        }
        if profile.models.is_empty() || profile.models.iter().any(|model| model.id.is_empty()) {
            return Err(format!(
                "llm-pi-ai: provider \"{provider}\" needs at least one named model"
            ));
        }
        const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
        for model in &profile.models {
            let Some(reasoning) = &model.reasoning_efforts else {
                continue;
            };
            let efforts = match reasoning {
                ReasoningEffortsConfig::Disabled(false) => continue,
                ReasoningEffortsConfig::Disabled(true) => {
                    return Err(format!(
                        "llm-pi-ai: provider \"{provider}\" model \"{}\" reasoningEfforts only accepts false or a level map",
                        model.id
                    ));
                }
                ReasoningEffortsConfig::Levels(efforts) => efforts,
            };
            if efforts.is_empty() {
                return Err(format!(
                    "llm-pi-ai: provider \"{provider}\" model \"{}\" has empty reasoningEfforts",
                    model.id
                ));
            }
            for (level, wire) in efforts {
                if !LEVELS.contains(&level.as_str()) {
                    return Err(format!(
                        "llm-pi-ai: provider \"{provider}\" model \"{}\" has unknown reasoning effort \"{level}\"",
                        model.id
                    ));
                }
                if level != "off" && wire.as_deref().is_none_or(str::is_empty) {
                    return Err(format!(
                        "llm-pi-ai: provider \"{provider}\" model \"{}\" reasoningEfforts.{level} needs a wire value",
                        model.id
                    ));
                }
                if level == "off" && wire.as_deref().is_some_and(str::is_empty) {
                    return Err(format!(
                        "llm-pi-ai: provider \"{provider}\" model \"{}\" reasoningEfforts.off must be null or a non-empty wire value",
                        model.id
                    ));
                }
            }
            if !efforts.keys().any(|level| level != "off") {
                return Err(format!(
                    "llm-pi-ai: provider \"{provider}\" model \"{}\" offers no reasoning level beyond off",
                    model.id
                ));
            }
        }
        if !profile.keyless && profile.api_key_env.as_deref().is_none_or(str::is_empty) {
            return Err(format!(
                "llm-pi-ai: provider \"{provider}\" needs apiKeyEnv unless keyless is true"
            ));
        }
    }
    Ok(settings)
}

impl OpenAiCompatibleAdapter {
    #[allow(clippy::result_large_err)] // The delegate must preserve the shared LlmError resolver seam.
    fn delegate(&self, provider: &str) -> dsh_llm_deepseek::DeepSeekAdapter {
        let profile = self
            .profiles
            .lock()
            .get(provider)
            .cloned()
            .expect("registered OpenAI-compatible route has a profile");
        let resolved =
            dsh_llm_deepseek::resolve_adapter_options(&dsh_llm_deepseek::DeepSeekConfig {
                api: Some(profile.api.clone()),
                api_key_env: profile.api_key_env.clone(),
                keyless: profile.keyless,
                base_url: Some(profile.base_url.clone()),
                models: Some(
                    profile
                        .models
                        .iter()
                        .map(|model| dsh_llm_deepseek::DeepSeekCatalogModel {
                            id: model.id.clone(),
                            name: model.name.clone(),
                            description: None,
                            context_window: model.context_window,
                            max_tokens: model.max_tokens,
                            reasoning_efforts: match model.reasoning_efforts.as_ref() {
                                Some(ReasoningEffortsConfig::Disabled(false)) => Some(Vec::new()),
                                Some(ReasoningEffortsConfig::Disabled(true)) => None,
                                Some(ReasoningEffortsConfig::Levels(_)) | None => {
                                    resolved_reasoning_efforts(model).map(|efforts| {
                                        efforts
                                            .iter()
                                            .map(|(id, wire)| {
                                                dsh_llm_deepseek::CatalogReasoningEffort {
                                                    id: id.clone(),
                                                    name: match id.as_str() {
                                                        "off" => "Off",
                                                        "minimal" => "Minimal",
                                                        "low" => "Low",
                                                        "medium" => "Medium",
                                                        "high" => "High",
                                                        "xhigh" => "Extra High",
                                                        "max" => "Extra High",
                                                        _ => id.as_str(),
                                                    }
                                                    .to_string(),
                                                    wire: wire
                                                        .clone()
                                                        .unwrap_or_else(|| "off".to_string()),
                                                }
                                            })
                                            .collect()
                                    })
                                }
                            },
                            image_input: None,
                        })
                        .collect(),
                ),
                ..Default::default()
            })
            .expect("validated OpenAI-compatible profile");
        let credentials = self.credentials.clone();
        dsh_llm_deepseek::DeepSeekAdapter::new(dsh_llm_deepseek::DeepSeekAdapterOptions {
            options: Arc::new(move || Ok(resolved.clone())),
            resolve_api_key: Arc::new(move |snapshot| {
                let credentials = credentials.clone();
                let api_key_env = snapshot.api_key_env.clone();
                Box::pin(async move {
                    let reference = dsh_credentials::credential_ref(&api_key_env);
                    Ok(credentials
                        .resolve(&reference)
                        .await
                        .map(|value| value.value))
                })
            }),
            resolve_attachments: Some(Arc::new({
                let attachment_ctx = self.attachment_ctx.clone();
                move || {
                    attachment_ctx
                        .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false)
                        .map(|slot| slot.as_ref().clone())
                }
            })),
            provider_name: Some(
                profile
                    .display_name
                    .clone()
                    .unwrap_or_else(|| provider.to_string()),
            ),
            reasoning_wire_format: dsh_llm_deepseek::ReasoningWireFormat::OpenAi,
        })
    }
}

#[async_trait::async_trait]
impl dsh_llm::LlmAdapter for OpenAiCompatibleAdapter {
    fn provider_info(&self, provider: &str) -> dsh_llm::LlmProviderInfo {
        self.delegate(provider).provider_info(provider)
    }

    fn provider_retry_policy(&self, provider: &str) -> Option<dsh_llm::ResolvedRetryPolicy> {
        self.delegate(provider).provider_retry_policy(provider)
    }

    async fn list_models(&self, provider: &str) -> Vec<dsh_llm::LlmModelInfo> {
        self.delegate(provider).list_models(provider).await
    }

    async fn resolve_model(
        &self,
        provider: &str,
        model: &str,
        signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> dsh_llm::LlmResolvedModelInfo {
        self.delegate(provider)
            .resolve_model(provider, model, signal)
            .await
    }

    fn stream(&self, options: &dsh_llm::GenerateOptions) -> dsh_llm::ChunkStream {
        // The delegated adapter captures one immutable profile snapshot and
        // maps stable effort ids to wire values inside that same request.
        self.delegate(&options.provider).stream(options)
    }
}

use axum::body::Body as WebBody;
use cordis::{ArcValue, Context, Plugin, PluginError, arc, make_disposer};
use dsh_agent::AgentRegistry;
use dsh_agent_loop::AgentLoop;
use dsh_commands::CommandRuntime;
use dsh_credentials::CredentialProvider;
use dsh_goal::GoalService;
use dsh_host_apiproxy::{
    AbortSignal, ApiProxyCarrier, ApiProxyDefaults, ApiProxyService, Body as CarrierBody,
    CarrierRequest, FetchHandler, FrameRequest, rpc_id, to_fetch_handler,
};
use dsh_host_directory_picker_browse::{BrowseDirectoryPicker, Config as PickerConfig};
use dsh_host_frontend_static::Config as FrontendConfig;
use dsh_host_frontend_static::apply as apply_frontend_static;
use dsh_host_plugin_inventory::PluginInventoryGateway;
use dsh_host_webserver::{
    Config as WebConfig, Host as BindHost, RouteDisposer, WebHandlerError, WebRequest, WebResponse,
    WebRoute, WebRouteKind, WebServer, WebUpgradeRoute, WebUpgraded, accept_websocket,
};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_jobs_local::LocalJobRegistry;
use dsh_llm::LlmRuntime;
use dsh_pwsh_local::LocalPwshExecutor;
use dsh_sandbox_local::LocalSandboxProvider;
use dsh_sandbox_policy::SandboxPolicyService;
use dsh_session::{SessionStore, session_id};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_session_persistence_jsonl::{JsonlCompression, JsonlConfig, JsonlSessionPersistence};
use dsh_session_query::{SessionQueryEngine, SessionSearchRequest};
use dsh_session_query_sqlite::{Config as SqliteSearchConfig, SqliteSearch};
use dsh_subprocess_local::LocalSubprocessRuntime;
use dsh_system_prompt::{PromptSection, PromptText, SystemPrompt};
use dsh_terminal::TerminalSessionService;
use dsh_tools::ToolRuntime;
use dsh_user_approval::ApprovalService;
use dsh_user_questions::UserQuestionService;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

const MAX_API_REQUEST_BODY_BYTES: usize = 300 * 1024 * 1024;

fn display_workspace_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

fn workspace_context_text(cwd: Option<&str>) -> String {
    match cwd {
        Some(cwd) => format!(
            "Current workspace (authoritative session working directory): {}. Use this exact path and drive for workspace-relative work; do not infer it from the Harness checkout or sandbox fallback.",
            display_workspace_path(cwd)
        ),
        None => "Current workspace: unavailable because this session has no working directory. Do not infer one from the Harness checkout or sandbox fallback.".to_string(),
    }
}

struct JsonSettingsStorage {
    path: std::path::PathBuf,
    defaults: serde_json::Map<String, serde_json::Value>,
    document: parking_lot::Mutex<indexmap::IndexMap<String, dsh_schemastery::Data>>,
}

fn json_to_settings_data(value: &serde_json::Value) -> dsh_schemastery::Data {
    match value {
        serde_json::Value::Null => dsh_schemastery::Data::Null,
        serde_json::Value::Bool(value) => dsh_schemastery::Data::Bool(*value),
        serde_json::Value::Number(value) => {
            dsh_schemastery::Data::Number(value.as_f64().unwrap_or(0.0))
        }
        serde_json::Value::String(value) => dsh_schemastery::Data::String(value.clone()),
        serde_json::Value::Array(values) => {
            dsh_schemastery::Data::Array(values.iter().map(json_to_settings_data).collect())
        }
        serde_json::Value::Object(values) => dsh_schemastery::Data::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), json_to_settings_data(value)))
                .collect(),
        ),
    }
}

fn merge_package_defaults(
    mut defaults: serde_json::Map<String, serde_json::Value>,
    user: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    for (key, value) in user {
        match (defaults.get_mut(&key), value) {
            (Some(serde_json::Value::Object(base)), serde_json::Value::Object(next)) => {
                *base = merge_package_defaults(std::mem::take(base), next);
            }
            (_, value) => {
                defaults.insert(key, value);
            }
        }
    }
    defaults
}

fn package_settings_defaults() -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let path = packaged_resource("settings.defaults.json");
    if !path.is_file() {
        return Ok(serde_json::Map::new());
    }
    let value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("parse {}: {error}", path.display()))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))
}

fn migrate_legacy_theme_settings(
    mut document: indexmap::IndexMap<String, dsh_schemastery::Data>,
) -> indexmap::IndexMap<String, dsh_schemastery::Data> {
    const RETIRED: [&str; 9] = [
        "system",
        "catppuccin",
        "dracula",
        "nord",
        "tokyo-night",
        "linear",
        "notion",
        "whale-song",
        "dragon-heir",
    ];
    let Some(dsh_schemastery::Data::Object(theme)) = document.get_mut("ui-theme") else {
        return document;
    };
    let should_migrate = matches!(
        theme.get("preference"),
        Some(dsh_schemastery::Data::String(preference))
            if RETIRED.contains(&preference.as_str())
    );
    if should_migrate {
        theme.insert(
            "preference".to_string(),
            dsh_schemastery::Data::String("light".to_string()),
        );
    }
    document
}

#[cfg(test)]
mod theme_settings_migration_tests {
    use super::{merge_package_defaults, migrate_legacy_theme_settings};
    use dsh_schemastery::Data;
    use indexmap::IndexMap;

    fn preference(document: &IndexMap<String, Data>) -> Option<&str> {
        let Data::Object(theme) = document.get("ui-theme")? else {
            return None;
        };
        let Data::String(preference) = theme.get("preference")? else {
            return None;
        };
        Some(preference)
    }

    fn document(preference: &str) -> IndexMap<String, Data> {
        let mut theme = IndexMap::new();
        theme.insert(
            "preference".to_string(),
            Data::String(preference.to_string()),
        );
        let mut document = IndexMap::new();
        document.insert("ui-theme".to_string(), Data::Object(theme));
        document
    }

    #[test]
    fn retired_theme_preferences_migrate_to_default_light() {
        for retired in [
            "system",
            "catppuccin",
            "dracula",
            "nord",
            "tokyo-night",
            "linear",
            "notion",
        ] {
            let migrated = migrate_legacy_theme_settings(document(retired));
            assert_eq!(preference(&migrated), Some("light"));
        }
    }

    #[test]
    fn current_skin_preference_is_preserved() {
        let migrated = migrate_legacy_theme_settings(document("blue-fantasy"));
        assert_eq!(preference(&migrated), Some("blue-fantasy"));
    }

    #[test]
    fn package_defaults_fill_missing_sections_but_user_values_win() {
        let defaults = serde_json::json!({
            "ui-theme": {"preference": "deepseek-official", "fontSize": 14},
            "agent-default-model": {"provider": "opencode-free", "model": "mimo-v2.5-free"}
        })
        .as_object()
        .cloned()
        .expect("defaults object");
        let user = serde_json::json!({
            "ui-theme": {"preference": "dark"}
        })
        .as_object()
        .cloned()
        .expect("user object");
        let merged = merge_package_defaults(defaults, user);
        assert_eq!(merged["ui-theme"]["preference"], "dark");
        assert_eq!(merged["ui-theme"]["fontSize"], 14);
        assert_eq!(merged["agent-default-model"]["model"], "mimo-v2.5-free");
    }
}

#[async_trait::async_trait]
impl dsh_settings::SettingsStorage for JsonSettingsStorage {
    fn writable(&self) -> bool {
        true
    }

    fn document_path(&self) -> Option<String> {
        Some(self.path.to_string_lossy().into_owned())
    }

    async fn load(&self) -> Result<indexmap::IndexMap<String, dsh_schemastery::Data>, String> {
        let user = if self.path.exists() {
            let value: serde_json::Value = serde_json::from_slice(
                &tokio::fs::read(&self.path)
                    .await
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            value
                .as_object()
                .cloned()
                .ok_or_else(|| "settings document must be an object".to_string())?
        } else {
            serde_json::Map::new()
        };
        let value = serde_json::Value::Object(merge_package_defaults(self.defaults.clone(), user));
        let data = json_to_settings_data(&value);
        let dsh_schemastery::Data::Object(document) = data else {
            return Err("settings document must be an object".to_string());
        };
        let document = migrate_legacy_theme_settings(document);
        *self.document.lock() = document.clone();
        Ok(document)
    }

    async fn persist(
        &self,
        ns: &dsh_settings::SettingsNamespace,
        section: dsh_schemastery::Data,
    ) -> Result<(), String> {
        let path = self.path.clone();
        let ns = ns.as_str().to_string();
        let committed = dsh_atomic_write::with_file_lock(&path, async {
            let mut document = if path.exists() {
                let value: serde_json::Value =
                    serde_json::from_slice(&tokio::fs::read(&path).await?)
                        .map_err(std::io::Error::other)?;
                let data = json_to_settings_data(&value);
                let dsh_schemastery::Data::Object(document) = data else {
                    return Err(std::io::Error::other("settings document must be an object"));
                };
                document
            } else {
                indexmap::IndexMap::new()
            };
            document.insert(ns, section);
            let value = dsh_schemastery::Data::Object(document.clone())
                .to_json()
                .ok_or_else(|| std::io::Error::other("settings document is not JSON-compatible"))?;
            let bytes = serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?;
            dsh_atomic_write::write_file_atomic(
                &path,
                &bytes,
                dsh_atomic_write::WriteFileAtomicOptions {
                    mode: 0o600,
                    dir_mode: Some(0o700),
                },
            )
            .await?;
            Ok::<_, std::io::Error>(document)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        *self.document.lock() = committed;
        Ok(())
    }
}

fn decode_query_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let pair = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                if let Ok(byte) = u8::from_str_radix(pair, 16) {
                    decoded.push(byte);
                    index += 2;
                } else {
                    decoded.push(b'%');
                }
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn parse_query(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (decode_query_component(key), decode_query_component(value))
        })
        .collect()
}

fn valid_authority_port(port: Option<&str>) -> bool {
    port.is_none_or(|port| !port.is_empty() && port.parse::<u16>().is_ok())
}

fn is_loopback_authority(authority: &str) -> bool {
    if authority.is_empty() || authority.trim() != authority {
        return false;
    }
    if let Some(ipv6) = authority.strip_prefix('[') {
        let Some((host, suffix)) = ipv6.split_once(']') else {
            return false;
        };
        let port = if suffix.is_empty() {
            None
        } else if let Some(port) = suffix.strip_prefix(':') {
            Some(port)
        } else {
            return false;
        };
        return host == "::1" && valid_authority_port(port);
    }

    let (host, port) = match authority.split_once(':') {
        Some((host, port)) if !port.contains(':') => (host, Some(port)),
        Some(_) => return false,
        None => (authority, None),
    };
    if !valid_authority_port(port) {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let octets = host.split('.').collect::<Vec<_>>();
    octets.len() == 4
        && octets[0] == "127"
        && octets.iter().all(|octet| {
            (1..=3).contains(&octet.len())
                && octet.bytes().all(|byte| byte.is_ascii_digit())
                && octet.parse::<u8>().is_ok()
        })
}

fn canonical_authority(authority: &str, default_port: Option<u16>) -> Option<String> {
    let parsed = authority.parse::<http::uri::Authority>().ok()?;
    let host = parsed.host().to_ascii_lowercase();
    let port = parsed.port_u16().filter(|port| Some(*port) != default_port);
    Some(match port {
        Some(port) if host.contains(':') => format!("[{host}]:{port}"),
        Some(port) => format!("{host}:{port}"),
        None if host.contains(':') => format!("[{host}]"),
        None => host,
    })
}

fn origin_matches_host(origin: &str, host: &str) -> bool {
    if origin == "null" {
        return false;
    }
    let Ok(uri) = origin.parse::<http::Uri>() else {
        return false;
    };
    let default_port = match uri.scheme_str() {
        Some("http") => Some(80),
        Some("https") => Some(443),
        _ => return false,
    };
    let Some(origin_authority) = uri.authority() else {
        return false;
    };
    canonical_authority(origin_authority.as_str(), default_port)
        == canonical_authority(host, Some(80))
}

fn trusted_web_request(request: &WebRequest) -> bool {
    let host = request
        .headers()
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok());
    host.is_some_and(is_loopback_authority)
        && !request
            .headers()
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|site| site == "cross-site")
        && !request
            .headers()
            .get(http::header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|origin| !host.is_some_and(|host| origin_matches_host(origin, host)))
}

async fn pump_websocket_downlink(
    request: WebRequest,
    socket: WebUpgraded,
    api: Arc<ApiProxyService>,
    host_stream: bool,
) -> Result<(), WebHandlerError> {
    if !trusted_web_request(&request) {
        return Err(WebHandlerError::new("forbidden"));
    }
    let mut websocket = accept_websocket(socket).await;
    let signal = AbortSignal::new();
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let frame_request = FrameRequest {
        rpc_id: rpc_id(uuid::Uuid::new_v4().to_string()),
        payload: serde_json::json!({}),
    };
    let mut frames = if host_stream {
        api.events_host(frame_request, signal.clone())
    } else {
        api.events_mux(frame_request, signal.clone())
    };
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                websocket
                    .send(Message::Ping(Vec::new().into()))
                    .await
                    .map_err(|error| WebHandlerError::new(error.to_string()))?;
            }
            frame = frames.next() => {
                let Some(frame) = frame else { break; };
                let method = frame.payload.get("type").and_then(serde_json::Value::as_str).unwrap_or("stream/error");
                let wire = serde_json::json!({
                    "type": "server-request",
                    "rpcId": frame.rpc_id,
                    "method": method,
                    "payload": frame.payload,
                });
                websocket.send(Message::Text(serde_json::to_string(&wire).map_err(|error| WebHandlerError::new(error.to_string()))?.into())).await.map_err(|error| WebHandlerError::new(error.to_string()))?;
            }
            incoming = websocket.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(payload))) => {
                        websocket.send(Message::Pong(payload)).await.map_err(|error| WebHandlerError::new(error.to_string()))?;
                    }
                    Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => {
                        // Browser clients are downlink-only; ignore incidental
                        // carrier frames rather than tearing down a healthy
                        // subscription before its first host event.
                    }
                    Some(Err(_)) => break,
                }
            }
        }
    }
    signal.abort();
    Ok(())
}

async fn bridge_api_request(request: WebRequest, handler: Arc<FetchHandler>) -> WebResponse {
    let (parts, incoming) = request.into_parts();
    let host = parts
        .headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok());
    let trusted_host = host.is_some_and(is_loopback_authority);
    let cross_site = parts
        .headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site == "cross-site");
    let mismatched_origin = parts
        .headers
        .get(http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| !host.is_some_and(|host| origin_matches_host(origin, host)));
    if !trusted_host || cross_site || mismatched_origin {
        return http::Response::builder()
            .status(http::StatusCode::FORBIDDEN)
            .body(WebBody::from("forbidden"))
            .expect("static response");
    }
    if parts
        .headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_API_REQUEST_BODY_BYTES)
    {
        return http::Response::builder()
            .status(http::StatusCode::PAYLOAD_TOO_LARGE)
            .body(WebBody::empty())
            .expect("static response");
    }
    let bytes = match axum::body::to_bytes(WebBody::new(incoming), MAX_API_REQUEST_BODY_BYTES).await
    {
        Ok(bytes) => bytes,
        Err(_) => {
            return http::Response::builder()
                .status(http::StatusCode::PAYLOAD_TOO_LARGE)
                .body(WebBody::empty())
                .expect("static response");
        }
    };
    let query = parts.uri.query().map(parse_query).unwrap_or_default();
    let headers = parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect();
    #[cfg(windows)]
    let collect_after_response = parts.uri.path() == "/api/session.history";
    let response = handler
        .handle(CarrierRequest {
            method: parts.method,
            path: parts.uri.path().to_string(),
            query,
            headers,
            body: (!bytes.is_empty()).then(|| bytes.to_vec()),
        })
        .await;
    let (parts, body) = response.into_parts();
    let body = match body {
        CarrierBody::Bytes(bytes) => {
            #[cfg(windows)]
            {
                if collect_after_response {
                    return WebResponse::from_parts(
                        parts,
                        WebBody::from_stream(bytes_then_collect_stream(
                            bytes,
                            collect_allocator_after_response,
                        )),
                    );
                } else if bytes.len() >= 256 * 1024 {
                    // SAFETY: the typed RPC tree has already been serialized
                    // and dropped; collect this worker's transient pages.
                    unsafe { libmimalloc_sys::mi_collect(true) };
                }
            }
            WebBody::from(bytes)
        }
        CarrierBody::Stream(stream) => {
            use futures::StreamExt;
            let stream = stream.map(|item| item.map_err(|message| std::io::Error::other(message)));
            WebBody::from_stream(stream)
        }
    };
    WebResponse::from_parts(parts, body)
}

/// One booted host spine: the root context plus its registered services and
/// the disposable data directories owned by this boot.
pub struct HostSpine {
    pub ctx: Context,
    pub sessions: Arc<SessionStore>,
    pub agents: Arc<AgentRegistry>,
    pub llm: Arc<LlmRuntime>,
    pub agent_loop: Arc<AgentLoop>,
    pub tools: Arc<ToolRuntime>,
    pub system_prompt: Arc<SystemPrompt>,
    pub commands: Arc<CommandRuntime>,
    pub goals: Arc<GoalService>,
    pub questions: Arc<UserQuestionService>,
    pub approval: Arc<ApprovalService>,
    pub message_feedback: Arc<dsh_message_feedback::MessageFeedbackService>,
    pub persistence: Arc<JsonlSessionPersistence>,
    pub search: Arc<SqliteSearch>,
    pub query: Arc<SessionQueryEngine>,
    pub web_server: Arc<WebServer>,
    pub api_proxy: Arc<ApiProxyService>,
    pub agent_presets: Arc<dsh_agent_presets::AgentPresets>,
    api_route: RouteDisposer,
    wallpaper_route: RouteDisposer,
    web_preview_route: RouteDisposer,
    data_root: std::path::PathBuf,
    owns_data_root: bool,
    boot_probe_id: dsh_session::SessionId,
    companion_fiber: parking_lot::Mutex<Option<Arc<cordis::FiberCore>>>,
    lifecycle_fiber: parking_lot::Mutex<Option<Arc<cordis::FiberCore>>>,
    shutdown_result: tokio::sync::OnceCell<Result<(), String>>,
    shutdown_requested: std::sync::atomic::AtomicBool,
    shutdown_failures: Arc<parking_lot::Mutex<Vec<String>>>,
}

/// The network coordinates published only after the host has bound its
/// listener. `bound_addr` contains the OS-selected port when port zero was
/// requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostReadiness {
    pub bound_addr: std::net::SocketAddr,
}

/// Cloneable application-facing owner used by CLI and desktop launchers.
/// Clones join the same idempotent shutdown barrier.
#[derive(Clone)]
pub struct HostHandle {
    spine: Arc<HostSpine>,
}

impl HostHandle {
    pub fn spine(&self) -> &HostSpine {
        &self.spine
    }

    pub fn readiness(&self) -> HostReadiness {
        self.spine.readiness()
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.spine.shutdown().await
    }
}

impl std::ops::Deref for HostHandle {
    type Target = HostSpine;

    fn deref(&self) -> &Self::Target {
        &self.spine
    }
}

impl HostSpine {
    pub fn readiness(&self) -> HostReadiness {
        HostReadiness {
            bound_addr: self.web_server.bound_addr(),
        }
    }

    pub fn data_root(&self) -> &std::path::Path {
        &self.data_root
    }

    /// Stop ingress, dispose the host-owned fiber tree, drain persistence, and
    /// only then remove the temporary data root. Concurrent/repeated callers
    /// join the same result.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.shutdown_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.shutdown_result
            .get_or_init(|| async {
                self.web_server.shutdown().await;
                (self.web_preview_route)();
                (self.wallpaper_route)();
                (self.api_route)();

                let companion_fiber = self.companion_fiber.lock().clone();
                if let Some(fiber) = companion_fiber {
                    fiber.dispose().await;
                }

                let lifecycle_fiber = self.lifecycle_fiber.lock().clone();
                match lifecycle_fiber {
                    Some(fiber) => fiber.dispose().await,
                    None => self
                        .shutdown_failures
                        .lock()
                        .push("host lifecycle fiber is missing".to_string()),
                }

                let failures = self.shutdown_failures.lock().clone();
                if !failures.is_empty() {
                    return Err(format!(
                        "host shutdown did not drain safely: {}",
                        failures.join("; ")
                    ));
                }
                if self.owns_data_root {
                    match tokio::fs::remove_dir_all(&self.data_root).await {
                        Ok(()) => Ok(()),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(format!(
                            "host shutdown could not remove data root {}: {error}",
                            self.data_root.display()
                        )),
                    }
                } else {
                    Ok(())
                }
            })
            .await
            .clone()
    }
}

impl Drop for HostSpine {
    fn drop(&mut self) {
        if !self
            .shutdown_requested
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.web_server.request_shutdown();
            (self.web_preview_route)();
            (self.wallpaper_route)();
            (self.api_route)();
            eprintln!(
                "dsh-host dropped without shutdown().await; stop requested and data root preserved at {}",
                self.data_root.display()
            );
        }
    }
}

struct HostCompositionPlugin {
    output: Arc<parking_lot::Mutex<Option<Result<HostSpine, String>>>>,
    data_root: Option<std::path::PathBuf>,
    profile: Option<String>,
    port: u16,
}

#[async_trait::async_trait]
impl Plugin for HostCompositionPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("dsh-host-composition")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        *self.output.lock() = Some(compose_host_in_fiber(
            ctx,
            self.data_root.clone(),
            self.profile.as_deref(),
            self.port,
        ));
        Ok(())
    }
}

/// Compose the M6 host spine synchronously (the async service bindings
/// settle through their own fibers).
pub fn compose_host(ctx: &Context) -> Result<HostSpine, String> {
    compose_host_with_root(ctx, None, None, 0)
}

/// Compose the long-running application Host against the stable DSH home.
pub fn compose_persistent_host(ctx: &Context, profile: Option<&str>) -> Result<HostSpine, String> {
    let root = dsh_home_paths::resolve_dsh_home(None, &|name| std::env::var(name).ok());
    compose_persistent_host_at(ctx, root, profile)
}

/// Compose a persistent Host at an explicitly selected DSH home.
/// Embedded launchers use this instead of mutating the process environment.
pub fn compose_persistent_host_at(
    ctx: &Context,
    root: impl Into<std::path::PathBuf>,
    profile: Option<&str>,
) -> Result<HostSpine, String> {
    compose_host_with_root(ctx, Some(root.into()), profile, 0)
}

/// Compose a persistent Host at an explicitly selected home and TCP port.
/// Port zero preserves the OS-assigned test/embedding behavior.
pub fn compose_persistent_host_at_port(
    ctx: &Context,
    root: impl Into<std::path::PathBuf>,
    profile: Option<&str>,
    port: u16,
) -> Result<HostSpine, String> {
    compose_host_with_root(ctx, Some(root.into()), profile, port)
}

fn compose_host_with_root(
    ctx: &Context,
    data_root: Option<std::path::PathBuf>,
    profile: Option<&str>,
    port: u16,
) -> Result<HostSpine, String> {
    let output = Arc::new(parking_lot::Mutex::new(None));
    let fiber = ctx.plugin(
        Arc::new(HostCompositionPlugin {
            output: Arc::clone(&output),
            data_root,
            profile: profile.map(str::to_string),
            port,
        }),
        arc(()),
    );
    if let Err(error) = futures::executor::block_on(fiber.settle()) {
        futures::executor::block_on(fiber.dispose());
        return Err(format!("host composition: {}", error.message()));
    }
    let result = output
        .lock()
        .take()
        .ok_or_else(|| "host composition produced no result".to_string())?;
    match result {
        Ok(spine) => {
            *spine.lifecycle_fiber.lock() = Some(fiber);
            Ok(spine)
        }
        Err(error) => {
            futures::executor::block_on(fiber.dispose());
            Err(error)
        }
    }
}

/// Compose a cloneable host owner for long-running application entrypoints.
pub fn compose_host_handle(ctx: &Context) -> Result<HostHandle, String> {
    Ok(HostHandle {
        spine: Arc::new(compose_host(ctx)?),
    })
}

// DeepSeek's resolver seam intentionally preserves the core LlmError shape;
// boxing it here would make the Host adapter closure incompatible with the
// shared runtime contract.
#[allow(clippy::result_large_err)]
fn compose_host_in_fiber(
    ctx: &Context,
    configured_root: Option<std::path::PathBuf>,
    profile: Option<&str>,
    bind_port: u16,
) -> Result<HostSpine, String> {
    // Package-owned invariant companions run first so every later append is
    // validated.
    let _invariants = InvariantRegistry::new(
        ctx,
        InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let owns_data_root = configured_root.is_none();
    let data_root = configured_root
        .unwrap_or_else(|| std::env::temp_dir().join(format!("dsh-host-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&data_root).map_err(|error| format!("data root: {error}"))?;
    // Own the temporary root immediately. This is the first composition
    // effect, so reverse teardown closes every subsequently installed service
    // before attempting removal. A completed HostSpine takes ownership and
    // removes the root from its explicit shutdown barrier instead.
    let data_root_transferred = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let root_for_cleanup = data_root.clone();
    let transferred_for_cleanup = Arc::clone(&data_root_transferred);
    let cleanup_logger = ctx.named_logger(Some("dsh-host"));
    let _ = ctx.effect(
        "host.data-root",
        Box::pin(async move {
            Some(make_disposer(move || {
                let root = root_for_cleanup.clone();
                let transferred = Arc::clone(&transferred_for_cleanup);
                let logger = cleanup_logger.clone();
                Box::pin(async move {
                    if owns_data_root && !transferred.load(std::sync::atomic::Ordering::SeqCst) {
                        match tokio::fs::remove_dir_all(&root).await {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => logger.error(vec![arc(format!(
                                "failed host composition could not remove data root {}: {error}",
                                root.display()
                            ))]),
                        }
                    }
                })
            }))
        }),
    );
    let sessions_root = data_root.join("sessions");
    let search_path = data_root.join("search.db");

    let sessions = SessionStore::install(ctx);
    let session_projections = dsh_session_projection::SessionProjectionRegistry::install(ctx);
    dsh_session_turn_outline::apply(ctx)
        .map_err(|error| format!("session-turn-outline: {error}"))?;
    let _token_meter = dsh_token_meter::TokenMeter::install(ctx, Default::default());

    let _session_titles = dsh_session_title::SessionTitleService::install_with_registry(
        ctx,
        dsh_session_title::Config {
            fallback_max_words: 8,
            fallback_max_bytes: 96,
            max_title_bytes: 256,
        },
        Some(session_projections.clone()),
    )
    .map_err(|error| format!("session-title: {error}"))?;
    // Persistence and the derived search index are installed before active
    // agent fibers. Cordis disposes effects in reverse order, so agent work is
    // quiescent before the durability barrier and backend close run.
    let persistence = JsonlSessionPersistence::install(
        ctx,
        JsonlConfig {
            root: sessions_root.to_string_lossy().to_string(),
            pack_chunks: true,
            compression: JsonlCompression::Zstd,
            prepared_session_cache_size: 5,
            write_batch_max_delay_ms: 200,
        },
    )
    .map_err(|error| format!("sessionPersistence: {error}"))?;
    let search = SqliteSearch::install(
        ctx,
        &SqliteSearchConfig {
            path: search_path.to_string_lossy().to_string(),
            open_at: Some(dsh_session_query_sqlite::OpenAt::FirstSearch),
            ..Default::default()
        },
    )
    .map_err(|error| format!("sessionQuery: {}", error.message))?;
    let query = ctx
        .get_typed::<Arc<SessionQueryEngine>>("sessionQuery", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "sessionQuery service missing".to_string())?;
    let shutdown_failures = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sessions_for_shutdown = Arc::clone(&sessions);
    let failures_for_shutdown = Arc::clone(&shutdown_failures);
    let _ = ctx.effect(
        "host.persistence-drain-barrier",
        Box::pin(async move {
            Some(make_disposer(move || {
                let sessions = Arc::clone(&sessions_for_shutdown);
                let failures = Arc::clone(&failures_for_shutdown);
                Box::pin(async move {
                    for session in sessions.list() {
                        if let Err(error) = sessions.flush(&session).await {
                            failures.lock().push(format!(
                                "session {} flush failed: {error}",
                                session.id().as_str()
                            ));
                        }
                    }
                })
            }))
        }),
    );

    let agents = AgentRegistry::install(ctx);
    // Execution resources are owner-fiber services. Install the process root
    // first; reverse Cordis teardown then closes terminals/jobs before the
    // subprocess provider drains any remaining trees. The model-code runtime
    // is installed only after its fail-closed OS sandbox is available.
    let subprocess = LocalSubprocessRuntime::install(ctx);
    let sandbox = LocalSandboxProvider::install(ctx, Default::default());
    let _sandbox_policy = SandboxPolicyService::install(
        ctx,
        dsh_sandbox_policy::Config {
            mode: Some(dsh_sandbox::SandboxMode::WorkspaceWrite),
            workspace_root: None,
        },
    );
    let _code_runtime = dsh_code_runtime_node::NodeCodeRuntime::install(
        ctx,
        dsh_code_runtime_node::Config {
            require_os_sandbox: true,
            ..Default::default()
        },
    )
    .map_err(|error| format!("code-runtime-node: {error}"))?;
    let _jobs = LocalJobRegistry::install(ctx, Default::default());
    let terminals = TerminalSessionService::install(ctx);
    let _terminal_shell = dsh_terminal_bash::ShellTerminalBackend::install(ctx, Default::default())
        .map_err(|error| format!("terminal-bash: {error}"))?;
    let _shell = LocalPwshExecutor::install(ctx, Default::default());
    let dsh_home = data_root.clone();
    let settings_storage = Arc::new(JsonSettingsStorage {
        path: dsh_home.join("settings.json"),
        defaults: package_settings_defaults()?,
        document: parking_lot::Mutex::new(indexmap::IndexMap::new()),
    });
    let settings = dsh_settings::SettingsProvider::install(ctx, settings_storage);
    futures::executor::block_on(settings.ready()).map_err(|error| format!("settings: {error}"))?;
    let path_defaults = [
        ("dataDirectory", dsh_home.clone()),
        ("cacheDirectory", dsh_home.join("cache")),
        ("environmentDirectory", dsh_home.join("environments")),
        ("testDirectory", dsh_home.join("test-runs")),
    ];
    let path_properties = path_defaults
        .into_iter()
        .map(|(name, path)| {
            (
                name.to_string(),
                dsh_schemastery::Schema::string().default(dsh_schemastery::Data::String(
                    path.to_string_lossy().into_owned(),
                )),
            )
        })
        .collect();
    settings
        .register(
            ctx,
            dsh_settings::settings_namespace("storage-paths")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(path_properties),
            dsh_settings::SettingsRegisterOptions {
                validate: Some(Arc::new(|value| {
                    let dsh_schemastery::Data::Object(object) = value else {
                        return Err("storage paths must be an object".to_string());
                    };
                    for (name, value) in object {
                        let dsh_schemastery::Data::String(path) = value else {
                            return Err(format!("{name} must be a string"));
                        };
                        if !std::path::Path::new(path).is_absolute() {
                            return Err(format!("{name} must be an absolute path"));
                        }
                    }
                    Ok(())
                })),
                ..Default::default()
            },
        )
        .map_err(|error| format!("settings storage-paths: {error}"))?;
    let computer_use_scope = settings
        .register(
            ctx,
            dsh_settings::settings_namespace("computer-use")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(indexmap::IndexMap::from([
                (
                    "enabled".to_string(),
                    dsh_schemastery::Schema::boolean().default(dsh_schemastery::Data::Bool(false)),
                ),
                (
                    "command".to_string(),
                    dsh_schemastery::Schema::string()
                        .default(dsh_schemastery::Data::String(String::new())),
                ),
                (
                    "timeoutSeconds".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(5.0)
                        .max(300.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(60.0)),
                ),
            ])),
            dsh_settings::SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings computer-use: {error}"))?;
    let voice_scope = settings
        .register(
            ctx,
            dsh_settings::settings_namespace("voice")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(indexmap::IndexMap::from([
                (
                    "sttCommand".to_string(),
                    dsh_schemastery::Schema::string()
                        .default(dsh_schemastery::Data::String(String::new())),
                ),
                (
                    "ttsCommand".to_string(),
                    dsh_schemastery::Schema::string()
                        .default(dsh_schemastery::Data::String(String::new())),
                ),
                (
                    "timeoutSeconds".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(5.0)
                        .max(300.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(60.0)),
                ),
            ])),
            dsh_settings::SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings voice: {error}"))?;
    let memory_scope = settings
        .register(
            ctx,
            dsh_settings::settings_namespace("memory")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(indexmap::IndexMap::from([
                (
                    "enabled".to_string(),
                    dsh_schemastery::Schema::boolean().default(dsh_schemastery::Data::Bool(true)),
                ),
                (
                    "userProfileEnabled".to_string(),
                    dsh_schemastery::Schema::boolean().default(dsh_schemastery::Data::Bool(true)),
                ),
                (
                    "memoryBudget".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(256.0)
                        .max(20_000.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(2200.0)),
                ),
                (
                    "profileBudget".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(128.0)
                        .max(20_000.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(1375.0)),
                ),
                (
                    "provider".to_string(),
                    dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                        "builtin".to_string(),
                    ))
                    .default(dsh_schemastery::Data::String("builtin".to_string())),
                ),
                (
                    "contextEngine".to_string(),
                    dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                        "compressor".to_string(),
                    ))
                    .default(dsh_schemastery::Data::String("compressor".to_string())),
                ),
                (
                    "autoCompact".to_string(),
                    dsh_schemastery::Schema::boolean().default(dsh_schemastery::Data::Bool(true)),
                ),
                (
                    "compactThreshold".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(0.1)
                        .max(0.95)
                        .step(0.05)
                        .default(dsh_schemastery::Data::Number(0.5)),
                ),
                (
                    "compactTarget".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(0.05)
                        .max(0.9)
                        .step(0.05)
                        .default(dsh_schemastery::Data::Number(0.2)),
                ),
                (
                    "protectRecentMessages".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(1.0)
                        .max(200.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(20.0)),
                ),
            ])),
            dsh_settings::SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings memory: {error}"))?;
    let security_scope = settings
        .register(
            ctx,
            dsh_settings::settings_namespace("security")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(indexmap::IndexMap::from([
                (
                    "approvalTimeoutSeconds".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(5.0)
                        .max(300.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(30.0)),
                ),
                (
                    "unattendedPolicy".to_string(),
                    dsh_schemastery::Schema::union(vec![
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "deny".to_string(),
                        )),
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "allow-safe-only".to_string(),
                        )),
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "allow-all".to_string(),
                        )),
                    ])
                    .default(dsh_schemastery::Data::String("deny".to_string())),
                ),
                (
                    "riskToolPolicy".to_string(),
                    dsh_schemastery::Schema::union(vec![
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "ask".to_string(),
                        )),
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "deny".to_string(),
                        )),
                    ])
                    .default(dsh_schemastery::Data::String("ask".to_string())),
                ),
                (
                    "outsideWritePolicy".to_string(),
                    dsh_schemastery::Schema::union(vec![
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "ask-directory".to_string(),
                        )),
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "ask-every-time".to_string(),
                        )),
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "deny".to_string(),
                        )),
                    ])
                    .default(dsh_schemastery::Data::String("ask-directory".to_string())),
                ),
                (
                    "sensitiveReadPolicy".to_string(),
                    dsh_schemastery::Schema::union(vec![
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "ask".to_string(),
                        )),
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "deny".to_string(),
                        )),
                    ])
                    .default(dsh_schemastery::Data::String("ask".to_string())),
                ),
                (
                    "credentialShellPolicy".to_string(),
                    dsh_schemastery::Schema::union(vec![
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "strict".to_string(),
                        )),
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "ask".to_string(),
                        )),
                    ])
                    .default(dsh_schemastery::Data::String("strict".to_string())),
                ),
            ])),
            dsh_settings::SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings security: {error}"))?;
    // Subagent defaults namespace: wires the "子智能体" settings section.
    // Fields map to child agent resolution (provider/model/reasoning/maxTokens),
    // the loop's parallel-tool-call cap, and the delegation depth cap. Fields
    // that the runtime does not yet consume are kept so the UI can round-trip
    // them without validation loss.
    let subagent_scope = settings
        .register(
            ctx,
            dsh_settings::settings_namespace("subagent")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(indexmap::IndexMap::from([
                (
                    "defaultProvider".to_string(),
                    dsh_schemastery::Schema::string()
                        .default(dsh_schemastery::Data::String(String::new())),
                ),
                (
                    "defaultModel".to_string(),
                    dsh_schemastery::Schema::string()
                        .default(dsh_schemastery::Data::String(String::new())),
                ),
                (
                    "defaultReasoningEffort".to_string(),
                    dsh_schemastery::Schema::string()
                        .default(dsh_schemastery::Data::String(String::new())),
                ),
                (
                    "defaultMaxTokens".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(0.0)
                        .max(1_000_000.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(0.0)),
                ),
                (
                    "maxTurns".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(1.0)
                        .max(10_000.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(250.0)),
                ),
                (
                    "maxParallel".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(1.0)
                        .max(64.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(10.0)),
                ),
                (
                    "maxDepth".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(0.0)
                        .max(32.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(0.0)),
                ),
                (
                    "timeoutSeconds".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(0.0)
                        .max(86_400.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(0.0)),
                ),
                (
                    "toolCallMode".to_string(),
                    dsh_schemastery::Schema::union(vec![
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "auto".to_string(),
                        )),
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "code".to_string(),
                        )),
                        dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                            "native".to_string(),
                        )),
                    ])
                    .default(dsh_schemastery::Data::String("auto".to_string())),
                ),
                (
                    "serviceTier".to_string(),
                    dsh_schemastery::Schema::string()
                        .default(dsh_schemastery::Data::String(String::new())),
                ),
                (
                    "apiRetryCount".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(0.0)
                        .max(20.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(3.0)),
                ),
            ])),
            dsh_settings::SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings subagent: {error}"))?;
    let theme_preference_schema = dsh_schemastery::Schema::union(
        [
            "light",
            "dark",
            "blue-fantasy",
            "harbor",
            "xp",
            "minecraft",
            "trading",
            "miku",
            "deepseek-official",
        ]
        .into_iter()
        .map(|choice| {
            dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(choice.to_string()))
        })
        .collect(),
    )
    .default(dsh_schemastery::Data::String("light".to_string()));
    settings
        .register(
            ctx,
            dsh_settings::settings_namespace("ui-theme")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(indexmap::IndexMap::from([
                ("preference".to_string(), theme_preference_schema),
                (
                    "fontSize".to_string(),
                    dsh_schemastery::Schema::number()
                        .min(12.0)
                        .max(17.0)
                        .step(1.0)
                        .default(dsh_schemastery::Data::Number(14.0)),
                ),
            ])),
            dsh_settings::SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings ui-theme: {error}"))?;
    for (namespace, field, choices, default) in [
        ("locale", "preference", &["zh", "en"][..], None),
        (
            "ui-conversation",
            "busyEnter",
            &["queue", "steer"][..],
            Some("queue"),
        ),
    ] {
        let choice_schema = dsh_schemastery::Schema::union(
            choices
                .iter()
                .map(|choice| {
                    dsh_schemastery::Schema::constant(dsh_schemastery::Data::String(
                        (*choice).to_string(),
                    ))
                })
                .collect(),
        );
        let choice_schema = match default {
            Some(default) => {
                choice_schema.default(dsh_schemastery::Data::String(default.to_string()))
            }
            None => choice_schema,
        };
        settings
            .register(
                ctx,
                dsh_settings::settings_namespace(namespace)
                    .map_err(|error| format!("settings namespace: {error}"))?,
                dsh_schemastery::Schema::object(indexmap::IndexMap::from([(
                    field.to_string(),
                    choice_schema,
                )])),
                dsh_settings::SettingsRegisterOptions::default(),
            )
            .map_err(|error| format!("settings {namespace}: {error}"))?;
    }
    settings
        .register(
            ctx,
            dsh_settings::settings_namespace("ui-wallpaper")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(indexmap::IndexMap::from([(
                "bingDaily".to_string(),
                dsh_schemastery::Schema::boolean().default(dsh_schemastery::Data::Bool(false)),
            )])),
            dsh_settings::SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings ui-wallpaper: {error}"))?;
    let llm = LlmRuntime::install(ctx);
    dsh_session_title_first_prompt_llm::apply(
        ctx,
        dsh_session_title_first_prompt_llm::Config {
            target_words: 5,
            target_cjk_characters: 10,
            max_input_bytes: 4_096,
            max_output_tokens: 32,
            timeout_ms: 15_000,
            provider: None,
            model: None,
        },
    )
    .map_err(|error| format!("session-title-first-prompt-llm: {error}"))?;
    for settings_ns in ["llm-deepseek", "llm-pi-ai"] {
        let discovery = Arc::new(|request: &dsh_llm::LlmModelDiscoveryRequest| {
            let request = request.clone();
            Box::pin(async move { discover_openai_compatible_models(request).await })
                as cordis::BoxFuture<'static, Result<Vec<dsh_llm::LlmDiscoveredModel>, String>>
        });
        let _discovery = llm
            .register_model_discovery(ctx, settings_ns, discovery)
            .map_err(|error| format!("{settings_ns} discovery: {error}"))?;
    }
    let _attachments = dsh_attachment_local::LocalAttachmentStore::install(
        ctx,
        dsh_attachment_local::Config {
            dsh_home: Some(dsh_home.to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    let credentials = dsh_credentials_local::LocalCredentialProvider::install(
        ctx,
        dsh_credentials_local::Config {
            dsh_home: Some(dsh_home.to_string_lossy().into_owned()),
            // The browser is the authoritative writer. Starting with no
            // credentials document must not fail while trying to watch a path
            // that does not exist yet.
            watch: Some(false),
            ..Default::default()
        },
    )
    .map_err(|error| format!("credentials-local: {error}"))?;
    let mut deepseek_settings_properties = indexmap::IndexMap::new();
    deepseek_settings_properties.insert(
        "apiKeyEnv".to_string(),
        dsh_schemastery::Schema::string()
            .role("credential-ref", None)
            .default(dsh_schemastery::Data::String(
                dsh_llm_deepseek::DEFAULT_API_KEY_ENV.to_string(),
            )),
    );
    deepseek_settings_properties.insert(
        "baseURL".to_string(),
        dsh_schemastery::Schema::string().default(dsh_schemastery::Data::String(
            dsh_llm_deepseek::PUBLIC_BASE_URL.to_string(),
        )),
    );
    let deepseek_scope = settings
        .register(
            ctx,
            dsh_settings::settings_namespace("llm-deepseek")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(deepseek_settings_properties),
            dsh_settings::SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings llm-deepseek: {error}"))?;
    let _deepseek_directory = llm
        .register_configurable_providers(
            ctx,
            vec![dsh_llm::LlmConfigurableProvider {
                provider: dsh_llm_deepseek::PROVIDER.to_string(),
                display_name: "DeepSeek".to_string(),
                settings_ns: "llm-deepseek".to_string(),
                settings_path: Vec::new(),
                declared: None,
            }],
        )
        .map_err(|error| format!("llm-deepseek directory: {error}"))?;
    let web = dsh_web::WebRuntime::install(
        ctx,
        dsh_web::Config {
            search_provider: Some("deepseek-official".to_string()),
        },
    );
    let web_credentials = credentials.clone();
    let web_provider = Arc::new(dsh_web_search_deepseek::DeepSeekSearchProvider::new(
        dsh_web_search_deepseek::Options {
            api_key: None,
            resolve_api_key: Some(Arc::new(move || {
                let credentials = web_credentials.clone();
                Box::pin(async move {
                    let reference = dsh_credentials::credential_ref("DEEPSEEK_API_KEY");
                    Ok(credentials
                        .resolve(&reference)
                        .await
                        .map(|resolved| resolved.value))
                })
            })),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            base_url: "https://api.deepseek.com/anthropic/v1".to_string(),
            model: "deepseek-v4-flash".to_string(),
            api_version: "2023-06-01".to_string(),
            max_tokens: 4096,
            max_uses: 5,
            record_request: None,
        },
    ));
    let _web_provider = web
        .register_search_provider(web_provider)
        .map_err(|error| format!("web-search-deepseek: {error}"))?;

    let deepseek_scope_for_options = deepseek_scope.clone();
    let deepseek_credentials = credentials.clone();
    let deepseek_attachment_ctx = ctx.clone();
    let deepseek_adapter = Arc::new(dsh_llm_deepseek::DeepSeekAdapter::new(
        dsh_llm_deepseek::DeepSeekAdapterOptions {
            options: Arc::new(move || {
                let value = (deepseek_scope_for_options.get)()
                    .to_json()
                    .ok_or_else(|| {
                        dsh_llm::LlmError::new(
                            "llm-deepseek settings are not JSON-compatible",
                            "INVALID_CONFIG",
                            dsh_llm::LlmErrorOptions::default(),
                        )
                    })?;
                let api_key_env = value
                    .get("apiKeyEnv")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let base_url = value
                    .get("baseURL")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .or_else(|| std::env::var("DSH_DEEPSEEK_BASE_URL").ok());
                dsh_llm_deepseek::resolve_adapter_options(&dsh_llm_deepseek::DeepSeekConfig {
                    api_key_env,
                    base_url,
                    ..Default::default()
                })
            }),
            resolve_api_key: Arc::new(move |snapshot| {
                let credentials = deepseek_credentials.clone();
                let api_key_env = snapshot.api_key_env.clone();
                Box::pin(async move {
                    let reference = dsh_credentials::credential_ref(&api_key_env);
                    Ok(credentials
                        .resolve(&reference)
                        .await
                        .map(|resolved| resolved.value))
                })
            }),
            resolve_attachments: Some(Arc::new(move || {
                deepseek_attachment_ctx
                    .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false)
                    .map(|slot| slot.as_ref().clone())
            })),
            provider_name: None,
            reasoning_wire_format: dsh_llm_deepseek::ReasoningWireFormat::DeepSeek,
        },
    ));
    let _deepseek_registration = dsh_llm_deepseek::apply(ctx, &llm, deepseek_adapter)
        .map_err(|error| format!("llm-deepseek: {}", error.failure.message))?;

    let default_model = dsh_agent_default_model::AgentDefaultModelConfigService::install(
        ctx,
        dsh_agent_default_model::AgentDefaultModelConfig {
            provider: dsh_llm_deepseek::PROVIDER.to_string(),
            model: "deepseek-v4-flash".to_string(),
        },
    );
    futures::executor::block_on(default_model.ready())
        .map_err(|error| format!("agent-default-model ready: {error}"))?;

    let pi_scope = settings
        .register(
            ctx,
            dsh_settings::settings_namespace("llm-pi-ai")
                .map_err(|error| format!("settings namespace: {error}"))?,
            openai_compatible_schema(),
            dsh_settings::SettingsRegisterOptions {
                validate: Some(Arc::new(|value| openai_profiles(value).map(|_| ()))),
                ..Default::default()
            },
        )
        .map_err(|error| format!("settings llm-pi-ai: {error}"))?;
    let initial_pi = openai_profiles(&(pi_scope.get)())?;
    let pi_profiles = Arc::new(parking_lot::Mutex::new(initial_pi.providers));
    let pi_adapter: Arc<dyn dsh_llm::LlmAdapter> = Arc::new(OpenAiCompatibleAdapter {
        profiles: pi_profiles.clone(),
        credentials: credentials.clone(),
        attachment_ctx: ctx.clone(),
    });
    let initial_routes: Vec<String> = pi_profiles.lock().keys().cloned().collect();
    let pi_registration = llm
        .register_adapter(ctx, initial_routes, pi_adapter.clone())
        .map_err(|error| format!("llm-pi-ai: {error}"))?;
    let initial_entries: Vec<dsh_llm::LlmConfigurableProvider> = pi_profiles
        .lock()
        .iter()
        .map(|(provider, profile)| dsh_llm::LlmConfigurableProvider {
            provider: provider.clone(),
            display_name: profile
                .display_name
                .clone()
                .unwrap_or_else(|| provider.clone()),
            settings_ns: "llm-pi-ai".to_string(),
            settings_path: vec!["providers".to_string(), provider.clone()],
            declared: Some(true),
        })
        .collect();
    let pi_directory = llm
        .register_configurable_providers(ctx, initial_entries)
        .map_err(|error| format!("llm-pi-ai directory: {error}"))?;
    let _pi_watch = (pi_scope.watch)(Arc::new(move |next, _previous| {
        let parsed = openai_profiles(next);
        let profiles = pi_profiles.clone();
        let registration_replace = pi_registration.replace.clone();
        let directory_replace = pi_directory.replace.clone();
        if let Ok(parsed) = parsed {
            let routes: Vec<String> = parsed.providers.keys().cloned().collect();
            let entries = parsed
                .providers
                .iter()
                .map(|(provider, profile)| dsh_llm::LlmConfigurableProvider {
                    provider: provider.clone(),
                    display_name: profile
                        .display_name
                        .clone()
                        .unwrap_or_else(|| provider.clone()),
                    settings_ns: "llm-pi-ai".to_string(),
                    settings_path: vec!["providers".to_string(), provider.clone()],
                    declared: Some(true),
                })
                .collect::<Vec<_>>();
            let previous = {
                let mut current = profiles.lock();
                std::mem::replace(&mut *current, parsed.providers)
            };
            if (registration_replace)(routes).is_err() || (directory_replace)(entries).is_err() {
                *profiles.lock() = previous;
            }
        }
        Box::pin(async {})
    }));
    let system_prompt = SystemPrompt::install(ctx, dsh_system_prompt::Config::default())
        .map_err(|error| format!("systemPrompt: {error}"))?;
    let _harness_source = system_prompt.section(
        ctx,
        PromptSection {
            name: "harness:source".to_string(),
            order: -99.0,
            text: PromptText::Static(format!(
                "The DeepSeek Harness implementation checkout is at {}. The checkout location and current working directory are separate values and may differ; never infer the working directory from this path. Use pwd to determine the current working directory. Use this checkout only to inspect or extend DSH itself.",
                env!("CARGO_MANIFEST_DIR"),
            )),
            complete: None,
        },
    );
    let _zh_visible_output = system_prompt.section(
        ctx,
        PromptSection {
            name: "language:zh-visible-output".to_string(),
            order: 5.0,
            text: PromptText::Static(
                "所有用户可见的推理摘要、计划、进度、错误说明和最终答案必须从第一个字开始使用简体中文；代码、文件名、命令和必要技术术语除外。"
                    .to_string(),
            ),
            complete: None,
        },
    );
    let sandbox_policy_for_context = Arc::clone(&_sandbox_policy);
    let sessions_for_context = Arc::clone(&sessions);
    let _sandbox_context = system_prompt.context(
        ctx,
        dsh_system_prompt::PromptContext {
            name: "sandbox:policy".to_string(),
            order: 110.0,
            text: PromptText::Provider(Arc::new(move |assembly| {
                let session = assembly
                    .field_str("sessionId")
                    .and_then(|id| sessions_for_context.get(&dsh_session::session_id(id)));
                let workspace = workspace_context_text(
                    session
                        .as_ref()
                        .and_then(|session| session.header().cwd.as_deref()),
                );
                let execution =
                    sandbox_policy_for_context.resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
                        session: session.map(Arc::new),
                        mode: None,
                    });
                format!(
                    "{workspace}\nCurrent DSH file policy: {}. The workspace boundary is {}.",
                    execution.mode.as_str(),
                    display_workspace_path(&execution.workspace_root)
                )
            })),
        },
    );
    let security_policy_state: dsh_tools::SecurityPolicyState =
        Arc::new(parking_lot::RwLock::new({
            let value = (security_scope.get)();
            let object = match value {
                dsh_schemastery::Data::Object(object) => object,
                _ => indexmap::IndexMap::new(),
            };
            let text = |key: &str| match object.get(key) {
                Some(dsh_schemastery::Data::String(value)) => value.as_str(),
                _ => "",
            };
            dsh_tools::SecurityPolicyConfig {
                risk_tool_policy: if text("riskToolPolicy") == "deny" {
                    dsh_tools::RiskToolPolicy::Deny
                } else {
                    dsh_tools::RiskToolPolicy::Ask
                },
                outside_write_policy: match text("outsideWritePolicy") {
                    "deny" => dsh_tools::OutsideWritePolicy::Deny,
                    "ask-every-time" => dsh_tools::OutsideWritePolicy::AskEveryTime,
                    _ => dsh_tools::OutsideWritePolicy::AskDirectory,
                },
                sensitive_read_policy: if text("sensitiveReadPolicy") == "deny" {
                    dsh_tools::SensitiveReadPolicy::Deny
                } else {
                    dsh_tools::SensitiveReadPolicy::Ask
                },
                credential_shell_policy: if text("credentialShellPolicy") == "ask" {
                    dsh_tools::CredentialShellPolicy::Ask
                } else {
                    dsh_tools::CredentialShellPolicy::Strict
                },
            }
        }));
    let watched_security_policy = Arc::clone(&security_policy_state);
    let _security_policy_watch = (security_scope.watch)(Arc::new(move |next, _previous| {
        if let dsh_schemastery::Data::Object(object) = next {
            let text = |key: &str| match object.get(key) {
                Some(dsh_schemastery::Data::String(value)) => value.as_str(),
                _ => "",
            };
            *watched_security_policy.write() = dsh_tools::SecurityPolicyConfig {
                risk_tool_policy: if text("riskToolPolicy") == "deny" {
                    dsh_tools::RiskToolPolicy::Deny
                } else {
                    dsh_tools::RiskToolPolicy::Ask
                },
                outside_write_policy: match text("outsideWritePolicy") {
                    "deny" => dsh_tools::OutsideWritePolicy::Deny,
                    "ask-every-time" => dsh_tools::OutsideWritePolicy::AskEveryTime,
                    _ => dsh_tools::OutsideWritePolicy::AskDirectory,
                },
                sensitive_read_policy: if text("sensitiveReadPolicy") == "deny" {
                    dsh_tools::SensitiveReadPolicy::Deny
                } else {
                    dsh_tools::SensitiveReadPolicy::Ask
                },
                credential_shell_policy: if text("credentialShellPolicy") == "ask" {
                    dsh_tools::CredentialShellPolicy::Ask
                } else {
                    dsh_tools::CredentialShellPolicy::Strict
                },
            };
        }
        Box::pin(async {})
    }));
    let tools = ToolRuntime::install(
        ctx,
        dsh_tools::Config {
            mode: None,
            max_parallel_sub_calls: None,
        },
    )
    .map_err(|error| format!("tools: {error}"))?;
    dsh_tools::install_security_policy(ctx, security_policy_state);
    let install_timeout_policy = dsh_timeout_policy::apply(ctx);
    futures::executor::block_on(install_timeout_policy());
    let _fs = dsh_fs_local::LocalFileSystem::install(
        ctx,
        dsh_fs_local::Config {
            cwd: None,
            diff_basis_max_bytes: None,
        },
    )
    .map_err(|error| format!("fs-local: {error}"))?;
    let _skills = dsh_skill::SkillRegistry::install(ctx, Default::default())
        .map_err(|error| format!("skills: {error}"))?;
    let _skill_badge = dsh_skill_badge::apply(ctx);
    if matches!(
        (memory_scope.get)(),
        dsh_schemastery::Data::Object(ref object)
            if matches!(object.get("enabled"), Some(dsh_schemastery::Data::Bool(true)))
    ) {
        let memory_root = data_root.join("memory");
        dsh_tool_memory_local::install(ctx, memory_root.clone())
            .map_err(|error| format!("memory: {error}"))?;
        let sessions_for_memory = Arc::clone(&sessions);
        let memory_scope_for_context = memory_scope.clone();
        let _memory_context = system_prompt.context(
            ctx,
            dsh_system_prompt::PromptContext {
                name: "memory:entries".to_string(),
                order: 105.0,
                text: PromptText::Provider(Arc::new(move |assembly| {
                    let preset = assembly
                        .field_str("sessionId")
                        .and_then(|id| sessions_for_memory.get(&dsh_session::session_id(id)))
                        .and_then(|session| session.header().agent_preset.clone())
                        .unwrap_or_else(|| "default".to_string());
                    let budget = match (memory_scope_for_context.get)() {
                        dsh_schemastery::Data::Object(object) => match object.get("memoryBudget") {
                            Some(dsh_schemastery::Data::Number(value)) => *value as usize,
                            _ => 2200,
                        },
                        _ => 2200,
                    };
                    dsh_tool_memory_local::render_enabled_file(&memory_root, &preset, budget)
                })),
            },
        );
    }
    if let dsh_schemastery::Data::Object(object) = (voice_scope.get)() {
        let stt_command = match object.get("sttCommand") {
            Some(dsh_schemastery::Data::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            _ => None,
        };
        let tts_command = match object.get("ttsCommand") {
            Some(dsh_schemastery::Data::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            _ => None,
        };
        let timeout_ms = match object.get("timeoutSeconds") {
            Some(dsh_schemastery::Data::Number(value)) => (*value as u64) * 1_000,
            _ => 60_000,
        };
        dsh_tool_voice_command::install(
            ctx,
            dsh_tool_voice_command::Config {
                stt_command,
                tts_command,
                timeout_ms,
            },
        )
        .map_err(|error| format!("voice: {error}"))?;
    }
    if let dsh_schemastery::Data::Object(object) = (computer_use_scope.get)()
        && matches!(
            object.get("enabled"),
            Some(dsh_schemastery::Data::Bool(true))
        )
    {
        let command = match object.get("command") {
            Some(dsh_schemastery::Data::String(value)) => value.trim().to_string(),
            _ => String::new(),
        };
        let timeout_ms = match object.get("timeoutSeconds") {
            Some(dsh_schemastery::Data::Number(value)) => (*value as u64) * 1_000,
            _ => 60_000,
        };
        dsh_tool_computer_use_command::install(
            ctx,
            dsh_tool_computer_use_command::Config {
                command,
                timeout_ms,
            },
        )
        .map_err(|error| format!("computer-use: {error}"))?;
    }
    dsh_tool_terminal::ToolTerminalService::install(ctx)
        .map_err(|error| format!("tool-terminal: {error}"))?;
    let _subagents = dsh_subagent::SubagentRuntime::install(ctx);
    dsh_subagent_spawn_in_process::apply(ctx, &Default::default())
        .map_err(|error| format!("subagent-spawn: {}", error.message))?;
    dsh_subagent_fork_in_process::apply(ctx, &Default::default())
        .map_err(|error| format!("subagent-fork: {}", error.message))?;
    dsh_subagent_codex::apply(ctx, &Default::default())
        .map_err(|error| format!("subagent-codex: {error}"))?;
    dsh_subagent_claude_code::apply(ctx, &Default::default())
        .map_err(|error| format!("subagent-claude-code: {error}"))?;
    // Bridge the `subagent` settings namespace into the child resolver as a
    // live snapshot service. Re-read on every `settings/updated` event so a
    // user change takes effect for the next child without a restart.
    let subagent_defaults = {
        use dsh_schemastery::Data;
        use futures::FutureExt;
        let read = |scope: &dsh_settings::SettingsScope| {
            dsh_subagent::SubagentDefaults::from_strings(
                match (scope.get)() {
                    Data::Object(ref o) => match o.get("defaultProvider") {
                        Some(Data::String(s)) => s.as_str(),
                        _ => "",
                    },
                    _ => "",
                },
                match (scope.get)() {
                    Data::Object(ref o) => match o.get("defaultModel") {
                        Some(Data::String(s)) => s.as_str(),
                        _ => "",
                    },
                    _ => "",
                },
                match (scope.get)() {
                    Data::Object(ref o) => match o.get("defaultReasoningEffort") {
                        Some(Data::String(s)) => s.as_str(),
                        _ => "",
                    },
                    _ => "",
                },
                match (scope.get)() {
                    Data::Object(ref o) => match o.get("defaultMaxTokens") {
                        Some(Data::Number(n)) => *n,
                        _ => 0.0,
                    },
                    _ => 0.0,
                },
                match (scope.get)() {
                    Data::Object(ref o) => match o.get("maxDepth") {
                        Some(Data::Number(n)) => *n,
                        _ => 0.0,
                    },
                    _ => 0.0,
                },
                match (scope.get)() {
                    Data::Object(ref o) => match o.get("maxTurns") {
                        Some(Data::Number(n)) => *n,
                        _ => 0.0,
                    },
                    _ => 0.0,
                },
                match (scope.get)() {
                    Data::Object(ref o) => match o.get("timeoutSeconds") {
                        Some(Data::Number(n)) => *n,
                        _ => 0.0,
                    },
                    _ => 0.0,
                },
            )
        };
        let defaults = Arc::new(read(&subagent_scope));
        ctx.register_service(defaults.clone());
        let bridge_scope = subagent_scope.clone();
        let bridge_defaults = defaults.clone();
        let listener: Arc<cordis::Listener> =
            Arc::new(move |_ctx: &Context, args: Vec<cordis::ArcValue>| {
                let is_subagent = args
                    .first()
                    .and_then(|v| v.downcast_ref::<dsh_settings::SettingsNamespace>())
                    .is_some_and(|ns| ns.as_str() == "subagent");
                if is_subagent {
                    bridge_defaults.update(read(&bridge_scope));
                }
                async move { None }.boxed()
            });
        let _ = futures::executor::block_on(ctx.on(
            "settings/updated",
            listener,
            cordis::EventOptions::default(),
        ));
        defaults
    };
    let _ = subagent_defaults;
    let agent_loop = AgentLoop::install(ctx, dsh_agent_loop::Config::default())
        .map_err(|error| format!("agentLoop: {error}"))?;
    let commands = CommandRuntime::install(ctx);
    let _feedback_command =
        dsh_command_feedback::apply(ctx).map_err(|error| format!("command-feedback: {error}"))?;
    let goals = GoalService::install(ctx, dsh_goal::Config::default());
    let _goal_round_driver =
        dsh_goal_round_driver::apply(ctx).map_err(|error| format!("goal-round-driver: {error}"))?;
    let _goal_command =
        dsh_command_goal::apply(ctx).map_err(|error| format!("command-goal: {error}"))?;
    let questions = UserQuestionService::install(ctx);
    let approval_timeout_ms = match (security_scope.get)() {
        dsh_schemastery::Data::Object(object) => {
            object
                .get("approvalTimeoutSeconds")
                .and_then(|value| match value {
                    dsh_schemastery::Data::Number(value) => Some((*value as u64) * 1_000),
                    _ => None,
                })
        }
        _ => None,
    };
    let approval = ApprovalService::install(
        ctx,
        dsh_user_approval::Config {
            policy: None,
            timeout_ms: approval_timeout_ms,
        },
    );
    let read_approval_runtime = |value: &dsh_schemastery::Data| {
        let object = match value {
            dsh_schemastery::Data::Object(object) => object,
            _ => return (30_000, dsh_user_approval::UnattendedPolicy::Deny),
        };
        let timeout = match object.get("approvalTimeoutSeconds") {
            Some(dsh_schemastery::Data::Number(value)) => (*value as u64) * 1_000,
            _ => 30_000,
        };
        let unattended = match object.get("unattendedPolicy") {
            Some(dsh_schemastery::Data::String(value)) if value == "allow-all" => {
                dsh_user_approval::UnattendedPolicy::AllowAll
            }
            Some(dsh_schemastery::Data::String(value)) if value == "allow-safe-only" => {
                dsh_user_approval::UnattendedPolicy::AllowSafeOnly
            }
            _ => dsh_user_approval::UnattendedPolicy::Deny,
        };
        (timeout, unattended)
    };
    let (runtime_timeout, runtime_unattended) = read_approval_runtime(&(security_scope.get)());
    approval.set_runtime_options(runtime_timeout, runtime_unattended);
    let watched_approval = Arc::clone(&approval);
    let _approval_settings_watch = (security_scope.watch)(Arc::new(move |next, _previous| {
        let (timeout, unattended) = read_approval_runtime(next);
        watched_approval.set_runtime_options(timeout, unattended);
        async move {}.boxed()
    }));
    let permission_presets =
        dsh_permission_presets::PermissionPresetService::install(ctx, Default::default())
            .map_err(|error| format!("permission-presets: {error}"))?;
    futures::executor::block_on(permission_presets.ready())
        .map_err(|error| format!("permission-presets ready: {error}"))?;
    dsh_schedule::apply(ctx);
    // ---- M6 shell: the web face over the spine ----
    // The loader service anchors the plugin inventory and profile
    // composition (the Rust static registry serves empty for now).
    let loader = futures::executor::block_on(dsh_cordis_loader::LoaderService::new(ctx));
    enum PresetBuiltinPlugin {
        Persona,
        AgentInstructions { dsh_home: std::path::PathBuf },
        Pwsh,
        WorkflowEngine,
        Workflow,
        Ralph,
    }
    #[async_trait::async_trait]
    impl cordis::Plugin for PresetBuiltinPlugin {
        async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
            match self {
                Self::Persona => {
                    let text = config
                        .downcast_ref::<serde_json::Value>()
                        .and_then(|value| value.get("text"))
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            PluginError::new(arc("persona requires config.text".to_string()))
                        })?;
                    let prompt = ctx
                        .get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
                        .map(|slot| slot.as_ref().clone())
                        .ok_or_else(|| {
                            PluginError::new(arc("persona requires systemPrompt".to_string()))
                        })?;
                    let disposer = prompt.section(
                        ctx,
                        PromptSection {
                            name: dsh_system_prompt::PERSONA_SECTION.to_string(),
                            order: dsh_system_prompt::PERSONA_ORDER,
                            text: PromptText::Static(text.to_string()),
                            complete: None,
                        },
                    );
                    let _ = ctx.effect("persona", Box::pin(async move { Some(disposer) }));
                }
                Self::AgentInstructions { dsh_home } => {
                    let max_bytes = config
                        .downcast_ref::<serde_json::Value>()
                        .and_then(|value| value.get("maxBytes"))
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(65_536) as usize;
                    let disposer = dsh_agent_instructions::apply(
                        ctx,
                        dsh_agent_instructions::Config {
                            dsh_home: dsh_home.clone(),
                            max_bytes,
                            max_source_bytes: 1024 * 1024,
                        },
                    );
                    let _ = ctx.effect(
                        "agent-instructions",
                        Box::pin(async move { Some(disposer) }),
                    );
                }
                Self::Pwsh => {
                    dsh_tool_pwsh::ToolPwshService::install(ctx)
                        .map_err(|error| PluginError::new(arc(error)))?;
                }
                Self::WorkflowEngine => {
                    let workflow =
                        dsh_workflow_node::NodeWorkflowEngine::install(ctx, Default::default())
                            .map_err(|error| PluginError::new(arc(error)))?;
                    let service: Arc<dyn dsh_workflow::WorkflowEngine> = workflow.clone();
                    ctx.register_service(service);
                    let _ = ctx.effect(
                        "workflow-node",
                        Box::pin(async move {
                            Some(make_disposer(move || {
                                let workflow = workflow.clone();
                                Box::pin(async move { workflow.dispose().await })
                            }))
                        }),
                    );
                }
                Self::Workflow => {
                    let disposer = dsh_tool_workflow::apply(ctx)
                        .map_err(|error| PluginError::new(arc(error)))?;
                    let _ = ctx.effect("tool-workflow", Box::pin(async move { Some(disposer) }));
                }
                Self::Ralph => {
                    let disposer = dsh_tool_ralph::apply(ctx, &Default::default())
                        .map_err(|error| PluginError::new(arc(error)))?;
                    let _ = ctx.effect("tool-ralph", Box::pin(async move { Some(disposer) }));
                }
            }
            Ok(())
        }
    }
    struct NoopPlugin;
    #[async_trait::async_trait]
    impl cordis::Plugin for NoopPlugin {
        fn name(&self) -> Option<&'static str> {
            Some("noop")
        }
        fn inject(&self) -> cordis::InjectSpec {
            cordis::InjectSpec::new([])
        }
        async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
            Ok(())
        }
    }
    loader.core.register("noop", Arc::new(NoopPlugin));
    loader.core.register(
        "@deepseek-ai/dsh-persona",
        Arc::new(PresetBuiltinPlugin::Persona),
    );
    loader.core.register(
        "@deepseek-ai/dsh-agent-instructions",
        Arc::new(PresetBuiltinPlugin::AgentInstructions {
            dsh_home: dsh_home.clone(),
        }),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-pwsh",
        Arc::new(PresetBuiltinPlugin::Pwsh),
    );
    loader.core.register(
        "@deepseek-ai/dsh-agent-tool-presentation",
        Arc::new(dsh_agent_tool_presentation::ToolPresentationPlugin {
            config: dsh_agent_tool_presentation::Config {
                mode: dsh_tools::ToolPresentationMode::Code,
            },
        }),
    );
    loader.core.register(
        "host-plugin-inventory",
        Arc::new(dsh_host_plugin_inventory::PluginInventoryGatewayPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-jobs",
        Arc::new(dsh_tool_jobs::ToolJobsPlugin::new()),
    );
    loader.core.register(
        "@deepseek-ai/dsh-skill-filesystem",
        Arc::new(dsh_skill_filesystem::SkillFilesystemPlugin::new(
            Default::default(),
        )),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-skill",
        Arc::new(dsh_tool_skill::ToolSkillPlugin::new(Default::default())),
    );
    loader.core.register(
        "@deepseek-ai/dsh-plan-mode",
        Arc::new(dsh_plan_mode::PlanModePlugin::new(Default::default())),
    );
    loader.core.register(
        "@deepseek-ai/dsh-compaction-basic",
        Arc::new(dsh_compaction::basic::BasicCompactionPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-command-compact",
        Arc::new(dsh_command_compact::CommandCompactPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-compaction-tool-result-pruner",
        dsh_compaction_tool_result_pruner::plugin(),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-fs",
        Arc::new(dsh_tool_fs::ToolFsPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-fs-search",
        Arc::new(dsh_tool_fs_search::ToolFsSearchPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-str-replace-editor",
        Arc::new(dsh_tool_str_replace_editor::ToolStrReplaceEditorPlugin::new()),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-fs-search",
        Arc::new(dsh_tool_fs_search::ToolFsSearchPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-goal",
        Arc::new(dsh_tool_goal::ToolGoalPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-ask-user",
        Arc::new(dsh_tool_ask_user::ToolAskUserPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-todo",
        Arc::new(dsh_tool_todo::ToolTodoPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-web",
        Arc::new(dsh_tool_web::ToolWebPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-subagent-control",
        Arc::new(dsh_tool_subagent_control::ToolSubagentControlPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-subagent-control/list-agents",
        Arc::new(dsh_tool_subagent_control::list_agents::ToolSubagentListAgentsPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-subagent",
        Arc::new(dsh_tool_subagent::ToolSubagentPlugin),
    );
    loader.core.register(
        "@deepseek-ai/dsh-workflow-worker-thread",
        Arc::new(PresetBuiltinPlugin::WorkflowEngine),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-workflow",
        Arc::new(PresetBuiltinPlugin::Workflow),
    );
    loader.core.register(
        "@deepseek-ai/dsh-tool-ralph",
        Arc::new(PresetBuiltinPlugin::Ralph),
    );
    if let Some(profile) = profile {
        let profile_dir = data_root.join("profiles").join(profile);
        client_plugins::materialize_bundled(&profile_dir)?;
        for plugin in client_plugins::discover(&profile_dir)? {
            loader.core.register(&plugin.id, Arc::new(NoopPlugin));
        }
    }
    ctx.register_service(loader);
    if let Some(profile) = profile {
        let plugin_config = data_root
            .join("profiles")
            .join(profile)
            .join("plugins.json");
        if plugin_config.is_file() {
            let raw = std::fs::read_to_string(&plugin_config).map_err(|error| {
                format!("plugins config read {}: {error}", plugin_config.display())
            })?;
            let entries: Vec<serde_json::Value> = serde_json::from_str(&raw).map_err(|error| {
                format!("plugins config parse {}: {error}", plugin_config.display())
            })?;
            let loader = ctx
                .get_typed::<Arc<dsh_cordis_loader::LoaderService>>("loader", false)
                .map(|slot| slot.as_ref().clone())
                .ok_or_else(|| "loader service missing after install".to_string())?;
            futures::executor::block_on(dsh_app_boot::boot("dsh-host", &loader, &entries))?;
        }
    }
    let mut onboarding_properties = indexmap::IndexMap::new();
    onboarding_properties.insert(
        "welcomeNoticeVersion".to_string(),
        dsh_schemastery::Schema::string().default(dsh_schemastery::Data::String(String::new())),
    );
    settings
        .register(
            ctx,
            dsh_settings::settings_namespace("ui-onboarding")
                .map_err(|error| format!("settings namespace: {error}"))?,
            dsh_schemastery::Schema::object(onboarding_properties),
            dsh_settings::SettingsRegisterOptions::default(),
        )
        .map_err(|error| format!("settings ui-onboarding: {error}"))?;
    let storage = dsh_storage::Storage::install(ctx);
    let json_backend = dsh_storage_json::JsonStorageBackend::new(
        dsh_home.join("storages").to_string_lossy().into_owned(),
    );
    storage
        .backend
        .register("json", json_backend)
        .map_err(|error| format!("storage json: {error}"))?;
    let domains = dsh_storage_domain::DomainFacility::install(
        ctx,
        dsh_storage_domain::DomainFacilityConfig {
            backend: "json".to_string(),
            routes: Default::default(),
        },
    )
    .map_err(|error| format!("storage domain: {error}"))?;
    let message_feedback = dsh_message_feedback::MessageFeedbackService::install(
        ctx,
        &dsh_message_feedback::Config {
            max_note_bytes: 16_384,
        },
    )
    .map_err(|error| format!("message-feedback: {error}"))?;
    let persistence_api: Arc<dyn dsh_session_persistence::SessionPersistenceApi> =
        persistence.clone();
    let _projection_cache = dsh_session_projection_cache::SessionProjectionCache::install(
        ctx,
        dsh_session_projection_cache::Config {
            write_every_events: 256,
            write_interval_ms: 5_000,
        },
        &domains,
        persistence_api.clone(),
    )
    .map_err(|error| format!("session-projection-cache: {error}"))?;
    let live: Arc<dyn dsh_workspace::LiveSessionStore> =
        Arc::new(dsh_workspace::StoreLiveSessions(sessions.clone()));
    let persistence_for_delete = persistence.clone();
    let workspace_registry = dsh_workspace::WorkspaceRegistry::install(
        ctx,
        &domains,
        persistence_api,
        Some(live),
        Arc::new(move |session_id| {
            let persistence = persistence_for_delete.clone();
            let session_id = session_id.clone();
            Box::pin(async move { persistence.delete(&session_id).await })
        }),
    )
    .map_err(|error| format!("workspace: {error}"))?;
    // The agent-presets roster: the shipped presets beside this app's
    // config plus the harness-home user root the service appends itself.
    // Anchored to the manifest, not the process cwd (tests and launchers
    // run from different directories; TS anchors to the package location
    // the same way).
    let shipped_preset_root = packaged_resource("config/agent-presets")
        .to_string_lossy()
        .into_owned();
    let preset_home = dsh_home.to_string_lossy().into_owned();
    let preset_env: Arc<dyn Fn(&str) -> Option<String> + Send + Sync> =
        Arc::new(move |name| (name == "DSH_HOME").then(|| preset_home.clone()));
    let agent_presets = dsh_agent_presets::AgentPresets::install(
        ctx,
        dsh_agent_presets::Config {
            default: "standard".to_string(),
            roots: vec![dsh_agent_presets::PresetRoot {
                path: shipped_preset_root,
                trust: dsh_agent_presets::PresetTrust::System,
            }],
            include_user_root: true,
        },
        preset_env,
    )
    .map_err(|error| format!("agentPresets: {error}"))?;
    // Port zero asks the OS for a free test/embedding port; application
    // launchers may select a stable port so the web-surface prompt stays
    // byte-identical across Host restarts.
    let web_server = futures::executor::block_on(WebServer::install(
        ctx,
        WebConfig {
            host: BindHost::Loopback,
            port: bind_port,
        },
    ))
    .map_err(|error| format!("webserver: {error}"))?;
    let _web_surface = system_prompt.section(
        ctx,
        PromptSection {
            name: "app:web-surface".to_string(),
            order: -98.0,
            text: PromptText::Provider(Arc::new({
                let port = web_server.port();
                move |_| {
                    let web_url = format!("http://127.0.0.1:{port}");
                    let update_contract = "The client-plugin HMR receiver is active, but client-plugin changes reload without a refresh only while `pnpm run dev:web` is also running from this same checkout to rebuild their bundles; verify that watcher before promising automatic updates. Every other change — the apps/web shell and plain packages — requires rebuilding the affected Web artifacts and verifying this existing URL after a page refresh. ";
                    format!(
                        "You are interacting with the user through the DeepSeek Harness Web GUI at {web_url}. When the user refers to \"this page\", \"this GUI\", or \"this app\" without naming another target, they mean this GUI. The browser provides no implicit DOM, route, or screenshot context. {update_contract}Starting another server does not update this GUI. The apps/web Vite entry builds the shell but is not a standalone application because only dsh web injects window.__DSH_BOOT__. Do not start a replacement server unless the user asks; if one is needed, use a managed background job and verify its exact URL."
                    )
                }
            })),
            complete: None,
        },
    );
    // The SPA dist server claims the fallback seat.
    let dist_index = packaged_resource("web/dist/index.html")
        .to_string_lossy()
        .into_owned();
    let _ = apply_frontend_static(ctx, FrontendConfig { dist_index })
        .map_err(|error| format!("frontend-static: {error}"))?;
    let manifest_path = packaged_resource("web/dist/plugins/manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|_| include_str!("../../../../web/dist/plugins/manifest.json").to_string());
    let mut boot_payload: serde_json::Value =
        serde_json::from_str(&manifest_text).expect("web plugin manifest");
    let object = boot_payload
        .as_object_mut()
        .expect("web plugin manifest must be an object");
    object.insert(
        "noSkin".to_string(),
        serde_json::Value::Bool(!packaged_resource("web/dist/skins").is_dir()),
    );
    object.insert("apiBase".to_string(), serde_json::json!("/api"));
    object.insert(
        "provider".to_string(),
        serde_json::json!("deepseek-official"),
    );
    object.insert("model".to_string(), serde_json::json!("deepseek-chat"));
    let _external_client_plugin_routes = if let Some(profile) = profile {
        let profile_dir = data_root.join("profiles").join(profile);
        client_plugins::compose(&web_server, &mut boot_payload, &profile_dir)?
    } else {
        Vec::new()
    };
    let web_preview_route = web_preview::register(
        &web_server,
        workspace_registry.clone(),
        agents.clone(),
        terminals.clone(),
        subprocess.clone(),
        sandbox.clone(),
    );
    let boot_profile = profile.map(|profile| data_root.join("profiles").join(profile));
    let _ = web_server.tap_index(Arc::new(move |html| {
        let mut payload = boot_payload.clone();
        if let Some(profile) = boot_profile.as_ref() {
            let disabled = client_plugins::disabled_plugins(profile);
            if let Some(entries) = payload
                .get_mut("entries")
                .and_then(serde_json::Value::as_array_mut)
            {
                entries.retain(|entry| {
                    entry
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .is_none_or(|id| !disabled.contains(id))
                });
            }
        }
        let boot_script = format!(
            "<script>window.__DSH_BOOT__={};</script>",
            serde_json::to_string(&payload).expect("web boot payload")
        );
        html.replacen("</head>", &format!("{boot_script}</head>"), 1)
    }));
    // The directory-picker seam serves the browse interaction.
    BrowseDirectoryPicker::install(ctx, PickerConfig::default());
    // Ensure the inventory service exists even when no profile entry mounted it.
    if ctx.get("pluginInventory", false).is_none() {
        let _ = PluginInventoryGateway::install(ctx);
    }
    // The apiproxy gateway wires the 52-RPC surface onto the spine.
    let api_proxy = ApiProxyService::install(
        ctx,
        ApiProxyDefaults {
            default_model_selection: Arc::new({
                let default_model = default_model.clone();
                move || {
                    let selection = default_model.current_selection();
                    dsh_host_apiproxy::ModelSelection {
                        provider: selection.provider,
                        model: selection.model,
                        reasoning_effort: selection
                            .reasoning_effort
                            .map(|effort| effort.to_string()),
                    }
                }
            }),
            dsh_home: data_root.to_string_lossy().into_owned(),
            plugins_document: profile.map(|profile| {
                data_root
                    .join("profiles")
                    .join(profile)
                    .join("plugins.json")
            }),
            open_path: Some(Arc::new(|path, signal| {
                Box::pin(async move {
                    let abort: dsh_native_command::NativeCommandAbort =
                        Arc::new(move || signal.aborted());
                    dsh_host_apiproxy::native_path_opener::open_native_path(
                        &path,
                        Some(abort),
                        &dsh_host_apiproxy::native_path_opener::PathOpenerInternals::default(),
                    )
                    .await
                    .map_err(|error| error.message)
                })
            })),
            ..Default::default()
        },
    );
    let fetch_handler = Arc::new(to_fetch_handler(api_proxy.clone()));
    let wallpaper_route = web_server.register(WebRoute {
        kind: WebRouteKind::Exact,
        path: "/__dsh-bing-wallpaper".to_string(),
        handler: Arc::new(move |_request| {
            Box::pin(async move {
                const META_URL: &str =
                    "https://www.bing.com/HPImageArchive.aspx?format=js&idx=0&n=1&mkt=zh-CN";
                const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::limited(2))
                    .build()
                    .map_err(|error| WebHandlerError::new(error.to_string()))?;
                let metadata: serde_json::Value = client
                    .get(META_URL)
                    .send()
                    .await
                    .map_err(|error| WebHandlerError::new(error.to_string()))?
                    .error_for_status()
                    .map_err(|error| WebHandlerError::new(error.to_string()))?
                    .json()
                    .await
                    .map_err(|error| WebHandlerError::new(error.to_string()))?;
                let relative = metadata
                    .get("images")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|images| images.first())
                    .and_then(|image| image.get("url"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|url| url.starts_with('/'))
                    .ok_or_else(|| {
                        WebHandlerError::new("Bing wallpaper metadata omitted image URL")
                    })?;
                let response = client
                    .get(format!("https://www.bing.com{relative}"))
                    .send()
                    .await
                    .map_err(|error| WebHandlerError::new(error.to_string()))?
                    .error_for_status()
                    .map_err(|error| WebHandlerError::new(error.to_string()))?;
                let content_type = response
                    .headers()
                    .get(http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| value.starts_with("image/"))
                    .unwrap_or("image/jpeg")
                    .to_string();
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|error| WebHandlerError::new(error.to_string()))?;
                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err(WebHandlerError::new("Bing wallpaper exceeds 16 MiB"));
                }
                http::Response::builder()
                    .status(http::StatusCode::OK)
                    .header(http::header::CONTENT_TYPE, content_type)
                    .header(http::header::CACHE_CONTROL, "public, max-age=21600")
                    .header(http::header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                    .body(WebBody::from(bytes))
                    .map_err(|error| WebHandlerError::new(error.to_string()))
            })
        }),
    });
    let api_route = web_server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/api".to_string(),
        handler: Arc::new(move |request| {
            let fetch_handler = Arc::clone(&fetch_handler);
            Box::pin(async move { Ok(bridge_api_request(request, fetch_handler).await) })
        }),
    });
    let mux_api = api_proxy.clone();
    let _ = web_server.register_upgrade(WebUpgradeRoute {
        path: "/api/events.mux".to_string(),
        handler: Arc::new(move |request, socket| {
            let api = mux_api.clone();
            Box::pin(async move { pump_websocket_downlink(request, socket, api, false).await })
        }),
    });
    let host_api = api_proxy.clone();
    let _ = web_server.register_upgrade(WebUpgradeRoute {
        path: "/api/events.host".to_string(),
        handler: Arc::new(move |request, socket| {
            let api = host_api.clone();
            Box::pin(async move { pump_websocket_downlink(request, socket, api, true).await })
        }),
    });
    data_root_transferred.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(HostSpine {
        ctx: ctx.clone(),
        sessions,
        agents,
        llm,
        agent_loop,
        tools,
        system_prompt,
        commands,
        goals,
        questions,
        approval,
        message_feedback,
        persistence,
        search,
        query,
        web_server,
        api_proxy,
        agent_presets,
        api_route,
        wallpaper_route,
        web_preview_route,
        data_root,
        owns_data_root,
        boot_probe_id: session_id(format!("host-boot-{}", uuid::Uuid::new_v4())),
        companion_fiber: parking_lot::Mutex::new(None),
        lifecycle_fiber: parking_lot::Mutex::new(None),
        shutdown_result: tokio::sync::OnceCell::new(),
        shutdown_requested: std::sync::atomic::AtomicBool::new(false),
        shutdown_failures,
    })
}

/// The service inventory plus a real durability-and-search probe — the
/// observable boot report shared by the binary and the integration test.
pub async fn boot_report(spine: &HostSpine) -> Result<serde_json::Value, String> {
    // Live path: a store-attached session, a user message, and a durability
    // flush through the JSONL coordinator.
    let session = spine
        .sessions
        .create(
            &spine.ctx,
            Some(spine.boot_probe_id.clone()),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .map_err(|error| format!("session create: {error}"))?;
    let starter = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "host boot live needle".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    session
        .append(
            "user/message",
            serde_json::to_value(&starter).map_err(|error| error.to_string())?,
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .map_err(|error| format!("append: {error}"))?;
    let flushed = spine
        .sessions
        .flush(&session)
        .await
        .map_err(|error| format!("flush: {error}"))?;

    // Persisted-only path: an independent durable log the search index must
    // reconcile through the erased persistence service.
    let durable_header = dsh_session::SessionHeader {
        version: dsh_session::SESSION_FORMAT_VERSION,
        id: session_id(format!("{}-persisted", spine.boot_probe_id)),
        created_at: 1,
        cwd: None,
        parent_session: None,
        seed_length: None,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    };
    let durable_event = dsh_session::SessionEvent {
        type_: "user/message".to_string(),
        seq: 0,
        time: 1,
        data: serde_json::to_value(dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::Text {
                text: "host persisted needle".to_string(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ))
        .expect("message"),
        ignorable: None,
        surface_op: Some(dsh_session::SurfaceOp::Append),
        source_event_seqs: None,
    };
    spine
        .persistence
        .create(durable_header.clone())
        .await
        .map_err(|error| format!("persisted create: {error}"))?;
    spine
        .persistence
        .append(&durable_header.id, &[durable_event])
        .await
        .map_err(|error| format!("persisted append: {error}"))?;
    let snapshots = spine
        .persistence
        .list_snapshots()
        .await
        .map_err(|error| format!("snapshots: {error}"))?;

    // The FTS5 index must find both the live and the persisted log.
    let live_hits = spine
        .query
        .search_sessions(
            &SessionSearchRequest {
                query: "live needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(|error| format!("live search: {}", error.message))?;
    let persisted_hits = spine
        .query
        .search_sessions(
            &SessionSearchRequest {
                query: "persisted needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(|error| format!("persisted search: {}", error.message))?;

    // The agent-presets roster serves the shipped presets beside this app's
    // config (a live discovery read proves the mount).
    let roster = spine
        .agent_presets
        .list()
        .await
        .map_err(|error| format!("preset roster: {error}"))?;

    Ok(serde_json::json!({
        "services": [
            "invariants",
            "sessions",
            "agents",
            "llm",
            "systemPrompt",
            "tools",
            "agentLoop",
            "commands",
            "goals",
            "userQuestions",
            "approval",
            "sessionPersistence",
            "sessionQuery",
            "schedule",
            "agentPresets",
            "subprocess",
            "sandbox",
            "sandboxPolicy",
            "jobs",
            "terminals",
            "shell",
            "subagents",
            "codeRuntime",
            "workflowEngine",
        ],
        "session": {
            "id": session.id().as_str(),
            "seq": session.seq(),
            "toolCount": spine.tools.schemas(None).len(),
        },
        "probe": {
            "flushAcknowledged": flushed,
            "persistedSnapshotCount": snapshots.len(),
            "liveSearchHits": live_hits.items.len(),
            "persistedSearchHits": persisted_hits.items.len(),
            "presetCount": roster.len(),
        },
    }))
}

struct HostCompanionsPlugin;

#[async_trait::async_trait]
impl Plugin for HostCompanionsPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("dsh-host-companions")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let _ = dsh_session::invariant::apply(ctx).await;
        let _goal = dsh_goal::invariant::apply(ctx);
        let _goal_round_driver = dsh_goal_round_driver::invariant::apply(ctx);
        let _command_goal = dsh_command_goal::invariant::apply(ctx);
        let _tool_goal = dsh_tool_goal::invariant::apply(ctx);
        let _plan_mode = dsh_plan_mode::invariant::apply(ctx);
        let _agent_loop = dsh_agent_loop::apply_agent_loop_invariant(ctx);
        let _schedule = dsh_schedule::invariant::apply(ctx);
        let _query_sqlite = dsh_session_query_sqlite::invariant::apply(ctx);
        let _ = ctx.plugin(Arc::new(dsh_llm::LlmInvariantPlugin), arc(()));
        Ok(())
    }
}

/// Mount package-owned invariant companions in an independently disposable
/// child fiber. Repeated calls join the same settled fiber.
pub fn mount_companions(spine: &HostSpine) -> Result<(), String> {
    if spine
        .shutdown_requested
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err("cannot mount host companions after shutdown has started".to_string());
    }
    let fiber = {
        let mut slot = spine.companion_fiber.lock();
        slot.get_or_insert_with(|| spine.ctx.plugin(Arc::new(HostCompanionsPlugin), arc(())))
            .clone()
    };
    futures::executor::block_on(fiber.settle())
        .map_err(|error| format!("host companions: {}", error.message()))
}

// Re-exported anchors for compositions.
pub use dsh_agent::AgentRegistry as AgentRegistryType;
pub use dsh_session::SessionStore as SessionStoreType;

#[cfg(test)]
mod reasoning_tests {
    use super::{
        OpenAiCompatibleModelConfig, ReasoningEffortsConfig, inferred_gpt_reasoning_efforts,
        resolved_reasoning_efforts, workspace_context_text,
    };

    fn model(id: &str) -> OpenAiCompatibleModelConfig {
        OpenAiCompatibleModelConfig {
            id: id.to_string(),
            name: None,
            context_window: None,
            max_tokens: None,
            reasoning_efforts: None,
        }
    }

    #[test]
    fn gpt_five_stable_max_maps_to_openai_xhigh() {
        let efforts = inferred_gpt_reasoning_efforts("gpt-5.6-sol").expect("GPT capability");
        assert_eq!(
            efforts.get("max").and_then(Clone::clone).as_deref(),
            Some("xhigh")
        );
        assert!(!efforts.contains_key("xhigh"));
    }

    #[test]
    fn unknown_models_do_not_gain_reasoning_capability() {
        assert!(inferred_gpt_reasoning_efforts("muse-spark-1.2").is_none());
    }

    #[test]
    fn explicit_false_disables_gpt_inference() {
        let mut model = model("gpt-5.6-sol");
        model.reasoning_efforts = Some(ReasoningEffortsConfig::Disabled(false));
        assert!(resolved_reasoning_efforts(&model).is_none());
    }

    #[test]
    fn explicit_map_overrides_gpt_inference() {
        let mut model = model("gpt-5.6-sol");
        model.reasoning_efforts = Some(ReasoningEffortsConfig::Levels(indexmap::IndexMap::from([
            ("off".to_string(), None),
            ("high".to_string(), Some("ultra".to_string())),
        ])));
        let efforts = resolved_reasoning_efforts(&model).expect("explicit capability");
        assert_eq!(
            efforts.get("high").and_then(Clone::clone).as_deref(),
            Some("ultra")
        );
        assert!(!efforts.contains_key("max"));
    }

    #[test]
    fn workspace_context_uses_human_windows_drive_path() {
        assert_eq!(
            workspace_context_text(Some(r"\\?\D:\deepwork\deepseek-harness-rs")),
            "Current workspace (authoritative session working directory): D:\\deepwork\\deepseek-harness-rs. Use this exact path and drive for workspace-relative work; do not infer it from the Harness checkout or sandbox fallback."
        );
    }

    #[test]
    fn workspace_context_preserves_unc_path_without_device_prefix() {
        assert_eq!(
            workspace_context_text(Some(r"\\?\UNC\server\share\repo")),
            r"Current workspace (authoritative session working directory): \\server\share\repo. Use this exact path and drive for workspace-relative work; do not infer it from the Harness checkout or sandbox fallback."
        );
    }
}
