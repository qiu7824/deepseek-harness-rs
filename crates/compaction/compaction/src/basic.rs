use std::sync::Arc;

use cordis::{ArcValue, InjectSpec, Plugin, PluginError, arc};
use dsh_commands::CommandId;
use dsh_llm::{
    BlockAssembler, ContentBlock, FinishReason, GenerateOptions, LlmRuntime, Message,
    MessageSource, create_user_message,
};
use dsh_session::{
    Session, SessionStore, SurfaceIntent, SurfaceOp, derive_event_message, fold_request_header,
};
use dsh_token_meter::TokenMeter;
use futures::StreamExt;
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub enum RetentionConfig {
    Ratio(f64),
    Tokens(u64),
}

#[derive(Debug, Clone, Default)]
pub struct ModelCompactPolicyConfig {
    pub provider: String,
    pub model: String,
    pub threshold_ratio: Option<f64>,
    pub retention: Option<RetentionConfig>,
    pub summarization_provider: Option<String>,
    pub summarization_model: Option<String>,
    pub max_tokens: Option<u64>,
    pub compaction_retries: Option<u64>,
    pub max_overflow_retries: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct BasicCompactionConfig {
    pub threshold_ratio: Option<f64>,
    pub retention: Option<RetentionConfig>,
    pub summarization_provider: Option<String>,
    pub summarization_model: Option<String>,
    pub max_tokens: Option<u64>,
    pub compaction_retries: Option<u64>,
    pub max_overflow_retries: Option<u64>,
    pub model_policies: Vec<ModelCompactPolicyConfig>,
    pub auto: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub threshold_ratio: f64,
    pub retention: RetentionConfig,
    pub summarization_provider: String,
    pub summarization_model: String,
    pub max_tokens: u64,
    pub compaction_retries: u64,
    pub max_overflow_retries: u64,
    pub model_policies: Vec<ModelCompactPolicyConfig>,
    pub auto: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedTargetPolicy {
    pub provider: String,
    pub model: String,
    pub threshold_ratio: f64,
    pub retention: RetentionConfig,
    pub summarization_provider: String,
    pub summarization_model: String,
    pub max_tokens: u64,
    pub compaction_retries: u64,
    pub max_overflow_retries: u64,
}

#[derive(Debug, Clone)]
pub struct ResolvedCompactSpec {
    pub context_window: u64,
    pub threshold_tokens: u64,
    pub retain_tokens: u64,
}

fn validate_ratio(name: &str, value: f64) -> Result<(), String> {
    if value.is_finite() && value > 0.0 && value <= 1.0 {
        Ok(())
    } else {
        Err(format!("{name} ({value}) must be a number in (0, 1]"))
    }
}

fn validate_retention(
    threshold: f64,
    retention: &RetentionConfig,
    name: &str,
) -> Result<(), String> {
    if let RetentionConfig::Ratio(value) = retention {
        validate_ratio(&format!("{name}.retainRatio"), *value)?;
        if *value >= threshold {
            return Err(format!(
                "{name}: retainRatio ({value}) must be less than the resolved thresholdRatio ({threshold})"
            ));
        }
    }
    Ok(())
}

fn validate_summary_pair(
    provider: &Option<String>,
    model: &Option<String>,
    name: &str,
) -> Result<(), String> {
    match (provider, model) {
        (None, None) => Ok(()),
        (Some(provider), Some(model)) if provider.is_empty() == model.is_empty() => Ok(()),
        _ => Err(format!(
            "{name}: summarizationProvider and summarizationModel must be set together as an empty or non-empty pair"
        )),
    }
}

pub fn resolve_config(config: BasicCompactionConfig) -> Result<ResolvedConfig, String> {
    let threshold_ratio = config.threshold_ratio.unwrap_or(0.8);
    validate_ratio("BasicCompactionConfig.thresholdRatio", threshold_ratio)?;
    let retention = config
        .retention
        .clone()
        .unwrap_or(RetentionConfig::Ratio(0.16));
    validate_retention(threshold_ratio, &retention, "BasicCompactionConfig")?;
    validate_summary_pair(
        &config.summarization_provider,
        &config.summarization_model,
        "BasicCompactionConfig",
    )?;
    let mut seen = std::collections::HashSet::new();
    for (index, policy) in config.model_policies.iter().enumerate() {
        if policy.provider.is_empty() || policy.model.is_empty() {
            return Err(format!(
                "BasicCompactionConfig: modelPolicies[{index}] provider and model must be non-empty strings"
            ));
        }
        if !seen.insert((policy.provider.clone(), policy.model.clone())) {
            return Err(format!(
                "BasicCompactionConfig: duplicate model policy for {}/{}",
                policy.provider, policy.model
            ));
        }
        let threshold = policy.threshold_ratio.unwrap_or(threshold_ratio);
        validate_ratio(
            &format!("BasicCompactionConfig: modelPolicies[{index}].thresholdRatio"),
            threshold,
        )?;
        validate_retention(
            threshold,
            policy.retention.as_ref().unwrap_or(&retention),
            &format!("BasicCompactionConfig: modelPolicies[{index}]"),
        )?;
        validate_summary_pair(
            &policy.summarization_provider,
            &policy.summarization_model,
            &format!("BasicCompactionConfig: modelPolicies[{index}]"),
        )?;
    }
    if config.max_tokens == Some(0) {
        return Err("BasicCompactionConfig.maxTokens (0) must be a positive integer".into());
    }
    Ok(ResolvedConfig {
        threshold_ratio,
        retention,
        summarization_provider: config.summarization_provider.unwrap_or_default(),
        summarization_model: config.summarization_model.unwrap_or_default(),
        max_tokens: config.max_tokens.unwrap_or(8192),
        compaction_retries: config.compaction_retries.unwrap_or(1),
        max_overflow_retries: config.max_overflow_retries.unwrap_or(1),
        model_policies: config.model_policies,
        auto: config.auto.unwrap_or(true),
    })
}

pub fn resolve_target_policy(
    config: &ResolvedConfig,
    provider: &str,
    model: &str,
) -> ResolvedTargetPolicy {
    let override_ = config
        .model_policies
        .iter()
        .find(|entry| entry.provider == provider && entry.model == model);
    ResolvedTargetPolicy {
        provider: provider.into(),
        model: model.into(),
        threshold_ratio: override_
            .and_then(|value| value.threshold_ratio)
            .unwrap_or(config.threshold_ratio),
        retention: override_
            .and_then(|value| value.retention.clone())
            .unwrap_or_else(|| config.retention.clone()),
        summarization_provider: override_
            .and_then(|value| value.summarization_provider.clone())
            .unwrap_or_else(|| config.summarization_provider.clone()),
        summarization_model: override_
            .and_then(|value| value.summarization_model.clone())
            .unwrap_or_else(|| config.summarization_model.clone()),
        max_tokens: override_
            .and_then(|value| value.max_tokens)
            .unwrap_or(config.max_tokens),
        compaction_retries: override_
            .and_then(|value| value.compaction_retries)
            .unwrap_or(config.compaction_retries),
        max_overflow_retries: override_
            .and_then(|value| value.max_overflow_retries)
            .unwrap_or(config.max_overflow_retries),
    }
}

pub fn resolve_compact_spec(
    policy: &ResolvedTargetPolicy,
    context_window: u64,
) -> Result<ResolvedCompactSpec, String> {
    if context_window == 0 {
        return Err("BasicCompactionConfig: contextWindow (0) must be a positive integer".into());
    }
    let threshold_tokens = ((context_window as f64) * policy.threshold_ratio).floor() as u64;
    let retain_tokens = match policy.retention {
        RetentionConfig::Ratio(value) => ((context_window as f64) * value).floor() as u64,
        RetentionConfig::Tokens(value) => value,
    };
    if retain_tokens >= threshold_tokens {
        return Err(format!(
            "BasicCompactionConfig: {}/{} retainTokens ({retain_tokens}) must be less than threshold tokens {threshold_tokens}",
            policy.provider, policy.model
        ));
    }
    Ok(ResolvedCompactSpec {
        context_window,
        threshold_tokens,
        retain_tokens,
    })
}

pub struct BasicCompactionPlugin;

#[async_trait::async_trait]
impl Plugin for BasicCompactionPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("compaction-basic")
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["llm", "tokenMeter", "sessions"])
    }

    async fn apply(&self, ctx: &cordis::Context, config: ArcValue) -> Result<(), PluginError> {
        let value = config
            .downcast_ref::<serde_json::Value>()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let object = value.as_object().ok_or_else(|| {
            PluginError::new(arc("BasicCompactionConfig must be an object".to_string()))
        })?;
        let allowed = [
            "thresholdRatio",
            "retainRatio",
            "retainTokens",
            "summarizationProvider",
            "summarizationModel",
            "maxTokens",
            "compactionRetries",
            "maxOverflowRetries",
            "modelPolicies",
            "auto",
        ];
        if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(PluginError::new(arc(format!(
                "BasicCompactionConfig: unknown key \"{key}\""
            ))));
        }
        let max_tokens = object
            .get("maxTokens")
            .map(|value| {
                value.as_u64().ok_or_else(|| {
                    PluginError::new(arc(
                        "BasicCompactionConfig.maxTokens must be a positive integer".to_string(),
                    ))
                })
            })
            .transpose()?
            .unwrap_or(8192);
        if max_tokens == 0 {
            return Err(PluginError::new(arc(
                "BasicCompactionConfig.maxTokens (0) must be a positive integer".to_string(),
            )));
        }
        let auto = object
            .get("auto")
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    PluginError::new(arc(
                        "BasicCompactionConfig: auto must be a boolean".to_string()
                    ))
                })
            })
            .transpose()?
            .unwrap_or(true);
        let engine = BasicCompactionEngine::install(ctx, max_tokens)
            .map_err(|error| PluginError::new(arc(error)))?;
        if auto {
            let disposer = install_automatic(ctx, &engine);
            let _ = ctx.effect(
                "compaction-basic automatic listeners",
                Box::pin(async move { Some(disposer) }),
            );
        }
        Ok(())
    }
}

pub fn plugin() -> Arc<dyn Plugin> {
    Arc::new(BasicCompactionPlugin)
}

use crate::{
    CompactionAbort, CompactionAgentContext, CompactionEngine, CompactionResult, CompactionTrigger,
    ManualCompactAgentContext, ManualCompactionError, ManualCompactionErrorCode,
    compact_checkpoint_source, compaction_id, tool_pairing_balanced_after,
    tool_pairing_balanced_before,
};

const INSTRUCTION: &str = "You are acting as a compaction engine. Condense the conversation above into a structured checkpoint that preserves the user's goals, constraints, decisions, exact paths, commands, errors, completed work, pending work, and the single next action. Output only concise Markdown. Do not call tools and do not mention compaction.";
const PREAMBLE: &str = "This checkpoint condenses earlier conversation context. Treat it as established background and continue directly from the messages that follow.";

pub struct BasicCompactionEngine {
    llm: Arc<LlmRuntime>,
    sessions: Arc<SessionStore>,
    meter: Arc<TokenMeter>,
    operations: Mutex<()>,
    max_tokens: u64,
}

impl BasicCompactionEngine {
    pub fn install(ctx: &cordis::Context, max_tokens: u64) -> Result<Arc<Self>, String> {
        let llm = ctx
            .get_typed::<Arc<LlmRuntime>>("llm", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "compaction-basic requires the llm service".to_string())?;
        let sessions = ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "compaction-basic requires the sessions service".to_string())?;
        let meter = ctx
            .get_typed::<Arc<TokenMeter>>("tokenMeter", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "compaction-basic requires the tokenMeter service".to_string())?;
        let engine = Arc::new(Self {
            llm,
            sessions,
            meter,
            operations: Mutex::new(()),
            max_tokens: max_tokens.max(1),
        });
        let service: Arc<dyn CompactionEngine> = engine.clone();
        ctx.register_service(service);
        Ok(engine)
    }

    fn cancelled(signal: Option<&CompactionAbort>) -> bool {
        signal.is_some_and(|signal| signal())
    }

    fn select_range(session: &Session) -> Result<Option<(u64, u64)>, ManualCompactionError> {
        let surface = session.surface().map_err(|error| {
            ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
        })?;
        if surface.nodes.len() < 2 {
            return Ok(None);
        }
        let start = surface.nodes[0];
        let mut end_index = surface.nodes.len() - 2;
        while end_index > 0
            && !tool_pairing_balanced_after(session, surface.nodes[end_index]).map_err(|error| {
                ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
            })?
        {
            end_index -= 1;
        }
        let end = surface.nodes[end_index];
        if !tool_pairing_balanced_before(session, start)
            .map_err(|error| ManualCompactionError::new(ManualCompactionErrorCode::Commit, error))?
            || !tool_pairing_balanced_after(session, end).map_err(|error| {
                ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
            })?
        {
            return Ok(None);
        }
        Ok(Some((start, end)))
    }

    fn selected_messages(
        session: &Session,
        start: u64,
        end: u64,
    ) -> Result<Vec<Message>, ManualCompactionError> {
        let surface = session.surface().map_err(|error| {
            ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
        })?;
        let mut selected = Vec::new();
        let mut in_range = false;
        for seq in surface.nodes {
            if seq == start {
                in_range = true;
            }
            if in_range {
                let event = session.events().get(seq as usize).cloned().ok_or_else(|| {
                    ManualCompactionError::new(
                        ManualCompactionErrorCode::Changed,
                        "the selected history changed before summarization",
                    )
                })?;
                if let Some(message) = derive_event_message(&event) {
                    selected.push(message);
                }
            }
            if seq == end {
                break;
            }
        }
        Ok(selected)
    }

    fn assert_inactive(session: &Session) -> Result<(), ManualCompactionError> {
        let mut active = false;
        for event in session.events().iter() {
            match event.type_.as_str() {
                "compaction/start" => active = true,
                "compaction/end" => active = false,
                _ => {}
            }
        }
        if active {
            Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Busy,
                "compaction is already active for this session",
            ))
        } else {
            Ok(())
        }
    }

    async fn summarize(
        &self,
        agent: &CompactionAgentContext,
        mut messages: Vec<Message>,
        signal: Option<&CompactionAbort>,
    ) -> Result<
        (
            Vec<ContentBlock>,
            String,
            String,
            Option<dsh_llm::TokenUsage>,
        ),
        ManualCompactionError,
    > {
        if Self::cancelled(signal) {
            return Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Cancelled,
                "manual compaction was cancelled",
            ));
        }
        let header = fold_request_header(&agent.session.events(), None);
        let provider = header
            .as_ref()
            .map(|header| header.config.provider.clone())
            .filter(|value| !value.is_empty())
            .or_else(|| agent.provider.clone())
            .ok_or_else(|| {
                ManualCompactionError::new(
                    ManualCompactionErrorCode::Summary,
                    "no provider is available for summarization",
                )
            })?;
        let model = header
            .as_ref()
            .map(|header| header.config.model.clone())
            .filter(|value| !value.is_empty())
            .or_else(|| agent.model.clone())
            .ok_or_else(|| {
                ManualCompactionError::new(
                    ManualCompactionErrorCode::Summary,
                    "no model is available for summarization",
                )
            })?;
        messages.push(create_user_message(
            vec![ContentBlock::Text {
                text: INSTRUCTION.to_string(),
            }],
            MessageSource::Plugin {
                plugin: "dsh-compaction-basic".to_string(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        ));
        let options = GenerateOptions {
            provider: provider.clone(),
            model: model.clone(),
            reasoning_effort: None,
            messages,
            system: header.as_ref().and_then(|header| header.system.clone()),
            tools: header.as_ref().and_then(|header| header.tools.clone()),
            temperature: None,
            max_tokens: Some(self.max_tokens),
            stop: None,
            signal: signal.cloned(),
            session_id: Some(agent.session.id().to_string()),
            purpose: Some("compaction".to_string()),
            agent_loop_request: false,
        };
        let mut stream = self.llm.stream(options);
        let mut assembler = BlockAssembler::new();
        while let Some(chunk) = stream.next().await {
            assembler.push(&chunk);
        }
        match assembler.finish() {
            FinishReason::Stop | FinishReason::ToolCalls => {}
            FinishReason::MaxTokens => {
                return Err(ManualCompactionError::new(
                    ManualCompactionErrorCode::Summary,
                    "summarization was truncated at the token cap",
                ));
            }
            FinishReason::Error { failure } | FinishReason::Aborted { failure } => {
                return Err(ManualCompactionError::new(
                    ManualCompactionErrorCode::Summary,
                    failure.message,
                ));
            }
        }
        let summary: Vec<ContentBlock> = assembler
            .blocks()
            .into_iter()
            .filter(|block| matches!(block, ContentBlock::Text { .. }))
            .collect();
        if summary
            .iter()
            .all(|block| block.as_text().is_none_or(|text| text.trim().is_empty()))
        {
            return Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Summary,
                "summarization produced no text",
            ));
        }
        Ok((summary, provider, model, assembler.usage().cloned()))
    }
}

pub fn install_automatic(
    ctx: &cordis::Context,
    engine: &Arc<BasicCompactionEngine>,
) -> cordis::Disposer {
    let engine = engine.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let engine = engine.clone();
        Box::pin(async move {
            let payload = args
                .first()
                .and_then(|value| value.downcast_ref::<dsh_agent::AgentPreStepPayload>())
                .cloned()
                .expect("agent/pre-step payload");
            let next =
                cordis::downcast_arc::<cordis::NextFn>(args.last().expect("agent/pre-step next"))
                    .expect("agent/pre-step next");
            let context = CompactionAgentContext {
                session: payload.agent.session().clone(),
                provider: payload.agent.options().provider.clone(),
                model: payload.agent.options().model.clone(),
            };
            let _ = engine
                .compact_if_needed(&context, CompactionTrigger::Pressure, None)
                .await;
            Some(next.call().await)
        })
    });
    futures::executor::block_on(ctx.on("agent/pre-step", listener, Default::default()))
}

include!("basic_impl.rs");
