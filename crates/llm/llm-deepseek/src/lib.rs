//! DeepSeek official chat-completions adapter.

mod serialize;
mod sse;
mod translate;
mod transport;

use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use dsh_llm::{
    AdapterRegistrationHandle, ChunkStream, FinishReason, GenerateOptions, LlmAdapter, LlmError,
    LlmErrorOptions, LlmFailure, LlmModelContext, LlmModelInfo, LlmModelReasoningInfo,
    LlmProviderInfo, LlmReasoningEffortInfo, LlmResolvedModelInfo, LlmRuntime, ModelModality,
    ResolvedRetryPolicy, StreamChunk, app_identity, assert_usable_api_key, attribution_headers,
    reasoning_effort_id, resolve_retry_policy,
};
use futures::future::BoxFuture;

pub const PROVIDER: &str = "deepseek-official";
pub const PUBLIC_BASE_URL: &str = "https://api.deepseek.com";

const MAX_SUCCESS_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SUCCESS_STREAM_CHUNKS: usize = 100_000;
pub const DEFAULT_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
pub const DEFAULT_CONTEXT_WINDOW: u64 = 1_000_000;
pub const DEFAULT_MAX_TOKENS: u64 = 256_000;
pub const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepSeekReasoningEffort {
    Off,
    High,
    Max,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestDefaults {
    pub thinking: Option<ThinkingMode>,
    pub reasoning_effort: Option<DeepSeekReasoningEffort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSeekCatalogModel {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct DeepSeekConfig {
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
    pub thinking: Option<ThinkingMode>,
    pub reasoning_effort: Option<DeepSeekReasoningEffort>,
    pub max_tokens: Option<u64>,
    pub default_context_window: Option<u64>,
    pub models: Option<Vec<DeepSeekCatalogModel>>,
    pub stream_idle_timeout_ms: Option<u64>,
    pub retry_policy: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct ResolvedDeepSeekOptions {
    pub api_key_env: String,
    pub base_url: String,
    pub defaults: RequestDefaults,
    pub max_tokens: u64,
    pub default_context_window: u64,
    pub models: Vec<DeepSeekCatalogModel>,
    pub stream_idle_timeout: Duration,
    pub retry_policy: ResolvedRetryPolicy,
}

#[allow(clippy::result_large_err)] // Match the core LlmRuntime error seam.
pub fn resolve_adapter_options(
    config: &DeepSeekConfig,
) -> Result<ResolvedDeepSeekOptions, LlmError> {
    let models = config.models.clone().unwrap_or_else(|| {
        vec![
            DeepSeekCatalogModel {
                id: "deepseek-v4-flash".to_string(),
                name: Some("DeepSeek-V4-Flash".to_string()),
                description: None,
                context_window: Some(DEFAULT_CONTEXT_WINDOW),
                max_tokens: None,
            },
            DeepSeekCatalogModel {
                id: "deepseek-v4-pro".to_string(),
                name: Some("DeepSeek-V4-Pro".to_string()),
                description: None,
                context_window: Some(DEFAULT_CONTEXT_WINDOW),
                max_tokens: None,
            },
        ]
    });
    let retry_policy =
        resolve_retry_policy(config.retry_policy.as_ref(), "llm-deepseek: retryPolicy").map_err(
            |message| LlmError::new(&message, "INVALID_CONFIG", LlmErrorOptions::default()),
        )?;
    Ok(ResolvedDeepSeekOptions {
        api_key_env: config
            .api_key_env
            .clone()
            .unwrap_or_else(|| DEFAULT_API_KEY_ENV.to_string()),
        base_url: config
            .base_url
            .clone()
            .unwrap_or_else(|| PUBLIC_BASE_URL.to_string()),
        defaults: RequestDefaults {
            thinking: config.thinking,
            reasoning_effort: config.reasoning_effort,
        },
        max_tokens: config.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        default_context_window: config
            .default_context_window
            .unwrap_or(DEFAULT_CONTEXT_WINDOW),
        models,
        stream_idle_timeout: Duration::from_millis(
            config
                .stream_idle_timeout_ms
                .unwrap_or(DEFAULT_STREAM_IDLE_TIMEOUT_MS),
        ),
        retry_policy,
    })
}

pub type OptionsResolver = Arc<dyn Fn() -> Result<ResolvedDeepSeekOptions, LlmError> + Send + Sync>;
pub type ApiKeyResolver = Arc<
    dyn Fn(&ResolvedDeepSeekOptions) -> BoxFuture<'static, Result<Option<String>, LlmError>>
        + Send
        + Sync,
>;

pub struct DeepSeekAdapterOptions {
    pub options: OptionsResolver,
    pub resolve_api_key: ApiKeyResolver,
}

pub struct DeepSeekAdapter {
    config: DeepSeekAdapterOptions,
}

impl DeepSeekAdapter {
    pub fn new(config: DeepSeekAdapterOptions) -> Self {
        Self { config }
    }
}

fn failure(message: impl Into<String>, code: &str) -> LlmFailure {
    LlmFailure {
        message: message.into(),
        code: code.to_string(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

fn error_finish(failure: LlmFailure) -> StreamChunk {
    StreamChunk::Finish {
        reason: FinishReason::Error { failure },
        replay_state: None,
    }
}

fn http_failure(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> LlmFailure {
    let detail = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    let status_number = status.as_u16();
    let code = match status_number {
        401 | 403 => "AUTH",
        429 => "RATE_LIMIT",
        400 => {
            let text = detail.as_deref().unwrap_or_default().to_ascii_lowercase();
            if text.contains("context") && (text.contains("length") || text.contains("window")) {
                dsh_llm::CONTEXT_WINDOW_EXCEEDED_CODE
            } else {
                "INVALID_REQUEST"
            }
        }
        500..=599 => "SERVER",
        _ => "HTTP_ERROR",
    };
    let provider_retry_after_ms = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|seconds| seconds.checked_mul(1_000))
        .filter(|delay| *delay > 0);
    let request_id = headers
        .get("x-request-id")
        .or_else(|| headers.get("x-deepseek-request-id"))
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(dsh_llm::provider_request_id);
    LlmFailure {
        message: detail.unwrap_or_else(|| format!("DeepSeek API error (HTTP {status_number})")),
        code: code.to_string(),
        status: Some(u64::from(status_number)),
        provider_retry_after_ms,
        request_id,
    }
}

async fn request_chunks(
    options: GenerateOptions,
    connection: ResolvedDeepSeekOptions,
    api_key: String,
) -> Result<Vec<StreamChunk>, LlmFailure> {
    let body = serialize::serialize_request(&options, &connection.defaults)?;
    let url = format!(
        "{}/chat/completions",
        connection.base_url.trim_end_matches('/')
    );
    let encoded = serde_json::to_vec(&body).map_err(|error| {
        failure(
            format!("DeepSeek request encode failed: {error}"),
            "INVALID_REQUEST",
        )
    })?;
    let mut response = transport::post(
        &url,
        &api_key,
        encoded,
        &attribution_headers(&app_identity()),
    )
    .await
    .map_err(|error| failure(format!("DeepSeek API request failed: {error}"), "TRANSPORT"))?;
    if !response.status.is_success() {
        let status = response.status;
        let headers = response.headers.clone();
        let body = response
            .collect_limited(8 * 1024 * 1024)
            .await
            .unwrap_or_default();
        return Err(http_failure(status, &headers, &body));
    }

    let mut parser = sse::SseParser::new();
    let mut translator = translate::Translator::new();
    let mut output = Vec::new();
    let mut received_bytes = 0_usize;
    while let Some(bytes) =
        tokio::time::timeout(connection.stream_idle_timeout, response.next_data())
            .await
            .map_err(|_| failure("DeepSeek stream idle timeout", "TIMEOUT"))?
            .map_err(|error| failure(format!("DeepSeek API stream failed: {error}"), "TRANSPORT"))?
    {
        received_bytes = received_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| failure("DeepSeek response size overflowed", "RESPONSE_TOO_LARGE"))?;
        if received_bytes > MAX_SUCCESS_RESPONSE_BYTES {
            return Err(failure(
                "DeepSeek success response exceeded 8 MiB",
                "RESPONSE_TOO_LARGE",
            ));
        }
        for payload in parser.push(&bytes)? {
            let done = payload == sse::DONE;
            let translated = translator.consume(&payload)?;
            let total_chunks = output.len().checked_add(translated.len()).ok_or_else(|| {
                failure(
                    "DeepSeek stream chunk count overflowed",
                    "RESPONSE_TOO_LARGE",
                )
            })?;
            if total_chunks > MAX_SUCCESS_STREAM_CHUNKS {
                return Err(failure(
                    "DeepSeek success response emitted too many chunks",
                    "RESPONSE_TOO_LARGE",
                ));
            }
            output.extend(translated);
            if done {
                return Ok(output);
            }
        }
    }
    parser.finish()?;
    Ok(output)
}

struct RequestWorkerGuard {
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for RequestWorkerGuard {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if self
            .thread
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            let _ = self.thread.take().expect("checked worker handle").join();
        }
    }
}

async fn drive_owned_request(
    options: GenerateOptions,
    options_resolver: OptionsResolver,
    key_resolver: ApiKeyResolver,
    sender: tokio::sync::mpsc::UnboundedSender<StreamChunk>,
) {
    let connection = match options_resolver() {
        Ok(connection) => connection,
        Err(error) => {
            let _ = sender.send(error_finish(error.failure));
            return;
        }
    };
    let raw_key = match key_resolver(&connection).await {
        Ok(Some(key)) => key,
        Ok(None) => {
            let _ = sender.send(error_finish(failure(
                format!(
                    "llm-deepseek: no API key resolved from {}",
                    connection.api_key_env
                ),
                "MISSING_CREDENTIAL",
            )));
            return;
        }
        Err(error) => {
            let _ = sender.send(error_finish(error.failure));
            return;
        }
    };
    let api_key = match assert_usable_api_key(&raw_key, "llm-deepseek", &connection.api_key_env) {
        Ok(key) => key,
        Err(error) => {
            let _ = sender.send(error_finish(error.failure));
            return;
        }
    };
    match request_chunks(options, connection, api_key).await {
        Ok(chunks) => {
            for chunk in chunks {
                if sender.send(chunk).is_err() {
                    return;
                }
            }
        }
        Err(failure) => {
            let _ = sender.send(error_finish(failure));
        }
    }
}

#[async_trait::async_trait]
impl LlmAdapter for DeepSeekAdapter {
    fn provider_info(&self, provider: &str) -> LlmProviderInfo {
        LlmProviderInfo {
            id: provider.to_string(),
            name: "DeepSeek".to_string(),
        }
    }

    fn provider_retry_policy(&self, _provider: &str) -> Option<ResolvedRetryPolicy> {
        (self.config.options)()
            .ok()
            .map(|options| options.retry_policy)
    }

    async fn list_models(&self, provider: &str) -> Vec<LlmModelInfo> {
        let Ok(options) = (self.config.options)() else {
            return Vec::new();
        };
        options
            .models
            .into_iter()
            .map(|model| LlmModelInfo {
                provider: provider.to_string(),
                name: model.name.clone().unwrap_or_else(|| model.id.clone()),
                id: model.id,
                description: model.description,
                input_modalities: Some(vec![ModelModality::Text]),
            })
            .collect()
    }

    async fn resolve_model(
        &self,
        provider: &str,
        model: &str,
        _signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> LlmResolvedModelInfo {
        let options = (self.config.options)().expect("validated DeepSeek options");
        let configured = options.models.iter().find(|entry| entry.id == model);
        let effort = match options.defaults.reasoning_effort {
            Some(DeepSeekReasoningEffort::Off) => "off",
            Some(DeepSeekReasoningEffort::Max) => "max",
            _ => "high",
        };
        let efforts = if options.defaults.thinking == Some(ThinkingMode::Disabled) {
            vec![LlmReasoningEffortInfo {
                id: reasoning_effort_id("off"),
                name: "Off".to_string(),
                description: None,
            }]
        } else {
            ["off", "high", "max"]
                .into_iter()
                .map(|id| LlmReasoningEffortInfo {
                    id: reasoning_effort_id(id),
                    name: match id {
                        "off" => "Off",
                        "max" => "Max",
                        _ => "High",
                    }
                    .to_string(),
                    description: None,
                })
                .collect()
        };
        LlmResolvedModelInfo {
            provider: provider.to_string(),
            id: model.to_string(),
            name: configured
                .and_then(|entry| entry.name.clone())
                .unwrap_or_else(|| model.to_string()),
            description: configured.and_then(|entry| entry.description.clone()),
            input_modalities: Some(vec![ModelModality::Text]),
            context: Some(LlmModelContext {
                context_window: configured
                    .and_then(|entry| entry.context_window)
                    .unwrap_or(options.default_context_window),
            }),
            default_max_tokens: Some(
                configured
                    .and_then(|entry| entry.max_tokens)
                    .unwrap_or(options.max_tokens),
            ),
            reasoning: Some(LlmModelReasoningInfo {
                efforts,
                default_effort: Some(reasoning_effort_id(effort)),
            }),
        }
    }

    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        let options = options.clone();
        let options_resolver = Arc::clone(&self.config.options);
        let key_resolver = Arc::clone(&self.config.resolve_api_key);
        Box::pin(async_stream::stream! {
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let (cancel, cancelled) = tokio::sync::oneshot::channel();
            let thread = std::thread::Builder::new()
                .name("dsh-deepseek-request".to_string())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("DeepSeek request runtime");
                    runtime.block_on(async move {
                        tokio::select! {
                            _ = drive_owned_request(options, options_resolver, key_resolver, sender) => {},
                            _ = cancelled => {},
                        }
                    });
                })
                .expect("spawn DeepSeek request worker");
            let _guard = RequestWorkerGuard {
                cancel: Some(cancel),
                thread: Some(thread),
            };
            while let Some(chunk) = receiver.recv().await {
                yield chunk;
            }
        })
    }
}

#[allow(clippy::result_large_err)] // Match LlmRuntime::register_adapter.
pub fn apply(
    ctx: &Context,
    runtime: &Arc<LlmRuntime>,
    adapter: Arc<DeepSeekAdapter>,
) -> Result<AdapterRegistrationHandle, LlmError> {
    runtime.register_adapter(ctx, vec![PROVIDER.to_string()], adapter)
}
