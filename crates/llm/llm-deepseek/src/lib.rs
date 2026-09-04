//! DeepSeek official chat-completions adapter.

mod files_api;
mod responses;
mod serialize;
mod sse;
mod translate;

#[cfg(test)]
mod reasoning_mapping_tests;
mod transport;
mod upload_index;

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

pub use files_api::{
    DeepSeekFileId, DeepSeekFileObject, DeepSeekFilesClient, DeepSeekFilesError, FilesErrorCode,
    classify_files_status, deepseek_file_id, parse_file_object,
};
pub use upload_index::{
    DeepSeekFileScope, DeepSeekUploadIndex, DeepSeekUploadRecord, UploadIndexCommit,
    deepseek_file_scope,
};

pub const PROVIDER: &str = "deepseek-official";
pub const PUBLIC_BASE_URL: &str = "https://api.deepseek.com";

const MAX_SUCCESS_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SUCCESS_STREAM_CHUNKS: usize = 100_000;
pub const DEFAULT_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
pub const DEFAULT_CONTEXT_WINDOW: u64 = 1_000_000;
pub const DEFAULT_MAX_TOKENS: u64 = 256_000;
pub const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;
pub const DEFAULT_FILES_API_TIMEOUT_MS: u64 = 120_000;
pub const DEFAULT_MAX_REQUEST_FILES_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_INLINE_REQUEST_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
pub const DEFAULT_MAX_REQUEST_IMAGE_BYTES: usize = DEFAULT_MAX_INLINE_REQUEST_IMAGE_BYTES as usize;
pub const DEFAULT_MAX_IMAGES_PER_REQUEST: usize = 600;
pub const DEFAULT_REQUEST_IMAGE_MAX_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_IMAGE_OFFLOAD_BYTE_QUANTUM: u64 = 64 * 1024 * 1024;
pub const DEFAULT_INLINE_IMAGE_OFFLOAD_BYTE_QUANTUM: u64 = 10 * 1024 * 1024;
pub const DEFAULT_IMAGE_OFFLOAD_COUNT_QUANTUM: usize = 20;

/// Known provider output caps that are not normally disclosed by the
/// OpenAI-compatible `/models` response. Exact configured metadata wins.
pub fn inferred_model_max_tokens(model_id: &str) -> Option<u64> {
    match model_id.to_ascii_lowercase().as_str() {
        "glm-5.3" | "glm-5.3-flash" => Some(131_072),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepSeekReasoningEffort {
    Off,
    Low,
    High,
    Max,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestDefaults {
    pub thinking: Option<ThinkingMode>,
    pub reasoning_effort: Option<DeepSeekReasoningEffort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogReasoningEffort {
    /// Stable effort id exposed to DSH callers.
    pub id: String,
    /// Human-readable model-menu label.
    pub name: String,
    /// Exact provider wire value.
    pub wire: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSeekCatalogModel {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    /// Optional exact-model reasoning catalog. Absence keeps the DeepSeek
    /// adapter defaults; an explicit catalog is used by generic routes.
    pub reasoning_efforts: Option<Vec<CatalogReasoningEffort>>,
    /// Whether this exact catalog route accepts image input.
    pub image_input: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct DeepSeekConfig {
    pub api: Option<String>,
    pub api_key_env: Option<String>,
    /// Send requests without an Authorization header for an explicitly
    /// configured anonymous OpenAI-compatible route.
    pub keyless: bool,
    pub base_url: Option<String>,
    pub thinking: Option<ThinkingMode>,
    pub reasoning_effort: Option<DeepSeekReasoningEffort>,
    pub max_tokens: Option<u64>,
    pub default_context_window: Option<u64>,
    pub models: Option<Vec<DeepSeekCatalogModel>>,
    pub stream_idle_timeout_ms: Option<u64>,
    pub files_api_timeout_ms: Option<u64>,
    pub files_index_path: Option<std::path::PathBuf>,
    pub retry_policy: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct ResolvedDeepSeekOptions {
    pub api: String,
    pub api_key_env: String,
    pub keyless: bool,
    pub base_url: String,
    pub defaults: RequestDefaults,
    pub max_tokens: u64,
    pub default_context_window: u64,
    pub models: Vec<DeepSeekCatalogModel>,
    pub stream_idle_timeout: Duration,
    pub files_api_timeout: Duration,
    pub files_index_path: Option<std::path::PathBuf>,
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
                reasoning_efforts: None,
                image_input: Some(false),
            },
            DeepSeekCatalogModel {
                id: "deepseek-v4-pro".to_string(),
                name: Some("DeepSeek-V4-Pro".to_string()),
                description: None,
                context_window: Some(DEFAULT_CONTEXT_WINDOW),
                max_tokens: None,
                reasoning_efforts: None,
                image_input: Some(false),
            },
            DeepSeekCatalogModel {
                id: "deepseek-v4-flash-vision-exp".to_string(),
                name: Some("DeepSeek-V4-Flash-Vision-Exp".to_string()),
                description: None,
                context_window: Some(DEFAULT_CONTEXT_WINDOW),
                max_tokens: None,
                reasoning_efforts: None,
                image_input: Some(true),
            },
        ]
    });
    let retry_policy =
        resolve_retry_policy(config.retry_policy.as_ref(), "llm-deepseek: retryPolicy").map_err(
            |message| LlmError::new(&message, "INVALID_CONFIG", LlmErrorOptions::default()),
        )?;
    let api = config
        .api
        .clone()
        .unwrap_or_else(|| "openai-completions".to_string());
    if api != "openai-completions" && api != "openai-responses" {
        return Err(LlmError::new(
            "unsupported llm-deepseek api protocol",
            "UNSUPPORTED_PROTOCOL",
            LlmErrorOptions::default(),
        ));
    }
    Ok(ResolvedDeepSeekOptions {
        api,
        api_key_env: config
            .api_key_env
            .clone()
            .unwrap_or_else(|| DEFAULT_API_KEY_ENV.to_string()),
        keyless: config.keyless,
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
        files_api_timeout: Duration::from_millis(
            config
                .files_api_timeout_ms
                .unwrap_or(DEFAULT_FILES_API_TIMEOUT_MS),
        ),
        files_index_path: config.files_index_path.clone(),
        retry_policy,
    })
}

fn endpoint_url(base_url: &str, api: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let root = trimmed
        .strip_suffix("/chat/completions")
        .or_else(|| trimmed.strip_suffix("/responses"))
        .unwrap_or(trimmed);
    let suffix = if api == "openai-responses" {
        "responses"
    } else {
        "chat/completions"
    };
    format!("{root}/{suffix}")
}

pub type OptionsResolver = Arc<dyn Fn() -> Result<ResolvedDeepSeekOptions, LlmError> + Send + Sync>;
pub type ApiKeyResolver = Arc<
    dyn Fn(&ResolvedDeepSeekOptions) -> BoxFuture<'static, Result<Option<String>, LlmError>>
        + Send
        + Sync,
>;
pub type AttachmentResolver =
    Arc<dyn Fn() -> Option<Arc<dyn dsh_attachment::AttachmentStore>> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningWireFormat {
    /// DeepSeek's `thinking` object plus optional `reasoning_effort`.
    DeepSeek,
    /// OpenAI-compatible `reasoning_effort`; Responses conversion maps it to
    /// the nested `reasoning` object.
    OpenAi,
}

pub struct DeepSeekAdapterOptions {
    pub options: OptionsResolver,
    pub resolve_api_key: ApiKeyResolver,
    pub resolve_attachments: Option<AttachmentResolver>,
    pub provider_name: Option<String>,
    pub reasoning_wire_format: ReasoningWireFormat,
}

pub struct DeepSeekAdapter {
    config: DeepSeekAdapterOptions,
}

fn upload_lock(
    scope: &DeepSeekFileScope,
    variant: &dsh_attachment::ImageVariantId,
) -> Arc<tokio::sync::Mutex<()>> {
    use std::sync::{Mutex, OnceLock, Weak};

    static LOCKS: OnceLock<Mutex<std::collections::HashMap<String, Weak<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    let key = format!("{}:{}", scope.as_str(), variant.as_str());
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
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
    provider_name: &str,
) -> LlmFailure {
    let detail = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            ["/error/message", "/message", "/detail"]
                .into_iter()
                .find_map(|path| value.pointer(path))
                .or_else(|| value.pointer("/error"))
                .and_then(|detail| match detail {
                    serde_json::Value::String(text) => Some(text.clone()),
                    serde_json::Value::Null => None,
                    other => serde_json::to_string(other).ok(),
                })
        })
        .map(|detail| detail.chars().take(4_096).collect::<String>());
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
        message: detail
            .unwrap_or_else(|| format!("{provider_name} API error (HTTP {status_number})")),
        code: code.to_string(),
        status: Some(u64::from(status_number)),
        provider_retry_after_ms,
        request_id,
    }
}

fn collect_image_attachments<'a>(
    blocks: &'a [dsh_llm::ContentBlock],
    ordered: &mut Vec<&'a dsh_llm::ImageAttachmentRef>,
    seen: &mut std::collections::HashSet<String>,
) {
    for block in blocks {
        match block {
            dsh_llm::ContentBlock::Image { attachment } => {
                if seen.insert(attachment.attachment_id.clone()) {
                    ordered.push(attachment);
                }
            }
            dsh_llm::ContentBlock::ToolResult { content, .. } => {
                collect_image_attachments(content, ordered, seen);
            }
            _ => {}
        }
    }
}

fn request_image_attachments(options: &GenerateOptions) -> Vec<&dsh_llm::ImageAttachmentRef> {
    let mut ordered = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for message in &options.messages {
        collect_image_attachments(&message.content, &mut ordered, &mut seen);
    }
    ordered
}

fn project_estimated_request(options: &GenerateOptions) -> GenerateOptions {
    let estimate = |image: &dsh_llm::ImageAttachmentRef| {
        image
            .bytes
            .unwrap_or(0)
            .min(DEFAULT_REQUEST_IMAGE_MAX_BYTES)
    };
    let mut projected = options.clone();
    projected.messages = dsh_llm::offload_request_images_with_policy(
        &options.messages,
        &dsh_llm::RequestImageOffloadPolicy {
            representation: dsh_llm::RequestImageRepresentation::Raw,
            max_images: Some(DEFAULT_MAX_IMAGES_PER_REQUEST),
            max_bytes: Some(DEFAULT_MAX_REQUEST_FILES_BYTES),
            count_quantum: Some(DEFAULT_IMAGE_OFFLOAD_COUNT_QUANTUM),
            byte_quantum: Some(DEFAULT_IMAGE_OFFLOAD_BYTE_QUANTUM),
            byte_length: Some(&estimate),
        },
    );
    projected
}

fn request_image_dimensions(width: u64, height: u64, max_pixels: u64) -> (u64, u64) {
    if width == 0 || height == 0 || width.saturating_mul(height) <= max_pixels {
        return (width.max(1), height.max(1));
    }
    let scale = (max_pixels as f64 / (width as f64 * height as f64)).sqrt();
    (
        ((width as f64 * scale).floor() as u64).max(1),
        ((height as f64 * scale).floor() as u64).max(1),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImageGridResize {
    grid_height: u64,
    grid_width: u64,
    best_height: u64,
    best_width: u64,
    tokens: u64,
}

fn image_grid_tokens(grid_height: u64, grid_width: u64) -> u64 {
    let mut tokens = grid_height.saturating_mul(grid_width + 1).saturating_add(2);
    if grid_height % 2 == 1 {
        tokens = tokens.saturating_add(grid_width + 1);
    }
    tokens.saturating_add((grid_height.div_ceil(2) * (grid_width + 1) % 2) * 2)
}

fn solve_image_resize_ratio(height: u64, width: u64, budget: u64) -> ImageGridResize {
    const PATCH: u64 = 14;
    const DOWNSAMPLE: u64 = 3;
    let aspect = height as f64 / width as f64;
    let ideal_grid_width = (((budget - 2) as f64 / aspect) + 0.25).sqrt() - 0.5;
    let ideal_grid_height = ideal_grid_width * aspect;
    let (best_height, best_width) = if ideal_grid_width < 1.0 {
        let grid_width = 1u64;
        let mut grid_height = (budget - 2) / (grid_width + 1);
        if grid_height % 2 == 1 {
            grid_height -= 1;
        }
        (
            grid_height * PATCH * DOWNSAMPLE,
            grid_width * PATCH * DOWNSAMPLE,
        )
    } else if ideal_grid_height < 2.0 {
        let grid_height = 2u64;
        let grid_width = (budget - 2) / grid_height - 1;
        assert!(
            grid_width > 1,
            "deepseek image tokens: no grid fits the token budget"
        );
        (
            grid_height * PATCH * DOWNSAMPLE,
            grid_width * PATCH * DOWNSAMPLE,
        )
    } else {
        let grid_width = ideal_grid_width.trunc() as u64;
        let mut grid_height = ideal_grid_height.trunc() as u64;
        if grid_height % 2 == 1 {
            grid_height -= 1;
        }
        let width_scale = (grid_width * PATCH * DOWNSAMPLE) as f64 / width as f64;
        let height_scale = (grid_height * PATCH * DOWNSAMPLE) as f64 / height as f64;
        let scale = width_scale.min(height_scale);
        (
            ((height as f64 * scale / PATCH as f64).trunc() as u64) * PATCH,
            ((width as f64 * scale / PATCH as f64).trunc() as u64) * PATCH,
        )
    };
    let grid_height = (best_height / PATCH).div_ceil(DOWNSAMPLE);
    let grid_width = (best_width / PATCH).div_ceil(DOWNSAMPLE);
    ImageGridResize {
        grid_height,
        grid_width,
        best_height,
        best_width,
        tokens: image_grid_tokens(grid_height, grid_width),
    }
}

fn resize_image_once(width: u64, height: u64) -> ImageGridResize {
    const PATCH: u64 = 14;
    const DOWNSAMPLE: u64 = 3;
    const MAX_TOKENS: u64 = 384;
    const PAD: u64 = 3;
    const MIN_PIXELS: u64 = 384 * 384;
    let mut width = width.max(1);
    let mut height = height.max(1);
    if width > height.saturating_mul(8) {
        width = height.saturating_mul(8);
    }
    let pixels = width.saturating_mul(height);
    if pixels < MIN_PIXELS {
        let scale = (MIN_PIXELS as f64 / pixels as f64).sqrt();
        width = (width as f64 * scale).trunc() as u64;
        height = (height as f64 * scale).trunc() as u64;
    }
    let padded_width = width.div_ceil(PATCH) * PATCH;
    let padded_height = height.div_ceil(PATCH) * PATCH;
    let grid_height = (padded_height / PATCH).div_ceil(DOWNSAMPLE);
    let grid_width = (padded_width / PATCH).div_ceil(DOWNSAMPLE);
    let budget = MAX_TOKENS - PAD;
    let mut resize = ImageGridResize {
        grid_height,
        grid_width,
        best_height: padded_height,
        best_width: padded_width,
        tokens: image_grid_tokens(grid_height, grid_width),
    };
    if resize.tokens > budget {
        resize = solve_image_resize_ratio(height, width, budget);
        let mut reduced = budget;
        while resize.tokens > budget {
            reduced -= 1;
            resize = solve_image_resize_ratio(height, width, reduced);
        }
    }
    resize.tokens += PAD;
    resize
}

fn deepseek_image_tokens(width: u64, height: u64) -> u64 {
    let mut current = resize_image_once(width, height);
    for _ in 1..10 {
        let next = resize_image_once(current.best_width, current.best_height);
        if next == current {
            return current.tokens;
        }
        current = next;
    }
    panic!("deepseek image tokens: resize did not converge for {width}x{height}")
}

fn model_modalities(model: Option<&DeepSeekCatalogModel>) -> Option<Vec<ModelModality>> {
    model.and_then(|model| {
        model.image_input.map(|image_input| {
            if image_input {
                vec![ModelModality::Text, ModelModality::Image]
            } else {
                vec![ModelModality::Text]
            }
        })
    })
}

#[cfg(test)]
mod image_pricing_tests {
    use super::*;

    #[test]
    fn image_token_pricing_matches_provider_reference_values() {
        for (width, height, expected) in [
            (100, 100, 117),
            (384, 384, 117),
            (640, 480, 209),
            (800, 800, 349),
            (1024, 768, 357),
            (1920, 1080, 369),
            (2000, 2000, 349),
            (5000, 5000, 349),
            (300, 50, 101),
            (9000, 1, 113),
            (16, 8192, 381),
            (100, 4036, 253),
            (4921, 353, 289),
            (97, 7289, 245),
        ] {
            assert_eq!(
                deepseek_image_tokens(width, height),
                expected,
                "unexpected image token price for {width}x{height}"
            );
        }
    }

    #[test]
    fn vision_route_prices_images_and_text_route_substitutes_them() {
        let resolved = resolve_adapter_options(&DeepSeekConfig::default()).expect("options");
        let adapter = DeepSeekAdapter::new(DeepSeekAdapterOptions {
            options: Arc::new(move || Ok(resolved.clone())),
            resolve_api_key: Arc::new(|_| Box::pin(async { Ok(None) })),
            resolve_attachments: None,
            provider_name: None,
            reasoning_wire_format: ReasoningWireFormat::DeepSeek,
        });
        let image = dsh_llm::ImageAttachmentRef {
            attachment_id: "sha256:vision".to_string(),
            media_type: Some("image/png".to_string()),
            bytes: Some(2048),
            width: Some(800),
            height: Some(800),
            name: None,
        };

        let vision = adapter
            .image_request_pricing(PROVIDER, "deepseek-v4-flash-vision-exp")
            .expect("vision pricing")(&[image.clone()]);
        assert_eq!(vision[0].visual_tokens, 349);
        assert!(vision[0].text.contains("request image 800x800px"));

        let text = adapter
            .image_request_pricing(PROVIDER, "deepseek-v4-flash")
            .expect("text pricing")(&[image]);
        assert_eq!(text[0].visual_tokens, 0);
        assert!(text[0].text.contains("text-only"));
        let unknown = adapter
            .image_request_pricing(PROVIDER, "future-model")
            .expect("unknown model remains image-capable")(&[
            dsh_llm::ImageAttachmentRef {
                attachment_id: "sha256:unknown".to_string(),
                media_type: Some("image/png".to_string()),
                bytes: Some(2048),
                width: Some(800),
                height: Some(800),
                name: None,
            },
        ]);
        assert!(unknown[0].visual_tokens > 0);
        assert!(!unknown[0].text.contains("text-only"));
    }

    #[tokio::test]
    async fn model_metadata_advertises_only_declared_image_routes() {
        let resolved = resolve_adapter_options(&DeepSeekConfig::default()).expect("options");
        let adapter = DeepSeekAdapter::new(DeepSeekAdapterOptions {
            options: Arc::new(move || Ok(resolved.clone())),
            resolve_api_key: Arc::new(|_| Box::pin(async { Ok(None) })),
            resolve_attachments: None,
            provider_name: None,
            reasoning_wire_format: ReasoningWireFormat::DeepSeek,
        });
        let listed = adapter.list_models(PROVIDER).await;
        let flash = listed
            .iter()
            .find(|model| model.id == "deepseek-v4-flash")
            .expect("flash model");
        let vision = listed
            .iter()
            .find(|model| model.id == "deepseek-v4-flash-vision-exp")
            .expect("vision model");
        assert_eq!(flash.input_modalities, Some(vec![ModelModality::Text]));
        assert_eq!(
            vision.input_modalities,
            Some(vec![ModelModality::Text, ModelModality::Image])
        );

        let uncatalogued = adapter.resolve_model(PROVIDER, "future-model", None).await;
        assert_eq!(uncatalogued.input_modalities, None);
    }
}

fn project_exact_request(
    options: &GenerateOptions,
    image_meta: &std::collections::HashMap<String, serialize::PreparedImageMeta>,
    representation: dsh_llm::RequestImageRepresentation,
) -> GenerateOptions {
    let exact = |image: &dsh_llm::ImageAttachmentRef| {
        image_meta
            .get(&image.attachment_id)
            .map_or(0, |meta| meta.bytes)
    };
    let (max_bytes, byte_quantum) = match representation {
        dsh_llm::RequestImageRepresentation::Raw => (
            DEFAULT_MAX_REQUEST_FILES_BYTES,
            DEFAULT_IMAGE_OFFLOAD_BYTE_QUANTUM,
        ),
        dsh_llm::RequestImageRepresentation::Base64 => (
            DEFAULT_MAX_INLINE_REQUEST_IMAGE_BYTES,
            DEFAULT_INLINE_IMAGE_OFFLOAD_BYTE_QUANTUM,
        ),
    };
    let mut projected = options.clone();
    projected.messages = dsh_llm::offload_request_images_with_policy(
        &options.messages,
        &dsh_llm::RequestImageOffloadPolicy {
            representation,
            max_images: Some(DEFAULT_MAX_IMAGES_PER_REQUEST),
            max_bytes: Some(max_bytes),
            count_quantum: Some(DEFAULT_IMAGE_OFFLOAD_COUNT_QUANTUM),
            byte_quantum: Some(byte_quantum),
            byte_length: Some(&exact),
        },
    );
    projected
}

async fn resolve_image_urls(
    options: &GenerateOptions,
    store: Option<&Arc<dyn dsh_attachment::AttachmentStore>>,
) -> Result<
    (
        std::collections::HashMap<String, String>,
        std::collections::HashMap<String, serialize::PreparedImageMeta>,
    ),
    LlmFailure,
> {
    use base64::Engine;
    let mut urls = std::collections::HashMap::new();
    let mut image_meta = std::collections::HashMap::new();
    for attachment in request_image_attachments(options) {
        let Some(store) = store else {
            return Err(failure(
                "DeepSeek image conversion requires the durable attachment service.",
                "UNSUPPORTED_CONTENT",
            ));
        };
        let reference = attachment_reference(attachment)?;
        let policy = dsh_attachment::RequestImagePolicy {
            max_pixels: 640_000,
            max_bytes: 1024 * 1024,
            preferred_media_type: dsh_attachment::ImageMediaType::Webp,
        };
        let version = store
            .read_image_request(&reference, &policy, None)
            .await
            .map_err(|error| {
                failure(format!("DeepSeek image read failed: {error}"), &error.code)
            })?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&version.data);
        image_meta.insert(
            attachment.attachment_id.clone(),
            serialize::PreparedImageMeta {
                attachment_id: attachment.attachment_id.clone(),
                bytes: version.data.len() as u64,
                width: version.width,
                height: version.height,
            },
        );
        urls.insert(
            attachment.attachment_id.clone(),
            format!("data:{};base64,{encoded}", version.media_type.as_str()),
        );
    }
    Ok((urls, image_meta))
}

fn attachment_reference(
    attachment: &dsh_llm::ImageAttachmentRef,
) -> Result<dsh_attachment::ImageAttachmentRef, LlmFailure> {
    let media_type = match attachment.media_type.as_deref() {
        Some("image/png") => dsh_attachment::ImageMediaType::Png,
        Some("image/jpeg") => dsh_attachment::ImageMediaType::Jpeg,
        Some("image/webp") => dsh_attachment::ImageMediaType::Webp,
        Some("image/gif") => dsh_attachment::ImageMediaType::Gif,
        _ => {
            return Err(failure(
                "DeepSeek image reference has no supported verified media type.",
                "UNSUPPORTED_CONTENT",
            ));
        }
    };
    Ok(dsh_attachment::ImageAttachmentRef {
        attachment_id: dsh_attachment::attachment_id(&attachment.attachment_id),
        media_type,
        bytes: attachment.bytes.ok_or_else(|| {
            failure(
                "DeepSeek image reference has no byte length.",
                "UNSUPPORTED_CONTENT",
            )
        })?,
        width: attachment.width.ok_or_else(|| {
            failure(
                "DeepSeek image reference has no width.",
                "UNSUPPORTED_CONTENT",
            )
        })?,
        height: attachment.height.ok_or_else(|| {
            failure(
                "DeepSeek image reference has no height.",
                "UNSUPPORTED_CONTENT",
            )
        })?,
        name: attachment.name.clone(),
    })
}

struct ResolvedRequestFiles {
    ids: std::collections::HashMap<String, String>,
    image_meta: std::collections::HashMap<String, serialize::PreparedImageMeta>,
    messages: Vec<dsh_llm::Message>,
    used: Vec<(dsh_attachment::ImageVariantId, DeepSeekFileId)>,
    index: DeepSeekUploadIndex,
    scope: crate::DeepSeekFileScope,
}

async fn resolve_image_file_ids(
    options: &GenerateOptions,
    connection: &ResolvedDeepSeekOptions,
    api_key: &str,
    store: Option<&Arc<dyn dsh_attachment::AttachmentStore>>,
) -> Result<Option<ResolvedRequestFiles>, LlmFailure> {
    let Some(store) = store else {
        return Ok(None);
    };
    let index_path = connection.files_index_path.clone().unwrap_or_else(|| {
        dsh_home_paths::resolve_dsh_home(None, &|name| std::env::var(name).ok())
            .join("llm-deepseek")
            .join("files-v3.json")
    });
    let index = DeepSeekUploadIndex::new(index_path);
    let scope = deepseek_file_scope(&connection.base_url, api_key);
    let files =
        DeepSeekFilesClient::new(&connection.base_url, api_key, connection.files_api_timeout);
    let policy = dsh_attachment::RequestImagePolicy {
        max_pixels: 640_000,
        max_bytes: 1024 * 1024,
        preferred_media_type: dsh_attachment::ImageMediaType::Webp,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| failure(error.to_string(), "FILES_API"))?
        .as_millis() as u64;
    let mut prepared = Vec::new();
    let mut image_meta = std::collections::HashMap::new();
    for attachment in request_image_attachments(options) {
        let reference = attachment_reference(attachment)?;
        let version = store
            .read_image_request(&reference, &policy, None)
            .await
            .map_err(|error| failure(error.to_string(), "FILES_API"))?;
        image_meta.insert(
            attachment.attachment_id.clone(),
            serialize::PreparedImageMeta {
                attachment_id: attachment.attachment_id.clone(),
                bytes: version.data.len() as u64,
                width: version.width,
                height: version.height,
            },
        );
        prepared.push((
            attachment.attachment_id.clone(),
            attachment.name.clone(),
            reference,
            version,
        ));
    }
    let exact_options = project_exact_request(
        options,
        &image_meta,
        dsh_llm::RequestImageRepresentation::Raw,
    );
    let retained: std::collections::HashSet<_> = request_image_attachments(&exact_options)
        .into_iter()
        .map(|attachment| attachment.attachment_id.clone())
        .collect();
    let mut ids = std::collections::HashMap::new();
    let mut used = Vec::new();
    for (attachment_id, name, reference, version) in prepared {
        if !retained.contains(&attachment_id) {
            continue;
        }
        let variant_lock = upload_lock(&scope, &version.variant_id);
        let _variant_guard = variant_lock.lock().await;
        if let Some(record) = index
            .get(&scope, &version.variant_id, now, 86_400_000)
            .await
            .map_err(|error| failure(error, "FILES_API"))?
        {
            used.push((record.variant_id.clone(), record.file_id.clone()));
            ids.insert(attachment_id.clone(), record.file_id.as_str().to_string());
            continue;
        }
        let uploaded = files
            .upload(
                version.data,
                version.media_type.as_str(),
                name.as_deref().unwrap_or("image.webp"),
                7 * 24 * 60 * 60,
            )
            .await
            .map_err(|error| failure(error.to_string(), "FILES_API"))?;
        let expires_at = uploaded
            .expires_at
            .ok_or_else(|| failure("DeepSeek Files upload omitted expiry", "FILES_API"))?
            * 1000;
        let candidate_file_id = uploaded.id.clone();
        let committed = index
            .commit(
                DeepSeekUploadRecord {
                    scope: scope.clone(),
                    attachment_id: reference.attachment_id,
                    variant_id: version.variant_id,
                    file_id: uploaded.id,
                    bytes: uploaded.bytes,
                    created_at: uploaded.created_at * 1000,
                    expires_at,
                },
                now,
                86_400_000,
            )
            .await
            .map_err(|error| failure(error, "FILES_API"))?;
        if !committed.accepted && committed.record.file_id != candidate_file_id {
            let _ = files.delete(&candidate_file_id).await;
        }
        for evicted in &committed.evicted {
            if evicted.file_id != committed.record.file_id {
                let _ = files.delete(&evicted.file_id).await;
            }
        }
        used.push((
            committed.record.variant_id.clone(),
            committed.record.file_id.clone(),
        ));
        ids.insert(attachment_id, committed.record.file_id.as_str().to_string());
    }
    if ids.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ResolvedRequestFiles {
            ids,
            image_meta,
            messages: exact_options.messages,
            used,
            index,
            scope,
        }))
    }
}

fn provider_rejected_file_id(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    let names_file = lower.contains("file")
        || lower.contains("file_id")
        || lower.contains("file-id")
        || lower.contains("file id");
    let missing = [
        "expired",
        "not found",
        "not_found",
        "deleted",
        "does not exist",
        "do not exist",
        "not created under this account",
        "not created under your account",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let invalid = lower.contains("invalid file_id")
        || lower.contains("invalid file-id")
        || lower.contains("invalid file id")
        || lower.contains("file_id invalid")
        || lower.contains("file-id invalid")
        || lower.contains("file id invalid");
    names_file && (missing || invalid)
}

fn detail_names_file_id(detail: &str, file_id: &DeepSeekFileId) -> bool {
    let value = file_id.as_str();
    detail.match_indices(value).any(|(index, _)| {
        let before = detail[..index].chars().next_back();
        let after = detail[index + value.len()..].chars().next();
        let boundary = |ch: Option<char>| {
            ch.is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))
        };
        boundary(before) && boundary(after)
    })
}

fn model_max_tokens(model: &str, connection: &ResolvedDeepSeekOptions) -> Option<u64> {
    let configured = connection
        .models
        .iter()
        .find(|entry| entry.id.eq_ignore_ascii_case(model))
        .and_then(|entry| entry.max_tokens);
    match (configured, inferred_model_max_tokens(model)) {
        (Some(configured), Some(known_limit)) => Some(configured.min(known_limit)),
        (Some(configured), None) => Some(configured),
        (None, inferred) => inferred,
    }
}

fn apply_model_max_tokens(options: &mut GenerateOptions, connection: &ResolvedDeepSeekOptions) {
    let Some(limit) = model_max_tokens(&options.model, connection) else {
        return;
    };
    options.max_tokens = Some(options.max_tokens.map_or(limit, |value| value.min(limit)));
}

fn map_reasoning_effort_for_request(
    options: &mut GenerateOptions,
    connection: &ResolvedDeepSeekOptions,
    reasoning_wire_format: ReasoningWireFormat,
) -> Result<(), LlmFailure> {
    if reasoning_wire_format != ReasoningWireFormat::OpenAi {
        return Ok(());
    }
    let Some(requested) = options.reasoning_effort.as_ref() else {
        return Ok(());
    };
    let Some(catalog) = connection
        .models
        .iter()
        .find(|model| model.id == options.model)
        .and_then(|model| model.reasoning_efforts.as_ref())
    else {
        return Ok(());
    };
    let Some(effort) = catalog
        .iter()
        .find(|effort| effort.id == requested.as_str())
    else {
        return Err(failure(
            format!(
                "provider model \"{}\" does not support reasoning effort \"{}\"",
                options.model,
                requested.as_str()
            ),
            "UNSUPPORTED_REASONING_EFFORT",
        ));
    };
    options.reasoning_effort = Some(reasoning_effort_id(&effort.wire));
    Ok(())
}

async fn request_chunks(
    options: GenerateOptions,
    connection: ResolvedDeepSeekOptions,
    api_key: String,
    provider_name: &str,
    reasoning_wire_format: ReasoningWireFormat,
    attachment_store: Option<Arc<dyn dsh_attachment::AttachmentStore>>,
    sender: &tokio::sync::mpsc::Sender<StreamChunk>,
    cancelled: Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<(), LlmFailure> {
    let mut options = project_estimated_request(&options);
    apply_model_max_tokens(&mut options, &connection);
    map_reasoning_effort_for_request(&mut options, &connection, reasoning_wire_format)?;
    if connection.api == "openai-responses" {
        let (image_urls, image_meta) =
            resolve_image_urls(&options, attachment_store.as_ref()).await?;
        let exact_options = project_exact_request(
            &options,
            &image_meta,
            dsh_llm::RequestImageRepresentation::Base64,
        );
        let chat_body = serialize::serialize_request_with_prepared_images(
            &exact_options,
            &connection.defaults,
            reasoning_wire_format,
            Some(&image_urls),
            None,
            Some(&image_meta),
        )?;
        return request_responses_chunks(
            &chat_body,
            &connection,
            &api_key,
            provider_name,
            &attribution_headers(&app_identity()),
            sender,
        )
        .await;
    }
    let url = endpoint_url(&connection.base_url, "openai-completions");
    let mut file_attempt = 0_u8;
    let mut response = loop {
        let resolved_files =
            resolve_image_file_ids(&options, &connection, &api_key, attachment_store.as_ref())
                .await;
        let (body, used_files) = match resolved_files {
            Ok(Some(files)) => {
                let mut exact_options = options.clone();
                exact_options.messages = files.messages.clone();
                (
                    serialize::serialize_request_with_prepared_images(
                        &exact_options,
                        &connection.defaults,
                        reasoning_wire_format,
                        None,
                        Some(&files.ids),
                        Some(&files.image_meta),
                    )?,
                    Some(files),
                )
            }
            _ => {
                let (image_urls, image_meta) =
                    resolve_image_urls(&options, attachment_store.as_ref()).await?;
                let exact_options = project_exact_request(
                    &options,
                    &image_meta,
                    dsh_llm::RequestImageRepresentation::Base64,
                );
                (
                    serialize::serialize_request_with_prepared_images(
                        &exact_options,
                        &connection.defaults,
                        reasoning_wire_format,
                        Some(&image_urls),
                        None,
                        Some(&image_meta),
                    )?,
                    None,
                )
            }
        };
        let encoded = serde_json::to_vec(&body).map_err(|error| {
            failure(
                format!("{provider_name} request encode failed: {error}"),
                "INVALID_REQUEST",
            )
        })?;
        let response = transport::post(
            &url,
            (!connection.keyless).then_some(api_key.as_str()),
            encoded,
            &attribution_headers(&app_identity()),
            cancelled.clone(),
        )
        .await
        .map_err(|error| {
            failure(
                format!("{provider_name} API request failed: {error}"),
                "TRANSPORT",
            )
        })?;
        if response.status.is_success() {
            break response;
        }
        let status = response.status;
        let headers = response.headers.clone();
        let error_body = response
            .collect_limited(8 * 1024 * 1024)
            .await
            .unwrap_or_default();
        let detail = String::from_utf8_lossy(&error_body);
        if let Some(files) = used_files.as_ref()
            && file_attempt == 0
            && provider_rejected_file_id(&detail)
        {
            let mut unique = files.used.clone();
            unique.sort_by(|a, b| {
                a.0.as_str()
                    .cmp(b.0.as_str())
                    .then_with(|| a.1.as_str().cmp(b.1.as_str()))
            });
            unique.dedup();
            let exact: Vec<_> = unique
                .iter()
                .filter(|(_, file_id)| detail_names_file_id(&detail, file_id))
                .cloned()
                .collect();
            let stale = if exact.is_empty() { unique } else { exact };
            for (variant, file_id) in stale {
                files
                    .index
                    .invalidate_exact(&files.scope, &variant, &file_id)
                    .await
                    .map_err(|error| failure(error, "FILES_API"))?;
            }
            file_attempt += 1;
            continue;
        }
        let image_schema_mismatch = used_files.is_none()
            && detail.contains("unknown variant `image_url`")
            && detail.contains("expected `text`");
        if image_schema_mismatch {
            let (image_urls, image_meta) =
                resolve_image_urls(&options, attachment_store.as_ref()).await?;
            let exact_options = project_exact_request(
                &options,
                &image_meta,
                dsh_llm::RequestImageRepresentation::Base64,
            );
            let inline_body = serialize::serialize_request_with_prepared_images(
                &exact_options,
                &connection.defaults,
                reasoning_wire_format,
                Some(&image_urls),
                None,
                Some(&image_meta),
            )?;
            return request_responses_chunks(
                &inline_body,
                &connection,
                &api_key,
                provider_name,
                &attribution_headers(&app_identity()),
                sender,
            )
            .await;
        }
        return Err(http_failure(status, &headers, &error_body, provider_name));
    };

    let mut parser = sse::SseParser::new();
    let mut translator = translate::Translator::new();
    let mut emitted_chunks = 0_usize;
    let mut received_bytes = 0_usize;
    let done_seen = false;
    loop {
        let next_data = tokio::time::timeout(connection.stream_idle_timeout, response.next_data());
        tokio::pin!(next_data);
        let bytes = tokio::select! {
            result = &mut next_data => result
                .map_err(|_| failure("DeepSeek stream idle timeout", "TIMEOUT"))?
                .map_err(|error| {
                    if done_seen {
                        failure("DeepSeek stream closed after DONE", "STREAM_CLOSED")
                    } else {
                        failure(format!("{provider_name} API stream failed: {error}"), "TRANSPORT")
                    }
                })?,
            _ = sender.closed() => return Err(failure("DeepSeek stream consumer closed", "CANCELLED")),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                if cancelled.as_ref().is_some_and(|is_cancelled| is_cancelled()) {
                    return Err(failure("DeepSeek stream cancelled", "CANCELLED"));
                }
                continue;
            },
        };
        let Some(bytes) = bytes else {
            break;
        };
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
            emitted_chunks = emitted_chunks
                .checked_add(translated.len())
                .ok_or_else(|| {
                    failure(
                        "DeepSeek stream chunk count overflowed",
                        "RESPONSE_TOO_LARGE",
                    )
                })?;
            if emitted_chunks > MAX_SUCCESS_STREAM_CHUNKS {
                return Err(failure(
                    "DeepSeek success response emitted too many chunks",
                    "RESPONSE_TOO_LARGE",
                ));
            }
            for chunk in translated {
                sender
                    .send(chunk)
                    .await
                    .map_err(|_| failure("DeepSeek stream consumer closed", "CANCELLED"))?;
            }
            if done {
                return Ok(());
            }
        }
    }
    for payload in parser.finish_at_eof()? {
        let translated = translator.consume(&payload)?;
        emitted_chunks = emitted_chunks
            .checked_add(translated.len())
            .ok_or_else(|| {
                failure(
                    "DeepSeek stream chunk count overflowed",
                    "RESPONSE_TOO_LARGE",
                )
            })?;
        if emitted_chunks > MAX_SUCCESS_STREAM_CHUNKS {
            return Err(failure(
                "DeepSeek success response emitted too many chunks",
                "RESPONSE_TOO_LARGE",
            ));
        }
        for chunk in translated {
            sender
                .send(chunk)
                .await
                .map_err(|_| failure("DeepSeek stream consumer closed", "CANCELLED"))?;
        }
    }
    for chunk in translator.close_after_explicit_finish()? {
        sender
            .send(chunk)
            .await
            .map_err(|_| failure("DeepSeek stream consumer closed", "CANCELLED"))?;
    }
    Ok(())
}

async fn request_responses_chunks(
    chat_body: &serde_json::Value,
    connection: &ResolvedDeepSeekOptions,
    api_key: &str,
    provider_name: &str,
    attribution: &[(String, String)],
    sender: &tokio::sync::mpsc::Sender<StreamChunk>,
) -> Result<(), LlmFailure> {
    let body = responses::request_from_chat(chat_body)?;
    let encoded = serde_json::to_vec(&body).map_err(|error| {
        failure(
            format!("Responses request encode failed: {error}"),
            "INVALID_REQUEST",
        )
    })?;
    let url = endpoint_url(&connection.base_url, "openai-responses");
    let mut response = transport::post(
        &url,
        (!connection.keyless).then_some(api_key),
        encoded,
        attribution,
        None,
    )
    .await
    .map_err(|error| {
        failure(
            format!("Responses API request failed: {error}"),
            "TRANSPORT",
        )
    })?;
    if !response.status.is_success() {
        let status = response.status;
        let headers = response.headers.clone();
        let body = response
            .collect_limited(8 * 1024 * 1024)
            .await
            .unwrap_or_default();
        return Err(http_failure(status, &headers, &body, provider_name));
    }
    let mut parser = sse::SseParser::new();
    let mut translator = responses::ResponsesTranslator::default();
    let mut emitted_chunks = 0_usize;
    let mut received_bytes = 0_usize;
    while let Some(bytes) =
        tokio::time::timeout(connection.stream_idle_timeout, response.next_data())
            .await
            .map_err(|_| failure("Responses stream idle timeout", "TIMEOUT"))?
            .map_err(|error| {
                failure(format!("Responses API stream failed: {error}"), "TRANSPORT")
            })?
    {
        received_bytes = received_bytes.saturating_add(bytes.len());
        if received_bytes > MAX_SUCCESS_RESPONSE_BYTES {
            return Err(failure(
                "Responses success response exceeded 8 MiB",
                "RESPONSE_TOO_LARGE",
            ));
        }
        for payload in parser.push(&bytes)? {
            let translated = translator.consume(&payload)?;
            emitted_chunks = emitted_chunks.saturating_add(translated.len());
            if emitted_chunks > MAX_SUCCESS_STREAM_CHUNKS {
                return Err(failure(
                    "Responses success response emitted too many chunks",
                    "RESPONSE_TOO_LARGE",
                ));
            }
            for chunk in translated {
                sender
                    .send(chunk)
                    .await
                    .map_err(|_| failure("Responses stream consumer closed", "CANCELLED"))?;
            }
        }
    }
    for payload in parser.finish_at_eof()? {
        for chunk in translator.consume(&payload)? {
            sender
                .send(chunk)
                .await
                .map_err(|_| failure("Responses stream consumer closed", "CANCELLED"))?;
        }
    }
    translator.finish()?;
    Ok(())
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
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn drive_owned_request(
    options: GenerateOptions,
    options_resolver: OptionsResolver,
    key_resolver: ApiKeyResolver,
    attachment_resolver: Option<AttachmentResolver>,
    provider_name: String,
    reasoning_wire_format: ReasoningWireFormat,
    sender: tokio::sync::mpsc::Sender<StreamChunk>,
) {
    let cancelled = options.signal.clone();
    let connection = match options_resolver() {
        Ok(connection) => connection,
        Err(error) => {
            let _ = sender.send(error_finish(error.failure)).await;
            return;
        }
    };
    let raw_key = if connection.keyless {
        "keyless-anonymous-route".to_string()
    } else {
        match key_resolver(&connection).await {
            Ok(Some(key)) => key,
            Ok(None) => {
                let _ = sender
                    .send(error_finish(failure(
                        format!(
                            "llm-deepseek: no API key resolved from {}",
                            connection.api_key_env
                        ),
                        "MISSING_CREDENTIAL",
                    )))
                    .await;
                return;
            }
            Err(error) => {
                let _ = sender.send(error_finish(error.failure)).await;
                return;
            }
        }
    };
    let api_key = match assert_usable_api_key(&raw_key, "llm-deepseek", &connection.api_key_env) {
        Ok(key) => key,
        Err(error) => {
            let _ = sender.send(error_finish(error.failure)).await;
            return;
        }
    };
    let attachment_store = attachment_resolver.and_then(|resolve| resolve());
    if let Err(failure) = request_chunks(
        options,
        connection,
        api_key,
        &provider_name,
        reasoning_wire_format,
        attachment_store,
        &sender,
        cancelled.clone(),
    )
    .await
    {
        let _ = sender.send(error_finish(failure)).await;
    }
}

#[async_trait::async_trait]
impl LlmAdapter for DeepSeekAdapter {
    fn provider_info(&self, provider: &str) -> LlmProviderInfo {
        LlmProviderInfo {
            id: provider.to_string(),
            name: self
                .config
                .provider_name
                .clone()
                .unwrap_or_else(|| "DeepSeek".to_string()),
        }
    }

    fn provider_retry_policy(&self, _provider: &str) -> Option<ResolvedRetryPolicy> {
        (self.config.options)()
            .ok()
            .map(|options| options.retry_policy)
    }

    fn image_request_pricing(
        &self,
        _provider: &str,
        model: &str,
    ) -> Option<dsh_llm::LlmImageRequestPricing> {
        let options = (self.config.options)().ok()?;
        let image_input = options
            .models
            .iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| entry.image_input)
            .unwrap_or(true);
        let model = model.to_string();
        Some(Arc::new(move |images| {
            if !image_input {
                return images
                    .iter()
                    .map(|image| dsh_llm::LlmImageRequestPrice {
                        visual_tokens: 0,
                        text: format!(
                            "[image {} is unavailable to text-only model {model}]",
                            image.attachment_id
                        ),
                    })
                    .collect();
            }
            images
                .iter()
                .map(|image| {
                    let (width, height) = request_image_dimensions(
                        image.width.unwrap_or(1),
                        image.height.unwrap_or(1),
                        640_000,
                    );
                    dsh_llm::LlmImageRequestPrice {
                        visual_tokens: deepseek_image_tokens(width, height),
                        text: format!(
                            "Image {}; request image {}x{}px.",
                            image.attachment_id, width, height
                        ),
                    }
                })
                .collect()
        }))
    }

    async fn list_models(&self, provider: &str) -> Vec<LlmModelInfo> {
        let Ok(options) = (self.config.options)() else {
            return Vec::new();
        };
        options
            .models
            .into_iter()
            .map(|model| {
                let input_modalities = model_modalities(Some(&model));
                LlmModelInfo {
                    provider: provider.to_string(),
                    name: model.name.clone().unwrap_or_else(|| model.id.clone()),
                    id: model.id,
                    description: model.description,
                    input_modalities,
                }
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
        if let Some(catalog) = configured.and_then(|entry| entry.reasoning_efforts.as_ref()) {
            let efforts = catalog
                .iter()
                .map(|effort| LlmReasoningEffortInfo {
                    id: reasoning_effort_id(&effort.id),
                    name: effort.name.clone(),
                    description: None,
                })
                .collect::<Vec<_>>();
            return LlmResolvedModelInfo {
                provider: provider.to_string(),
                id: model.to_string(),
                name: configured
                    .and_then(|entry| entry.name.clone())
                    .unwrap_or_else(|| model.to_string()),
                description: configured.and_then(|entry| entry.description.clone()),
                input_modalities: model_modalities(configured),
                context: Some(LlmModelContext {
                    context_window: configured
                        .and_then(|entry| entry.context_window)
                        .unwrap_or(options.default_context_window),
                }),
                default_max_tokens: Some(
                    model_max_tokens(model, &options).unwrap_or(options.max_tokens),
                ),
                reasoning: (!efforts.is_empty()).then_some(LlmModelReasoningInfo {
                    efforts,
                    // A declared catalog advertises choices but does not
                    // silently opt ordinary calls into paid reasoning.
                    default_effort: None,
                }),
            };
        }
        if self.config.reasoning_wire_format == ReasoningWireFormat::OpenAi {
            return LlmResolvedModelInfo {
                provider: provider.to_string(),
                id: model.to_string(),
                name: configured
                    .and_then(|entry| entry.name.clone())
                    .unwrap_or_else(|| model.to_string()),
                description: configured.and_then(|entry| entry.description.clone()),
                input_modalities: model_modalities(configured),
                context: Some(LlmModelContext {
                    context_window: configured
                        .and_then(|entry| entry.context_window)
                        .unwrap_or(options.default_context_window),
                }),
                default_max_tokens: Some(
                    model_max_tokens(model, &options).unwrap_or(options.max_tokens),
                ),
                reasoning: None,
            };
        }
        let effort = match options.defaults.reasoning_effort {
            Some(DeepSeekReasoningEffort::Off) => "off",
            Some(DeepSeekReasoningEffort::Low) => "low",
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
            ["off", "low", "high", "max"]
                .into_iter()
                .map(|id| LlmReasoningEffortInfo {
                    id: reasoning_effort_id(id),
                    name: match id {
                        "off" => "Off",
                        "low" => "Low",
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
            input_modalities: model_modalities(configured),
            context: Some(LlmModelContext {
                context_window: configured
                    .and_then(|entry| entry.context_window)
                    .unwrap_or(options.default_context_window),
            }),
            default_max_tokens: Some(
                model_max_tokens(model, &options).unwrap_or(options.max_tokens),
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
        let attachment_resolver = self.config.resolve_attachments.clone();
        let provider_name = self
            .config
            .provider_name
            .clone()
            .unwrap_or_else(|| "DeepSeek".to_string());
        let reasoning_wire_format = self.config.reasoning_wire_format;
        Box::pin(async_stream::stream! {
            let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
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
                            _ = drive_owned_request(options, options_resolver, key_resolver, attachment_resolver, provider_name, reasoning_wire_format, sender) => {},
                            _ = cancelled => {},
                        }
                    });
                    runtime.shutdown_timeout(Duration::from_millis(250));
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

#[cfg(test)]
mod endpoint_tests {
    use super::{endpoint_url, inferred_model_max_tokens};

    #[test]
    fn completions_replaces_responses_endpoint_suffix() {
        assert_eq!(
            endpoint_url(
                "https://ark.cn-beijing.volces.com/api/v3/responses",
                "openai-completions"
            ),
            "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
        );
    }

    #[test]
    fn responses_replaces_chat_completions_endpoint_suffix() {
        assert_eq!(
            endpoint_url(
                "https://example.test/v1/chat/completions",
                "openai-responses"
            ),
            "https://example.test/v1/responses"
        );
    }

    #[test]
    fn glm_5_3_uses_its_declared_output_limit() {
        assert_eq!(inferred_model_max_tokens("glm-5.3"), Some(131_072));
        assert_eq!(inferred_model_max_tokens("GLM-5.3"), Some(131_072));
        assert_eq!(inferred_model_max_tokens("glm-5.3-flash"), Some(131_072));
        assert_eq!(inferred_model_max_tokens("GLM-5.3-FLASH"), Some(131_072));
        assert_eq!(inferred_model_max_tokens("glm-5.2"), None);
    }
}
