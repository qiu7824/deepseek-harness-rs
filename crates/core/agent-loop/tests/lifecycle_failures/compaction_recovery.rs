use super::support::{harness, message, register_adapter};
use dsh_agent::Agent;
use dsh_compaction::basic::{
    BasicCompactionConfig, BasicCompactionEngine, RetentionConfig, install_automatic,
};
use dsh_llm::{ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, StreamChunk};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
struct Adapter {
    overflow: AtomicUsize,
    summaries: AtomicUsize,
    requests: AtomicUsize,
    block: AtomicBool,
    calls: parking_lot::Mutex<Vec<GenerateOptions>>,
    capacity: AtomicUsize,
    tool_first: AtomicBool,
}
#[async_trait::async_trait]
impl LlmAdapter for Adapter {
    async fn resolve_model(
        &self,
        provider: &str,
        model: &str,
        _: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> dsh_llm::LlmResolvedModelInfo {
        let capacity = self.capacity.load(Ordering::SeqCst) as u64;
        dsh_llm::LlmResolvedModelInfo {
            provider: provider.into(),
            id: model.into(),
            name: model.into(),
            system_prompt_update: None,
            execution_modes: vec![],
            description: None,
            input_modalities: None,
            context: (capacity > 0).then_some(dsh_llm::LlmModelContext {
                context_window: capacity,
                estimated: false,
            }),
            default_max_tokens: None,
            reasoning: None,
        }
    }
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        self.calls.lock().push(options.clone());
        if options.purpose.as_deref() == Some("compaction") {
            self.summaries.fetch_add(1, Ordering::SeqCst);
            if self.block.load(Ordering::SeqCst) {
                return Box::pin(futures::stream::pending());
            }
        } else {
            self.requests.fetch_add(1, Ordering::SeqCst);
            if self.tool_first.swap(false, Ordering::SeqCst) {
                return Box::pin(futures::stream::iter([
                    StreamChunk::BlockStart {
                        index: 0,
                        block_type: "tool_call".into(),
                    },
                    StreamChunk::BlockEnd {
                        index: 0,
                        block: ContentBlock::ToolCall {
                            id: dsh_llm::call_id("large-result"),
                            name: "quick".into(),
                            arguments: "{}".into(),
                        },
                    },
                    StreamChunk::Finish {
                        reason: FinishReason::ToolCalls,
                        replay_state: None,
                    },
                ]));
            }
            if self
                .overflow
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Box::pin(futures::stream::iter([StreamChunk::Finish {
                    reason: FinishReason::Error {
                        failure: dsh_llm::LlmFailure {
                            offload_images: None,
                            code: dsh_llm::CONTEXT_WINDOW_EXCEEDED_CODE.into(),
                            message: "fixture overflow".into(),
                            status: Some(400),
                            provider_retry_after_ms: None,
                            request_id: None,
                        },
                    },
                    replay_state: None,
                }]));
            }
        }
        Box::pin(futures::stream::iter([
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::Text {
                    text: "Saved the goal and pending work.".into(),
                },
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}
async fn setup() -> (super::support::Harness, Arc<Adapter>, cordis::Disposer) {
    let h = harness().await;
    let adapter = Arc::new(Adapter::default());
    register_adapter(&h, adapter.clone());
    dsh_token_meter::TokenMeter::install(&h.ctx, Default::default());
    for i in 0..3 {
        h.agent
            .followup(message(&format!("Remember goal {i} and its details.")));
        tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
            .await
            .unwrap();
    }
    let engine = BasicCompactionEngine::install_config(
        &h.ctx,
        BasicCompactionConfig {
            max_tokens: Some(128),
            retention: Some(RetentionConfig::Tokens(16)),
            max_overflow_retries: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    let dispose = install_automatic(&h.ctx, &engine);
    (h, adapter, dispose)
}
#[tokio::test]
async fn real_overflow_changes_request_surface_then_retries_once() {
    let (h, adapter, dispose) = setup().await;
    adapter.overflow.store(1, Ordering::SeqCst);
    let before = adapter.requests.load(Ordering::SeqCst);
    h.agent.followup(message("Continue after overflow"));
    tokio::time::timeout(Duration::from_secs(3), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.requests.load(Ordering::SeqCst) - before, 2);
    assert_eq!(adapter.summaries.load(Ordering::SeqCst), 1);
    let events = h.agent.session().events();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.type_ == "compaction/recovery")
            .count(),
        1
    );
    assert!(events.iter().any(|e| e.type_ == "compaction/summary"));
    let calls = adapter.calls.lock();
    let model: Vec<_> = calls.iter().filter(|c| c.agent_loop_request).collect();
    assert!(model[model.len() - 1].messages.len() < model[model.len() - 2].messages.len());
    drop(calls);
    dispose().await;
}

#[tokio::test]
async fn compaction_reprices_replaced_history_before_the_next_model_request() {
    use dsh_compaction::{CompactionAgentContext, CompactionEngine};
    use dsh_session::{SurfaceIntent, SurfaceOp};
    use serde_json::{Value, json};

    let (h, _, dispose) = setup().await;
    let session = h.agent.session();
    for n in 0..4 {
        session
            .append(
                "user/message",
                serde_json::to_value(message(&format!(
                    "Retain task constraint {n}: {}",
                    "history text ".repeat(512)
                )))
                .unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
    }
    session
        .append("request/context", json!({"contextWindow":100_000}), None)
        .unwrap();
    session.append("assistant/chunk", json!({"turn":3,"step":1,"chunk":{"type":"usage","usage":{"inputTokens":50_000,"outputTokens":0}}}), None).unwrap();
    let engine = h
        .ctx
        .get_typed::<Arc<dyn CompactionEngine>>("compaction", false)
        .unwrap();
    let agent = CompactionAgentContext {
        session: session.clone(),
        provider: Some("test".into()),
        model: Some("test".into()),
    };
    let definition = dsh_token_meter::context_pressure_projection_definition();
    let project = |events: &[dsh_session::SessionEvent]| {
        let mut state = (definition.init)(session.header());
        for event in events {
            state = (definition.apply)(&state, event);
        }
        (definition.schema)(&(definition.view)(&state)).unwrap()
    };
    // The second pass includes a prior replacement with a newer sequence than
    // the remaining old nodes; accounting must follow surface position.
    for pass in 0..2 {
        let before = session.events();
        let nodes = session.surface().unwrap().nodes;
        let start = usize::from(before[nodes[0] as usize].type_ == "system/message");
        let replaced = &nodes[start..nodes.len() - if pass == 0 { 2 } else { 1 }];
        let expected: u64 = replaced
            .iter()
            .filter_map(|seq| dsh_session::derive_event_message(&before[*seq as usize]))
            .map(|message| dsh_token_meter::estimate_message(&message))
            .sum();
        assert!(expected > replaced.len() as u64);
        let before_tokens = project(&before)["projectedTokens"].as_u64().unwrap();
        let result = engine
            .compact_region(replaced[0], *replaced.last().unwrap(), &agent, None)
            .await
            .unwrap();
        assert_eq!(
            result.shadowed_token_count, expected,
            "price messages in tokens, never in message count"
        );
        let after = session.events();
        let replacement = after
            .iter()
            .find(|event| {
                event.seq.get() > result.summary_seq
                    && matches!(event.surface_op, Some(SurfaceOp::Replace { .. }))
            })
            .unwrap();
        let inserted = dsh_token_meter::estimate_message(
            &dsh_session::derive_event_message(replacement).unwrap(),
        );
        let projected = project(&after);
        assert_eq!(
            projected["projectedTokens"],
            Value::from(before_tokens.saturating_sub(expected) + inserted)
        );
        assert_eq!(
            projected["pressureTokens"], 50_000,
            "provider billing sample is not rewritten by compaction"
        );
        assert_eq!(
            &after[..before.len()],
            before.as_slice(),
            "original evidence remains unchanged"
        );
        let mut legacy = after.as_ref().clone();
        for event in &mut legacy {
            if event.type_ == "compaction/summary" {
                event.data["shadowedTokenCount"] =
                    json!(event.data["shadowedSeqs"].as_array().unwrap().len());
            }
        }
        assert_eq!(
            project(&legacy),
            projected,
            "cold replay repairs occupancy for historical count-as-token summaries without rewriting the log"
        );
    }
    dispose().await;
}
#[tokio::test]
async fn repeated_overflow_stops_at_recovery_limit() {
    let (h, adapter, dispose) = setup().await;
    adapter.overflow.store(100, Ordering::SeqCst);
    let before = adapter.requests.load(Ordering::SeqCst);
    h.agent.followup(message("Unrecoverable overflow"));
    tokio::time::timeout(Duration::from_secs(3), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.requests.load(Ordering::SeqCst) - before, 2);
    assert_eq!(adapter.summaries.load(Ordering::SeqCst), 1);
    assert!(
        h.agent
            .session()
            .events()
            .iter()
            .any(|e| e.type_ == "turn/end" && e.data["reason"]["kind"] == "error")
    );
    dispose().await;
}
#[tokio::test]
async fn cancellation_interrupts_an_uncooperative_summary_stream() {
    let (h, adapter, dispose) = setup().await;
    adapter.overflow.store(1, Ordering::SeqCst);
    adapter.block.store(true, Ordering::SeqCst);
    h.agent.followup(message("Cancel while summarizing"));
    tokio::time::timeout(Duration::from_secs(2), async {
        while adapter.summaries.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    h.agent.cancel(dsh_agent::AgentCancelCause::User, None);
    tokio::time::timeout(Duration::from_secs(1), h.agent.when_idle())
        .await
        .unwrap();
    assert!(
        h.agent
            .session()
            .events()
            .iter()
            .any(|e| e.type_ == "compaction/end" && e.data.get("error").is_some())
    );
    dispose().await;
}

#[tokio::test]
async fn oversized_tool_output_is_pruned_before_a_summary_and_original_log_is_preserved() {
    let (h, adapter, dispose) = setup().await;
    let pruner = dsh_compaction_tool_result_pruner::ToolResultPruner::install(
        &h.ctx,
        dsh_compaction_tool_result_pruner::ResolvedConfig {
            threshold_chars: 100,
            head_chars: 40,
            tail_chars: 10,
        },
    )
    .unwrap();
    let mut tool = super::support::quick_tool(Arc::new(AtomicBool::new(false)));
    tool.execute = Arc::new(|_, _| Box::pin(async { Ok(serde_json::json!("x".repeat(12000))) }));
    h.tools.register(&h.ctx, tool).unwrap();
    adapter.tool_first.store(true, Ordering::SeqCst);
    h.agent.followup(message("Run the large output tool"));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    let original = h
        .agent
        .session()
        .events()
        .iter()
        .find(|e| e.type_ == "tool/result")
        .unwrap()
        .clone();
    adapter.capacity.store(2000, Ordering::SeqCst);
    h.agent.followup(message("Check bounded context"));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(
        adapter.summaries.load(Ordering::SeqCst),
        0,
        "pruning removed enough pressure to avoid a model summary"
    );
    assert!(
        h.agent
            .session()
            .events()
            .iter()
            .any(|e| e.type_ == "compaction/prune")
    );
    assert_eq!(
        original.data["message"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .len(),
        12000
    );
    assert!(
        serde_json::to_string(h.agent.session().derive_messages().unwrap().as_ref())
            .unwrap()
            .contains("tool result middle pruned")
    );
    drop(pruner);
    dispose().await;
}

struct MemoryStorage;
#[async_trait::async_trait]
impl dsh_settings::SettingsStorage for MemoryStorage {
    fn writable(&self) -> bool {
        true
    }
    async fn load(&self) -> Result<indexmap::IndexMap<String, schemastery::Data>, String> {
        Ok(Default::default())
    }
    async fn persist(
        &self,
        _: &dsh_settings::SettingsNamespace,
        _: schemastery::Data,
    ) -> Result<(), String> {
        Ok(())
    }
}
#[tokio::test]
async fn memory_compaction_switch_and_threshold_apply_without_restarting_the_agent() {
    let (h, adapter, dispose) = setup().await;
    let settings = dsh_settings::SettingsProvider::install(&h.ctx, Arc::new(MemoryStorage));
    settings.ready().await.unwrap();
    let scope = settings
        .register(
            &h.ctx,
            dsh_settings::settings_namespace("memory").unwrap(),
            schemastery::Schema::object(indexmap::IndexMap::from([
                (
                    "autoCompact".into(),
                    schemastery::Schema::boolean().default(schemastery::Data::Bool(false)),
                ),
                (
                    "compactThreshold".into(),
                    schemastery::Schema::number().default(schemastery::Data::Number(0.2)),
                ),
                (
                    "protectRecentMessages".into(),
                    schemastery::Schema::number().default(schemastery::Data::Number(1.0)),
                ),
            ])),
            Default::default(),
        )
        .unwrap();
    adapter.capacity.store(1000, Ordering::SeqCst);
    h.agent.followup(message(&"large history ".repeat(180)));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.summaries.load(Ordering::SeqCst), 0);
    (scope.update)(serde_json::json!({"autoCompact":true,"compactThreshold":0.3}))
        .await
        .unwrap();
    h.agent.followup(message("Continue with live compaction"));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.summaries.load(Ordering::SeqCst), 1);
    assert_eq!(
        adapter
            .calls
            .lock()
            .iter()
            .find(|c| c.purpose.as_deref() == Some("compaction"))
            .unwrap()
            .max_tokens,
        Some(128)
    );
    dispose().await;
}
