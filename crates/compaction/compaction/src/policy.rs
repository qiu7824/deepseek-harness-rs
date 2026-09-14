#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyWire {
    provider: Option<String>,
    model: Option<String>,
    threshold_ratio: Option<f64>,
    retain_ratio: Option<f64>,
    retain_tokens: Option<u64>,
    summarization_provider: Option<String>,
    summarization_model: Option<String>,
    max_tokens: Option<u64>,
    compaction_retries: Option<u64>,
    max_overflow_retries: Option<u64>,
    auto: Option<bool>,
    protect_recent_messages: Option<usize>,
    #[serde(default)]
    model_policies: Vec<serde_json::Value>,
}

pub fn parse_config(value: &serde_json::Value) -> Result<BasicCompactionConfig, String> {
    parse_policy_config(value, false)
}
fn parse_policy_config(
    value: &serde_json::Value,
    per_model: bool,
) -> Result<BasicCompactionConfig, String> {
    let wire: PolicyWire = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    if !per_model && (wire.provider.is_some() || wire.model.is_some()) {
        return Err("provider/model must be inside modelPolicies; use summarizationProvider/summarizationModel to select the summary route".into());
    }
    if per_model
        && (wire.auto.is_some()
            || wire.protect_recent_messages.is_some()
            || !wire.model_policies.is_empty())
    {
        return Err(
            "auto, protectRecentMessages and nested modelPolicies are not per-model options".into(),
        );
    }
    if wire.retain_ratio.is_some() && wire.retain_tokens.is_some() {
        return Err("retainRatio and retainTokens cannot both be specified".into());
    }
    let retention = wire
        .retain_tokens
        .map(RetentionConfig::Tokens)
        .or_else(|| wire.retain_ratio.map(RetentionConfig::Ratio));
    let mut policies = Vec::new();
    for entry in wire.model_policies {
        let item = parse_policy_config(&entry, true)?;
        if !item.model_policies.is_empty() {
            return Err("nested modelPolicies are unsupported".into());
        }
        policies.push(ModelCompactPolicyConfig {
            provider: entry
                .get("provider")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .into(),
            model: entry
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .into(),
            threshold_ratio: item.threshold_ratio,
            retention: item.retention,
            summarization_provider: item.summarization_provider,
            summarization_model: item.summarization_model,
            max_tokens: item.max_tokens,
            compaction_retries: item.compaction_retries,
            max_overflow_retries: item.max_overflow_retries,
        });
    }
    let _ = (wire.provider, wire.model);
    let config = BasicCompactionConfig {
        threshold_ratio: wire.threshold_ratio,
        retention,
        summarization_provider: wire.summarization_provider,
        summarization_model: wire.summarization_model,
        max_tokens: wire.max_tokens,
        compaction_retries: wire.compaction_retries,
        max_overflow_retries: wire.max_overflow_retries,
        model_policies: policies,
        auto: wire.auto,
        protect_recent_messages: wire.protect_recent_messages,
    };
    // Per-model retention is validated against the inherited threshold by resolve_config.
    Ok(config)
}

impl BasicCompactionEngine {
    fn resolved(&self) -> Result<ResolvedConfig, ManualCompactionError> {
        let mut config = self.config.clone();
        if let Some(settings) = self
            .ctx
            .get_typed::<Arc<dsh_settings::SettingsProvider>>("settings", false)
        {
            let namespace = dsh_settings::settings_namespace("memory").unwrap();
            if let Some(value) = settings.get(&namespace).and_then(|v| v.to_json()) {
                config.auto = config.auto.or_else(|| {
                    value
                        .get("autoCompact")
                        .and_then(serde_json::Value::as_bool)
                });
                config.threshold_ratio = config.threshold_ratio.or_else(|| {
                    value
                        .get("compactThreshold")
                        .and_then(serde_json::Value::as_f64)
                });
                config.retention = config.retention.or_else(|| {
                    value
                        .get("compactTarget")
                        .and_then(serde_json::Value::as_f64)
                        .map(RetentionConfig::Ratio)
                });
                if config.protect_recent_messages.is_none() {
                    if let Some(raw) = value.get("protectRecentMessages") {
                        let n = raw
                            .as_f64()
                            .filter(|n| {
                                n.is_finite() && *n >= 1.0 && n.fract() == 0.0 && *n <= 200.0
                            })
                            .ok_or_else(|| {
                                ManualCompactionError::new(
                                    ManualCompactionErrorCode::Summary,
                                    "protectRecentMessages must be an integer between 1 and 200",
                                )
                            })?;
                        config.protect_recent_messages = Some(n as usize);
                    }
                }
            }
        }
        resolve_config(config)
            .map_err(|e| ManualCompactionError::new(ManualCompactionErrorCode::Summary, e))
    }

    fn target(
        &self,
        agent: &CompactionAgentContext,
    ) -> Result<(String, String), ManualCompactionError> {
        let header = fold_request_header(&agent.session.events(), None);
        let provider = header
            .as_ref()
            .map(|h| h.config.provider.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| agent.provider.clone());
        let model = header
            .as_ref()
            .map(|h| h.config.model.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| agent.model.clone());
        provider.zip(model).ok_or_else(|| {
            ManualCompactionError::new(
                ManualCompactionErrorCode::Summary,
                "No model selected for compaction",
            )
        })
    }

    async fn compact_pressure(
        &self,
        agent: &CompactionAgentContext,
        trigger: CompactionTrigger,
        signal: Option<&CompactionAbort>,
        extra_tokens: u64,
    ) -> Result<Option<CompactionResult>, ManualCompactionError> {
        let config = self.resolved()?;
        if !config.auto {
            return Ok(None);
        }
        let (provider, model) = self.target(agent)?;
        let policy = resolve_target_policy(&config, &provider, &model);
        let info = self
            .llm
            .resolve_model_info(&provider, &model, signal)
            .await
            .map_err(|e| {
                ManualCompactionError::new(ManualCompactionErrorCode::Summary, e.to_string())
            })?;
        let spec = info
            .context
            .map(|c| resolve_compact_spec(&policy, c.context_window))
            .transpose()
            .map_err(|e| ManualCompactionError::new(ManualCompactionErrorCode::Summary, e))?;
        let measure = || {
            self.meter
                .measure(&agent.session, None)
                .total_tokens
                .saturating_add(extra_tokens)
        };
        let threshold = spec
            .as_ref()
            .map(|s| s.threshold_tokens)
            .unwrap_or(u64::MAX);
        if trigger == CompactionTrigger::Pressure && measure() < threshold {
            return Ok(None);
        }
        if Self::cancelled(signal) {
            return Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Cancelled,
                "compaction cancelled",
            ));
        }
        if let Some(pruner) = self
            .ctx
            .get_typed::<Arc<dsh_compaction_tool_result_pruner::ToolResultPruner>>(
                "toolResultPruner",
                false,
            )
        {
            let pruned = pruner
                .prune_session_protected(&agent.session, config.protect_recent_messages)
                .map_err(|e| ManualCompactionError::new(ManualCompactionErrorCode::Commit, e))?;
            if !pruned.pruned.is_empty() {
                self.sessions.flush(&agent.session).await.map_err(|e| {
                    ManualCompactionError::new(ManualCompactionErrorCode::Persistence, e)
                })?;
                if trigger == CompactionTrigger::Pressure && measure() < threshold {
                    return Ok(None);
                }
            }
        }
        let retain = spec.map(|s| s.retain_tokens).unwrap_or(0);
        let Some((start, end)) =
            self.select_range(&agent.session, retain, config.protect_recent_messages)?
        else {
            return Ok(None);
        };
        self.compact_region_inner(start, end, agent, signal, None, false)
            .await
            .map(Some)
    }
}
