use std::sync::Arc;

use cordis::{ArcValue, InjectSpec, Plugin, PluginError, arc};
use dsh_commands::CommandId;
use dsh_llm::{
    BlockAssembler, ContentBlock, FinishReason, GenerateOptions, LlmRuntime, Message,
    MessageSource, create_user_message,
};
use dsh_session::{Session, SessionStore, SurfaceIntent, SurfaceOp, fold_request_header};
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
    pub protect_recent_messages: Option<usize>,
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
    pub protect_recent_messages: usize,
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
    if config.protect_recent_messages == Some(0)
        || config.compaction_retries.unwrap_or(1) > 10
        || config.max_overflow_retries.unwrap_or(1) > 10
        || config.model_policies.iter().any(|p| {
            p.max_tokens == Some(0)
                || p.compaction_retries.unwrap_or(1) > 10
                || p.max_overflow_retries.unwrap_or(1) > 10
        })
    {
        return Err(
            "Compaction requires at least one protected message and at most 10 retries".into(),
        );
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
        protect_recent_messages: config.protect_recent_messages.unwrap_or(1),
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
        let config = parse_config(&value).map_err(|e| PluginError::new(arc(e)))?;
        let engine = BasicCompactionEngine::install_config(ctx, config)
            .map_err(|error| PluginError::new(arc(error)))?;
        let disposer = install_automatic(ctx, &engine);
        let _ = ctx.effect(
            "compaction-basic automatic listeners",
            Box::pin(async move { Some(disposer) }),
        );
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
    config: BasicCompactionConfig,
    ctx: cordis::Context,
}

struct CompactionLifecycleGuard {
    session: Session,
    lifecycle: serde_json::Value,
}
impl Drop for CompactionLifecycleGuard {
    fn drop(&mut self) {
        let id = &self.lifecycle["compactionId"];
        let ended = self.session.with_events(|events| {
            events
                .iter()
                .rev()
                .any(|e| e.type_ == "compaction/end" && &e.data["compactionId"] == id)
        });
        if !ended {
            self.lifecycle["error"] =
                serde_json::json!("Compaction interrupted before the checkpoint completed");
            let _ = self
                .session
                .append("compaction/end", self.lifecycle.clone(), None);
        }
    }
}

impl BasicCompactionEngine {
    pub fn install(ctx: &cordis::Context, max_tokens: u64) -> Result<Arc<Self>, String> {
        Self::install_config(
            ctx,
            BasicCompactionConfig {
                max_tokens: Some(max_tokens),
                ..Default::default()
            },
        )
    }

    pub fn install_config(
        ctx: &cordis::Context,
        config: BasicCompactionConfig,
    ) -> Result<Arc<Self>, String> {
        resolve_config(config.clone())?;
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
            config,
            ctx: ctx.clone(),
        });
        let service: Arc<dyn CompactionEngine> = engine.clone();
        ctx.register_service(service);
        Ok(engine)
    }

    fn cancelled(signal: Option<&CompactionAbort>) -> bool {
        signal.is_some_and(|signal| signal())
    }

    fn select_range(
        &self,
        session: &Session,
        retain_tokens: u64,
        protect: usize,
    ) -> Result<Option<(u64, u64)>, ManualCompactionError> {
        let surface = session.surface().map_err(|error| {
            ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
        })?;
        let events = session.events();
        // Only the protected head is outside history. Later system updates,
        // including dormant empty nodes, must not block bounded compaction.
        let start_index = usize::from(
            surface
                .nodes
                .first()
                .and_then(|seq| events.get(*seq as usize))
                .is_some_and(|event| event.type_ == "system/message"),
        );
        let events = session.events();
        let mut retained = 0u64;
        let mut count = 0usize;
        let mut keep_from = surface.nodes.len();
        for (index, seq) in surface.nodes.iter().enumerate().rev() {
            if index < start_index {
                break;
            }
            if let Some(message) = events
                .get(*seq as usize)
                .and_then(|event| session.derive_event_message(event))
            {
                if count >= protect && retained >= retain_tokens {
                    break;
                }
                retained = retained.saturating_add(self.meter.estimate_message(&message));
                count += 1;
            }
            keep_from = index;
        }
        let Some(mut end_index) = keep_from.checked_sub(1).filter(|i| *i >= start_index) else {
            return Ok(None);
        };
        let start = surface.nodes[start_index];
        while end_index > start_index
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
        let events = session.events();
        if let Some(head) = surface
            .nodes
            .first()
            .and_then(|seq| events.get(*seq as usize))
            .filter(|event| event.type_ == "system/message")
        {
            if let Some(message) = session.derive_event_message(head) {
                selected.push(message);
            }
        }
        let mut in_range = false;
        for seq in surface.nodes {
            if seq == start {
                in_range = true;
            }
            if in_range {
                let event = events.get(seq as usize).cloned().ok_or_else(|| {
                    ManualCompactionError::new(
                        ManualCompactionErrorCode::Changed,
                        "the selected history changed before summarization",
                    )
                })?;
                if let Some(message) = session.derive_event_message(&event) {
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
        let policy = resolve_target_policy(&self.resolved()?, &provider, &model);
        let provider = if policy.summarization_provider.is_empty() {
            provider
        } else {
            policy.summarization_provider.clone()
        };
        let model = if policy.summarization_model.is_empty() {
            model
        } else {
            policy.summarization_model.clone()
        };
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
        let surface = agent.session.surface().map_err(|error| {
            ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
        })?;
        let has_system_history = agent.session.with_events(|events| {
            surface.nodes.iter().any(|seq| {
                events
                    .get(*seq as usize)
                    .is_some_and(|event| event.type_ == "system/message")
            })
        });
        let legacy_system = if has_system_history {
            None
        } else {
            header.as_ref().and_then(|header| header.system.clone())
        };
        let mut options = GenerateOptions {
            provider: provider.clone(),
            model: model.clone(),
            reasoning_effort: None,
            messages,
            system: legacy_system,
            tools: header.as_ref().and_then(|header| header.tools.clone()),
            temperature: None,
            max_tokens: Some(policy.max_tokens),
            stop: None,
            signal: signal.cloned(),
            session_id: Some(agent.session.id().to_string()),
            purpose: Some("compaction".to_string()),
            agent_loop_request: false,
            telemetry: None,
        };
        let assembler = loop {
            let mut stream = self.llm.stream(options.clone());
            let mut assembler = BlockAssembler::new();
            loop {
                tokio::select! {
                    chunk = stream.next() => match chunk { Some(chunk) => assembler.push(&chunk), None => break },
                    _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => if Self::cancelled(signal) {
                        return Err(ManualCompactionError::new(ManualCompactionErrorCode::Cancelled, "compaction cancelled"));
                    }
                }
            }
            if let FinishReason::Error { failure } = assembler.finish() {
                if failure.code == "IMAGE_OFFLOAD_REQUIRED" && failure.offload_images.is_some() {
                    let ids: std::collections::HashSet<_> = options
                        .messages
                        .iter()
                        .map(|message| message.id.clone())
                        .collect();
                    let events = agent.session.events();
                    let selected: Vec<_> = agent
                        .session
                        .surface()
                        .map_err(|e| {
                            ManualCompactionError::new(ManualCompactionErrorCode::Summary, e)
                        })?
                        .nodes
                        .into_iter()
                        .filter(|seq| {
                            events
                                .get(*seq as usize)
                                .and_then(|event| agent.session.derive_event_message(event))
                                .is_some_and(|message| ids.contains(&message.id))
                        })
                        .collect();
                    if agent
                        .session
                        .offload_images_in_nodes(
                            failure.offload_images.unwrap_or(0),
                            Some(&selected),
                        )
                        .map_err(|e| {
                            ManualCompactionError::new(ManualCompactionErrorCode::Summary, e)
                        })?
                    {
                        let projected = agent.session.derive_messages().map_err(|e| {
                            ManualCompactionError::new(ManualCompactionErrorCode::Summary, e)
                        })?;
                        for message in &mut options.messages {
                            if let Some(next) = projected.iter().find(|next| next.id == message.id)
                            {
                                *message = next.clone();
                            }
                        }
                        continue;
                    }
                }
            }
            break assembler;
        };
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
    let pressure_engine = engine.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let engine = pressure_engine.clone();
        Box::pin(async move {
            let payload = args
                .first()
                .and_then(|v| v.downcast_ref::<dsh_agent::AgentPreStepPayload>())
                .cloned()
                .expect("pre-step payload");
            let next = cordis::downcast_arc::<cordis::NextFn>(args.last().unwrap()).unwrap();
            let decision = next.call().await;
            let Some(dsh_agent::PreStepDecision::Enter { messages, .. }) =
                decision.downcast_ref::<dsh_agent::PreStepDecision>()
            else {
                return Some(decision);
            };
            let extra = messages
                .iter()
                .map(|m| dsh_token_meter::estimate_content(&m.content))
                .fold(0u64, u64::saturating_add);
            let agent = CompactionAgentContext {
                session: payload.agent.session().clone(),
                provider: payload.agent.options().provider.clone(),
                model: payload.agent.options().model.clone(),
            };
            let signal = payload.signal.clone();
            let abort: CompactionAbort = Arc::new(move || signal.aborted());
            if let Err(error) = engine
                .compact_pressure(&agent, CompactionTrigger::Pressure, Some(&abort), extra)
                .await
            {
                if !payload.signal.aborted() {
                    let _ = agent.session.append("compaction/error", serde_json::json!({"turn":payload.turn,"step":payload.step,"code":error.code.as_str(),"message":error.message}), None);
                    payload
                        .signal
                        .abort_with(dsh_agent::AgentCancelCause::Hook {
                            reason: format!("Context compaction failed: {}", error.message),
                        });
                }
                return Some(arc(dsh_agent::PreStepDecision::Reject));
            }
            Some(decision)
        })
    });
    let pressure =
        futures::executor::block_on(ctx.on("agent/pre-step", listener, Default::default()));
    let engine = engine.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let engine = engine.clone();
        Box::pin(async move {
            let payload = args
                .first()
                .and_then(|v| v.downcast_ref::<dsh_agent::AgentRequestErrorPayload>())
                .cloned()
                .expect("request-error payload");
            let next = cordis::downcast_arc::<cordis::NextFn>(args.last().unwrap()).unwrap();
            if payload.failure.code != dsh_llm::CONTEXT_WINDOW_EXCEEDED_CODE {
                return Some(next.call().await);
            }
            let agent = CompactionAgentContext {
                session: payload.agent.session().clone(),
                provider: payload.agent.options().provider.clone(),
                model: payload.agent.options().model.clone(),
            };
            let attempt = agent
                .session
                .events()
                .iter()
                .filter(|e| {
                    e.type_ == "compaction/recovery"
                        && e.data["turn"].as_u64() == Some(payload.turn)
                        && e.data["step"].as_u64() == Some(payload.step)
                })
                .count() as u64;
            let signal = payload.signal.clone();
            let abort: CompactionAbort = Arc::new(move || signal.aborted());
            let recover = async {
                let config = engine.resolved()?;
                let (provider, model) = engine.target(&agent)?;
                let policy = resolve_target_policy(&config, &provider, &model);
                if !config.auto || attempt >= policy.max_overflow_retries || abort() { return Ok(false); }
                agent.session.append("compaction/recovery", serde_json::json!({"turn":payload.turn,"step":payload.step,"attempt":attempt+1}), None).map_err(|e| ManualCompactionError::new(ManualCompactionErrorCode::Commit,e))?;
                let before = agent.session.surface().map_err(|e| ManualCompactionError::new(ManualCompactionErrorCode::Commit,e))?.nodes;
                engine.compact_pressure(&agent, CompactionTrigger::ContextOverflow, Some(&abort), 0).await?;
                let after = agent.session.surface().map_err(|e| ManualCompactionError::new(ManualCompactionErrorCode::Commit,e))?.nodes;
                Ok::<bool,ManualCompactionError>(!abort() && before != after)
            }.await;
            let recovered = match recover {
                Ok(value) => value,
                Err(error) => {
                    let _ = agent.session.append("compaction/error",serde_json::json!({"turn":payload.turn,"step":payload.step,"code":error.code.as_str(),"message":error.message}),None);
                    false
                }
            };
            Some(arc(if recovered {
                Some(dsh_agent::RequestErrorAction::Retry)
            } else {
                None
            }))
        })
    });
    let overflow = futures::executor::block_on(ctx.on(
        "agent/request-error",
        listener,
        cordis::EventOptions::default().prepend(true),
    ));
    cordis::make_disposer(move || {
        let pressure = pressure.clone();
        let overflow = overflow.clone();
        Box::pin(async move {
            pressure().await;
            overflow().await;
        })
    })
}

include!("policy.rs");
include!("basic_impl.rs");
