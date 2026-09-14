#[async_trait::async_trait]
impl CompactionEngine for BasicCompactionEngine {
    async fn compact_if_needed(
        &self,
        agent: &CompactionAgentContext,
        trigger: CompactionTrigger,
        signal: Option<&CompactionAbort>,
    ) -> Result<Option<CompactionResult>, ManualCompactionError> {
        self.compact_pressure(agent, trigger, signal, 0).await
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
        let Some((start, end)) =
            self.select_range(&agent.session, 0, self.resolved()?.protect_recent_messages)?
        else {
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
        self.compact_region_inner(start, end, agent, signal, None, false)
            .await
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
        let lock = self.operations.lock();
        tokio::pin!(lock);
        let _operation = loop {
            tokio::select! {
                guard = &mut lock => break guard,
                _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => if Self::cancelled(signal) {
                    return Err(ManualCompactionError::new(ManualCompactionErrorCode::Cancelled, "compaction cancelled"));
                }
            }
        };
        Self::assert_inactive(&agent.session)?;
        let initial_surface = agent.session.surface().map_err(|error| {
            ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
        })?;
        // Replacement nodes keep their surface position but have newer log
        // sequences. Ranges must follow that position, never numeric seq order.
        let start_index = initial_surface.nodes.iter().position(|seq| *seq == start);
        let end_index = initial_surface.nodes.iter().position(|seq| *seq == end);
        let Some((start_index, end_index)) = start_index
            .zip(end_index)
            .filter(|(start, end)| start <= end)
        else {
            return Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Changed,
                "the requested compaction range is not a contiguous surface span",
            ));
        };
        let shadowed_seqs = initial_surface.nodes[start_index..=end_index].to_vec();
        if start_index == 0
            && agent.session.with_events(|events| {
                initial_surface
                    .nodes
                    .first()
                    .and_then(|seq| events.get(*seq as usize))
                    .is_some_and(|event| event.type_ == "system/message")
            })
        {
            return Err(ManualCompactionError::new(
                ManualCompactionErrorCode::Commit,
                "the protected system head cannot be included in a compaction range",
            ));
        }
        let open_turn =
            agent
                .session
                .events()
                .iter()
                .fold(None, |open, event| match event.type_.as_str() {
                    "turn/start" => event.data.get("turn").and_then(|value| value.as_u64()),
                    "turn/end" => None,
                    _ => open,
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
        let messages = Self::selected_messages(&agent.session, start, end)?;
        let start_event = agent
            .session
            .append("compaction/start", lifecycle.clone(), None)
            .map_err(|error| {
                ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
            })?;
        let _lifecycle_guard = CompactionLifecycleGuard {
            session: agent.session.clone(),
            lifecycle: lifecycle.clone(),
        };
        let (provider, model) = self.target(agent)?;
        let policy = resolve_target_policy(&self.resolved()?, &provider, &model);
        let mut attempts = 0;
        let summarized = loop {
            let result = self.summarize(agent, messages.clone(), signal).await;
            if !matches!(&result, Err(error) if error.code == ManualCompactionErrorCode::Summary)
                || attempts >= policy.compaction_retries
                || Self::cancelled(signal)
            {
                break result;
            }
            attempts += 1;
            agent
                .session
                .append(
                    "compaction/retry",
                    serde_json::json!({"compactionId": compaction.as_str(), "attempt": attempts}),
                    None,
                )
                .map_err(|e| ManualCompactionError::new(ManualCompactionErrorCode::Commit, e))?;
        };
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
                    "maxTokens": policy.max_tokens,
                    "usage": usage,
                }),
                None,
            )
            .map_err(|error| {
                ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
            })?;
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
                        std::iter::once(start_event.seq.get())
                            .chain(std::iter::once(summary_event.seq.get()))
                            .chain(shadowed_seqs.iter().copied())
                            .collect(),
                    ),
                }),
            )
            .map_err(|error| {
                ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
            })?;
        let end_event = agent
            .session
            .append("compaction/end", lifecycle, None)
            .map_err(|error| {
                ManualCompactionError::new(ManualCompactionErrorCode::Commit, error)
            })?;
        self.sessions.flush(&agent.session).await.map_err(|error| {
            ManualCompactionError::new(
                ManualCompactionErrorCode::Persistence,
                format!("manual compaction durability checkpoint failed: {error}"),
            )
        })?;
        Ok(CompactionResult {
            compaction_id: compaction,
            source_command_id: source_command_id.cloned(),
            start_seq: start_event.seq.get(),
            summary_seq: summary_event.seq.get(),
            end_seq: end_event.seq.get(),
            summary,
            shadowed_range: (start, end),
            shadowed_seqs,
            shadowed_token_count,
        })
    }
}
