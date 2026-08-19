//! Shared route, framing, timeout, assembly, and validation policy for
//! model-backed session-title providers. Rust port of
//! `packages/session/session-title-llm/src/index.ts`.
//!
//! # Deviations
//!
//! - `resolveSessionTitleLlmConfig` takes a JSON value (the loader/settings
//!   contract shape) and validates it with the TS messages, including the
//!   unknown-key rejection; the typed struct is the validated output.
//! - The `AbortSignal`/`deadline` fusion becomes the seam-local
//!   [`dsh_session_title::SessionTitleSignal`] plus a
//!   [`dsh_timeout::DeadlineSignal`]; the stream loop races both, and the
//!   timeout error carries `code`/`timeoutMs` (the TS reason fields).
//! - `Object.isFrozen` has no Rust equivalent (values are owned).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use cordis::Context;
use dsh_llm::{
    BlockAssembler, ContentBlock, FinishReason, GenerateOptions, LlmRuntime, MessageSource,
    create_user_message,
};
use dsh_session_title::{
    SessionTitleModelProvenance, SessionTitleProvider, SessionTitleProviderId,
    SessionTitleProviderRequest, SessionTitleProviderResult, SessionTitleUserMessage,
    normalize_session_title,
};
use dsh_timeout::{MAX_TIMER_DELAY_MS, deadline};

/// Capability-owned timeout reason code for auxiliary title requests (TS
/// `SESSION_TITLE_TIMEOUT_CODE`).
pub const SESSION_TITLE_TIMEOUT_CODE: &str = "SESSION_TITLE_TIMEOUT";

/// Required deployment policy for one model-backed title plugin (TS
/// `SessionTitleLlmConfig`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTitleLlmConfig {
    /// Target word count for non-CJK titles.
    pub target_words: u64,
    /// Target character count for Chinese, Japanese, or Korean titles.
    pub target_cjk_characters: u64,
    /// Maximum UTF-8 bytes in the final JSON-framed user prompt.
    pub max_input_bytes: u64,
    /// Auxiliary generation output-token cap.
    pub max_output_tokens: u64,
    /// End-to-end auxiliary request deadline in milliseconds.
    pub timeout_ms: u64,
    /// Optional explicit provider route; must be paired with `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Optional explicit model id; must be paired with `provider`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A generation failure carrying the TS message plus optional capability
/// code/timeout fields (the TS `Error & { code?, timeoutMs? }`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTitleLlmError {
    pub message: String,
    pub code: Option<String>,
    pub timeout_ms: Option<u64>,
}

impl SessionTitleLlmError {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            timeout_ms: None,
        }
    }
}

impl std::fmt::Display for SessionTitleLlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessionTitleLlmError {}

/// The complete configuration key set (TS `CONFIG_KEYS`).
const CONFIG_KEYS: [&str; 7] = [
    "targetWords",
    "targetCjkCharacters",
    "maxInputBytes",
    "maxOutputTokens",
    "timeoutMs",
    "provider",
    "model",
];

/// Validate and detach required model-provider configuration (TS
/// `resolveSessionTitleLlmConfig`). Accepts the loader-supplied JSON shape
/// and rejects unknown keys, non-positive integers, an over-long timeout,
/// and incomplete route pairs with the TS messages.
pub fn resolve_session_title_llm_config(
    value: &JsonValue,
) -> Result<SessionTitleLlmConfig, String> {
    let Some(object) = value.as_object() else {
        return Err("session-title-llm: configuration is required".to_string());
    };
    for key in object.keys() {
        if !CONFIG_KEYS.contains(&key.as_str()) {
            return Err(format!("session-title-llm: unknown config key \"{key}\""));
        }
    }
    let integer = |name: &str| -> Result<u64, String> {
        match object.get(name).and_then(|value| value.as_u64()) {
            Some(value) if value > 0 => Ok(value),
            _ => Err(format!(
                "session-title-llm: {name} must be a positive integer"
            )),
        }
    };
    let target_words = integer("targetWords")?;
    let target_cjk_characters = integer("targetCjkCharacters")?;
    let max_input_bytes = integer("maxInputBytes")?;
    let max_output_tokens = integer("maxOutputTokens")?;
    let timeout_ms = integer("timeoutMs")?;
    if timeout_ms > MAX_TIMER_DELAY_MS {
        return Err(format!(
            "session-title-llm: timeoutMs must not exceed {MAX_TIMER_DELAY_MS}"
        ));
    }
    let provider = object.get("provider").and_then(|value| value.as_str());
    let model = object.get("model").and_then(|value| value.as_str());
    let has_provider = provider.is_some();
    let has_model = model.is_some();
    if has_provider != has_model {
        return Err("session-title-llm: provider and model must be supplied together".to_string());
    }
    let (provider, model) = match (provider, model) {
        (Some(provider), Some(model)) if !provider.is_empty() && !model.is_empty() => {
            (Some(provider.to_string()), Some(model.to_string()))
        }
        (None, None) => (None, None),
        _ => {
            return Err(
                "session-title-llm: provider and model overrides must be non-empty strings"
                    .to_string(),
            );
        }
    };
    Ok(SessionTitleLlmConfig {
        target_words,
        target_cjk_characters,
        max_input_bytes,
        max_output_tokens,
        timeout_ms,
        provider,
        model,
    })
}

/// Select the provider-owned message subset from one fixed service revision
/// (TS `SessionTitleLlmMessageSelector`).
pub type SessionTitleLlmMessageSelector = Arc<
    dyn Fn(Vec<SessionTitleUserMessage>) -> Result<Vec<SessionTitleUserMessage>, String>
        + Send
        + Sync,
>;

/// Resolve the explicit pair or the exact route captured from
/// `request/header` (TS `resolveRoute`).
fn resolve_route(
    config: &SessionTitleLlmConfig,
    request: &SessionTitleProviderRequest,
) -> Result<SessionTitleModelProvenance, SessionTitleLlmError> {
    if let (Some(provider), Some(model)) = (&config.provider, &config.model) {
        return Ok(SessionTitleModelProvenance {
            provider: provider.clone(),
            model: model.clone(),
        });
    }
    request.route.clone().ok_or_else(|| {
        SessionTitleLlmError::plain(
            "session-title-llm: no logged request route is available; configure provider and model together",
        )
    })
}

/// Stable language-aware system instruction shared by both provider plugins
/// (TS `systemPrompt`).
pub fn system_prompt(config: &SessionTitleLlmConfig) -> String {
    [
        "Create a concise title for an AI coding-assistant session from the supplied human messages.",
        "Return only the title on one line, **in plain text of natural language**, with no quotes, prefix, explanation, Markdown, XML, or terminal control codes. No code is allowed.",
        "Use the language of the messages.",
        &format!(
            "Aim for about {} words in non-CJK languages or {} CJK characters.",
            config.target_words, config.target_cjk_characters
        ),
    ]
    .join("\n")
}

/// Frame exact messages as JSON so user text cannot break structural
/// delimiters (TS `frameMessages`).
pub fn frame_messages(messages: &[SessionTitleUserMessage]) -> String {
    format!(
        "Generate the session title from this JSON array of human messages:\n{}",
        serde_json::to_string(messages).expect("title messages serialize")
    )
}

/// Translate terminal finish reasons into an auxiliary-call failure (TS
/// `finishError`).
fn finish_error(finish: &FinishReason) -> Result<(), SessionTitleLlmError> {
    match finish {
        FinishReason::Stop => Ok(()),
        FinishReason::Error { failure } | FinishReason::Aborted { failure } => {
            Err(SessionTitleLlmError {
                message: failure.message.clone(),
                code: Some(failure.code.clone()),
                timeout_ms: None,
            })
        }
        FinishReason::MaxTokens => Err(SessionTitleLlmError::plain(
            "session-title-llm: title output reached maxOutputTokens",
        )),
        FinishReason::ToolCalls => Err(SessionTitleLlmError::plain(
            "session-title-llm: title model unexpectedly requested a tool",
        )),
    }
}

/// Register one model-backed provider through the shared configuration and
/// call policy (TS `registerSessionTitleLlmProvider`).
pub fn register_session_title_llm_provider(
    ctx: &Context,
    config: SessionTitleLlmConfig,
    id: &str,
    automatic: dsh_session_title::SessionTitleAutomaticMode,
    select_messages: SessionTitleLlmMessageSelector,
) -> Result<cordis::Disposer, String> {
    let title_service = ctx
        .get_typed::<Arc<dsh_session_title::SessionTitleService>>("sessionTitle", false)
        .ok_or_else(|| "sessionTitle service is not configured".to_string())?;
    let provider = LlmTitleProvider {
        id: dsh_session_title::session_title_provider_id(id.to_string()),
        automatic,
        ctx: ctx.clone(),
        config: Arc::new(config),
        select_messages,
    };
    title_service.register(ctx, Arc::new(provider))
}

/// One model-backed provider instance (the TS inline object registered with
/// the service).
struct LlmTitleProvider {
    id: SessionTitleProviderId,
    automatic: dsh_session_title::SessionTitleAutomaticMode,
    ctx: Context,
    config: Arc<SessionTitleLlmConfig>,
    select_messages: SessionTitleLlmMessageSelector,
}

#[async_trait::async_trait]
impl SessionTitleProvider for LlmTitleProvider {
    fn id(&self) -> &SessionTitleProviderId {
        &self.id
    }

    fn automatic(&self) -> dsh_session_title::SessionTitleAutomaticMode {
        self.automatic
    }

    async fn generate(
        &self,
        request: SessionTitleProviderRequest,
    ) -> Result<SessionTitleProviderResult, dsh_session_title::SessionTitleError> {
        let selected = (self.select_messages)(request.messages.clone())
            .map_err(|error| dsh_session_title::SessionTitleError::new(error))?;
        generate_session_title_with_llm(&self.ctx, &self.config, request, selected, self.id.clone())
            .await
            .map_err(|error| dsh_session_title::SessionTitleError::new(error.message))
    }
}

/// Generate one title through the shared auxiliary LLM call (TS
/// `generateSessionTitleWithLlm`).
pub async fn generate_session_title_with_llm(
    ctx: &Context,
    config: &SessionTitleLlmConfig,
    request: SessionTitleProviderRequest,
    selected_messages: Vec<SessionTitleUserMessage>,
    title_provider: SessionTitleProviderId,
) -> Result<SessionTitleProviderResult, SessionTitleLlmError> {
    if let Some(reason) = request.signal.abort_reason() {
        return Err(SessionTitleLlmError::plain(reason));
    }
    if selected_messages.is_empty() {
        return Err(SessionTitleLlmError::plain(
            "session-title-llm: at least one source message is required",
        ));
    }
    let framed_input = frame_messages(&selected_messages);
    let input_bytes = framed_input.len() as u64;
    if input_bytes > config.max_input_bytes {
        return Err(SessionTitleLlmError::plain(format!(
            "session-title-llm: input is {input_bytes} bytes, exceeding maxInputBytes {}",
            config.max_input_bytes
        )));
    }
    let route = resolve_route(config, &request)?;
    let messages = vec![create_user_message(
        vec![ContentBlock::Text { text: framed_input }],
        MessageSource::Plugin {
            plugin: "dsh-session-title-llm".to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    )];
    let system = system_prompt(config);
    let call_deadline = Arc::new(deadline(
        None,
        config.timeout_ms,
        SESSION_TITLE_TIMEOUT_CODE,
    ));
    let request_signal = request.signal.clone();
    let signal_predicate: Arc<dyn Fn() -> bool + Send + Sync> = {
        let deadline = call_deadline.clone();
        let request_signal = request_signal.clone();
        Arc::new(move || request_signal.is_aborted() || deadline.signal.is_cancelled())
    };
    let options = GenerateOptions {
        provider: route.provider.clone(),
        model: route.model.clone(),
        reasoning_effort: None,
        messages: messages.clone(),
        system: Some(system.clone()),
        tools: None,
        temperature: None,
        max_tokens: Some(config.max_output_tokens),
        stop: None,
        signal: Some(signal_predicate),
        session_id: Some(request.session.id().to_string()),
        purpose: Some("session-title".to_string()),
        agent_loop_request: false,
    };
    request
        .session
        .append(
            "session/title-llm-request",
            serde_json::json!({
                "titleProvider": title_provider,
                "messageSeqs": selected_messages.iter().map(|message| message.seq).collect::<Vec<u64>>(),
                "route": route,
                "system": system,
                "messages": messages,
                "maxTokens": config.max_output_tokens,
            }),
            None,
        )
        .map_err(|error| SessionTitleLlmError::plain(format!("title request append failed: {error}")))?;
    if let Some(reason) = request.signal.abort_reason() {
        return Err(SessionTitleLlmError::plain(reason));
    }

    let llm: Arc<Arc<LlmRuntime>> = ctx
        .get_typed::<Arc<LlmRuntime>>("llm", false)
        .ok_or_else(|| SessionTitleLlmError::plain("llm service is not configured"))?;
    let mut stream = Box::pin(llm.stream(options));
    let mut assembler = BlockAssembler::new();
    loop {
        tokio::select! {
            chunk = futures::StreamExt::next(&mut stream) => {
                match chunk {
                    Some(chunk) => {
                        if call_deadline.signal.is_cancelled() {
                            return Err(timeout_error(&call_deadline.signal));
                        }
                        if let Some(reason) = request.signal.abort_reason() {
                            return Err(SessionTitleLlmError::plain(reason));
                        }
                        assembler.push(&chunk);
                    }
                    None => break,
                }
            }
            _ = call_deadline.signal.cancelled() => {
                return Err(timeout_error(&call_deadline.signal));
            }
            _ = request.signal.cancelled() => {
                let reason = request.signal.abort_reason()
                    .unwrap_or_else(|| "session title generation aborted".to_string());
                return Err(SessionTitleLlmError::plain(reason));
            }
        }
    }
    if let Some(reason) = request.signal.abort_reason() {
        return Err(SessionTitleLlmError::plain(reason));
    }
    finish_error(&assembler.finish())?;
    let blocks = assembler.blocks();
    if blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolCall { .. }))
    {
        return Err(SessionTitleLlmError::plain(
            "session-title-llm: title output must contain text only",
        ));
    }
    let text = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<&str>>()
        .join(" ");
    let title = normalize_session_title(&text, u64::MAX);
    if title.is_empty() {
        return Err(SessionTitleLlmError::plain(
            "session-title-llm: title model produced no text",
        ));
    }
    Ok(SessionTitleProviderResult {
        title,
        message_seqs: selected_messages
            .iter()
            .map(|message| message.seq)
            .collect(),
        model: Some(route),
    })
}

fn timeout_error(signal: &dsh_timeout::DeadlineSignal) -> SessionTitleLlmError {
    let reason = signal.reason();
    SessionTitleLlmError {
        message: reason
            .as_ref()
            .map(|reason| format!("{reason}"))
            .unwrap_or_else(|| SESSION_TITLE_TIMEOUT_CODE.to_string()),
        code: Some(SESSION_TITLE_TIMEOUT_CODE.to_string()),
        timeout_ms: reason.map(|reason| reason.timeout_ms),
    }
}

/// The loader/settings schema shared by both provider plugins (TS
/// `SessionTitleLlmConfigSchema`; the field validators stay shared).
pub fn config_schema() -> dsh_schemastery::Schema {
    use dsh_schemastery::Schema;
    let mut properties = indexmap::IndexMap::new();
    for field in [
        "targetWords",
        "targetCjkCharacters",
        "maxInputBytes",
        "maxOutputTokens",
        "timeoutMs",
    ] {
        properties.insert(field.to_string(), Schema::number());
    }
    properties.insert("provider".to_string(), Schema::string());
    properties.insert("model".to_string(), Schema::string());
    Schema::object(properties)
}
