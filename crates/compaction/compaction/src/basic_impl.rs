#[async_trait::async_trait]
impl CompactionEngine for BasicCompactionEngine {
    async fn compact_if_needed(
        &self,
        agent: &CompactionAgentContext,
        trigger: CompactionTrigger,
        signal: Option<&CompactionAbort>,
    ) -> Result<Option<CompactionResult>, ManualCompactionError> {
        let header = fold_request_header(&agent.session.events(), None);
        let provider = header.as_ref().map(|header| header.config.provider.clone())
            .filter(|value| !value.is_empty()).or_else(|| agent.provider.clone());
        let model = header.as_ref().map(|header| header.config.model.clone())
            .filter(|value| !value.is_empty()).or_else(|| agent.model.clone());
        let (Some(provider), Some(model)) = (provider, model) else { return Ok(None); };
        let measurement = self.meter.measure(&agent.session, header);
        if trigger == CompactionTrigger::Pressure {
            let context = self.llm.resolve_model_info(&provider, &model, signal)
                .await.map_err(|error| ManualCompactionError::new(
                    ManualCompactionErrorCode::Summary, error.to_string()))?
                .context.ok_or_else(|| ManualCompactionError::new(
                    ManualCompactionErrorCode::Summary,
                    format!("no context capacity is declared for {provider}/{model}")))?;
            if measurement.total_tokens < context.context_window.saturating_mul(4) / 5 {
                return Ok(None);
            }
        }
        let Some((start, end)) = Self::select_range(&agent.session)? else { return Ok(None); };
        self.compact_region_inner(start, end, agent, signal, None, false).await.map(Some)
    }

    async fn compact_now(
        &self,
        agent: &ManualCompactAgentContext,
        signal: Option<&CompactionAbort>,
        source_command_id: Option<&CommandId>,
    ) -> Result<Option<CompactionResult>, ManualCompactionError> {
        let agent = CompactionAgentContext {
            session: agent.session.clone(),
            provider: agent.provider.clone(),
            model: agent.model.clone(),
        };
        let Some((start, end)) = Self::select_range(&agent.session)? else {
            return Ok(None);
        };
        self.compact_region_inner(start, end, &agent, signal, source_command_id, true)
            .await
            .map(Some)
    }

    async fn compact_region(
        &self,
        start: u64,
        end: u64,
        agent: &CompactionAgentContext,
        signal: Option<&CompactionAbort>,
    ) -> Result<CompactionResult, ManualCompactionError> {
        self.compact_region_inner(start, end, agent, signal, None, false).await
    }
}

impl BasicCompactionEngine {
    async fn compact_region_inner(
        &self,
        start: u64,
        end: u64,
        agent: &CompactionAgentContext,
        signal: Option<&CompactionAbort>,
        source_command_id: Option<&CommandId>,
        manual: bool,
    ) -> Result<CompactionResult, ManualCompactionError> {
        let _operation = self.operations.lock().await;
        Self::assert_inactive(&agent.session)?;
        let initial_surface = agent.session.surface().map_err(|error| {
            ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
        })?;
        let shadowed_seqs: Vec<u64> = initial_surface
            .nodes
            .iter()
            .copied()
            .skip_while(|seq| *seq != start)
            .take_while(|seq| *seq <= end)
            .collect();
        if shadowed_seqs.first() != Some(&start) || shadowed_seqs.last() != Some(&end) {
            return Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Changed,
                "the requested compaction range is not a contiguous surface span",
            ));
        }
        let open_turn = agent.session.events().iter().fold(None, |open, event| {
            match event.type_.as_str() {
                "turn/start" => event.data.get("turn").and_then(|value| value.as_u64()),
                "turn/end" => None,
                _ => open,
            }
        });
        if manual && open_turn.is_some() {
            return Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Busy,
                "manual compaction requires an idle session",
            ));
        }
        let compaction = compaction_id(uuid::Uuid::new_v4().to_string());
        let lifecycle = serde_json::json!({
            "compactionId": compaction.as_str(),
            "sourceCommandId": source_command_id.map(|id| id.as_str()),
            "turn": if manual { None } else { open_turn },
        });
        let start_event = agent
            .session
            .append("compaction/start", lifecycle.clone(), None)
            .map_err(|error| ManualCompactionError::new(ManualCompactionErrorCode::Commit, error))?;
        let messages = Self::selected_messages(&agent.session, start, end)?;
        let summarized = self.summarize(agent, messages, signal).await;
        let (summary, provider, model, usage) = match summarized {
            Ok(value) => value,
            Err(error) => {
                let mut failed = lifecycle;
                failed["error"] = serde_json::Value::String(error.message.clone());
                let _ = agent.session.append("compaction/end", failed, None);
                return Err(error);
            }
        };
        if Self::cancelled(signal) {
            let mut failed = lifecycle;
            failed["error"] = serde_json::Value::String("cancelled".to_string());
            let _ = agent.session.append("compaction/end", failed, None);
            return Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Cancelled,
                "manual compaction was cancelled",
            ));
        }
        let current_surface = agent.session.surface().map_err(|error| {
            ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
        })?;
        if current_surface.nodes != initial_surface.nodes {
            let mut failed = lifecycle;
            failed["error"] = serde_json::Value::String("surface changed".to_string());
            let _ = agent.session.append("compaction/end", failed, None);
            return Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Changed,
                "the compacted history changed during summarization",
            ));
        }
        let shadowed_token_count = shadowed_seqs.len() as u64;
        let summary_event = agent
            .session
            .append(
                "compaction/summary",
                serde_json::json!({
                    "compactionId": compaction.as_str(),
                    "sourceCommandId": source_command_id.map(|id| id.as_str()),
                    "summary": summary,
                    "shadowedRange": { "start": start, "end": end },
                    "shadowedSeqs": shadowed_seqs,
                    "shadowedTokenCount": shadowed_token_count,
                    "provider": provider,
                    "model": model,
                    "maxTokens": self.max_tokens,
                    "usage": usage,
                }),
                None,
            )
            .map_err(|error| ManualCompactionError::new(ManualCompactionErrorCode::Commit, error))?;
        let mut checkpoint = vec![ContentBlock::Text {
            text: format!("{PREAMBLE}\n\n<compacted-summary>"),
        }];
        checkpoint.extend(summary.clone());
        checkpoint.push(ContentBlock::Text {
            text: "</compacted-summary>".to_string(),
        });
        let message = create_user_message(
            checkpoint,
            compact_checkpoint_source(&compaction, source_command_id),
        );
        agent
            .session
            .append(
                "user/message",
                serde_json::to_value(&message).expect("checkpoint message"),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Replace { start, end },
                    source_event_seqs: Some(
                        std::iter::once(start_event.seq)
                            .chain(std::iter::once(summary_event.seq))
                            .chain(shadowed_seqs.iter().copied())
                            .collect(),
                    ),
                }),
            )
            .map_err(|error| ManualCompactionError::new(ManualCompactionErrorCode::Commit, error))?;
        let end_event = agent
            .session
            .append("compaction/end", lifecycle, None)
            .map_err(|error| ManualCompactionError::new(ManualCompactionErrorCode::Commit, error))?;
        self.sessions.flush(&agent.session).await.map_err(|error| {
            ManualCompactionError::new(
                ManualCompactionErrorCode::Persistence,
                format!("manual compaction durability checkpoint failed: {error}"),
            )
        })?;
        Ok(CompactionResult {
            compaction_id: compaction,
            source_command_id: source_command_id.cloned(),
            start_seq: start_event.seq,
            summary_seq: summary_event.seq,
            end_seq: end_event.seq,
            summary,
            shadowed_range: (start, end),
            shadowed_seqs,
            shadowed_token_count,
        })
    }
}
