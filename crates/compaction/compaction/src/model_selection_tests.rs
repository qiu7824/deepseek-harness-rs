use super::*;
use dsh_llm::{ChunkStream, LlmAdapter, StreamChunk};
use parking_lot::Mutex;
use serde_json::json;

#[derive(Default)]
struct RecordingAdapter {
    resolved: Mutex<Vec<(String, String)>>,
    calls: Mutex<Vec<GenerateOptions>>,
}

#[async_trait::async_trait]
impl LlmAdapter for RecordingAdapter {
    async fn resolve_model(
        &self,
        provider: &str,
        model: &str,
        _: Option<&CompactionAbort>,
    ) -> dsh_llm::LlmResolvedModelInfo {
        self.resolved.lock().push((provider.into(), model.into()));
        let reasoning = model == "reasoning-model";
        dsh_llm::LlmResolvedModelInfo {
            provider: provider.into(),
            id: model.into(),
            name: model.into(),
            system_prompt_update: None,
            execution_modes: if reasoning {
                vec![dsh_llm::ExecutionMode::Ultra]
            } else {
                vec![]
            },
            description: None,
            input_modalities: None,
            context: Some(dsh_llm::LlmModelContext {
                context_window: 100_000,
                estimated: false,
            }),
            default_max_tokens: Some(256),
            reasoning: reasoning.then(|| dsh_llm::LlmModelReasoningInfo {
                efforts: vec![dsh_llm::LlmReasoningEffortInfo {
                    id: dsh_llm::ReasoningEffortId::new("high"),
                    name: "High".into(),
                    description: None,
                }],
                default_effort: None,
            }),
        }
    }

    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        self.calls.lock().push(options.clone());
        Box::pin(futures::stream::iter([
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::Text {
                    text: "Preserved task checkpoint.".into(),
                },
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

struct Fixture {
    ctx: cordis::Context,
    engine: Arc<BasicCompactionEngine>,
    agent: CompactionAgentContext,
    adapter: Arc<RecordingAdapter>,
}

impl Fixture {
    async fn new(config: BasicCompactionConfig) -> Self {
        let ctx = cordis::Context::root();
        let llm = LlmRuntime::install(&ctx);
        let sessions = SessionStore::install(&ctx);
        TokenMeter::install(&ctx, Default::default());
        let adapter = Arc::new(RecordingAdapter::default());
        llm.register_adapter(
            &ctx,
            vec!["current-account".into(), "summary-account".into()],
            adapter.clone(),
        )
        .unwrap();
        let session = sessions.create(&ctx, None, None).await.unwrap();
        legacy_header(&session);
        for text in [
            "First requirement",
            "Second requirement",
            "Continue the task",
        ] {
            let message = create_user_message(
                vec![ContentBlock::Text { text: text.into() }],
                MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
            );
            session
                .append(
                    "user/message",
                    serde_json::to_value(message).unwrap(),
                    Some(SurfaceIntent {
                        surface_op: SurfaceOp::Append,
                        source_event_seqs: None,
                    }),
                )
                .unwrap();
        }
        let engine = BasicCompactionEngine::install_config(&ctx, config).unwrap();
        Self {
            ctx,
            engine,
            agent: CompactionAgentContext {
                session,
                provider: Some("agent-default".into()),
                model: Some("agent-default-model".into()),
            },
            adapter,
        }
    }

    fn select(&self, model: &str, execution_mode: &str, effort: Option<&str>) {
        self.agent
            .session
            .append(
                "model/selection",
                json!({
                    "provider":"current-account", "model":model,
                    "executionMode":execution_mode, "reasoningEffort":effort,
                }),
                None,
            )
            .unwrap();
    }

    async fn dispose(self) {
        for dispose in self.ctx.fiber.disposables.clear() {
            dispose().await;
        }
    }
}

fn legacy_header(session: &Session) {
    session
        .append(
            "request/header",
            json!({
                "header": {
                    "config": {
                        "provider":"removed-account", "model":"legacy-model",
                        "reasoningEffort":"legacy-only", "maxTokens":999_999,
                        "temperature":0.95, "stop":["legacy-stop"]
                    }
                },
                "reason":"resume"
            }),
            None,
        )
        .unwrap();
}

#[tokio::test]
async fn selected_route_allows_below_threshold_resume_after_old_provider_removal() {
    let fixture = Fixture::new(BasicCompactionConfig::default()).await;
    fixture.select("reasoning-model", "standard", Some("high"));
    fixture.select("plain-model", "standard", None);
    // A later request snapshot must not override an explicit selection.
    legacy_header(&fixture.agent.session);
    let before = fixture.agent.session.events();
    let result = fixture
        .engine
        .compact_if_needed(&fixture.agent, CompactionTrigger::Pressure, None)
        .await;
    assert!(matches!(result, Ok(None)), "{result:?}");
    assert!(fixture.adapter.calls.lock().is_empty());
    assert!(
        fixture
            .adapter
            .resolved
            .lock()
            .iter()
            .all(|(provider, model)| { provider == "current-account" && model == "plain-model" })
    );
    assert_eq!(fixture.agent.session.events().as_slice(), before.as_slice());
    fixture.dispose().await;
}

#[tokio::test]
async fn selected_model_compacts_without_legacy_reasoning_or_sampling_parameters() {
    let fixture = Fixture::new(BasicCompactionConfig {
        max_tokens: Some(128),
        ..Default::default()
    })
    .await;
    fixture.select("plain-model", "standard", None);
    let before = fixture.agent.session.events();
    let result = fixture
        .engine
        .compact_now(
            &ManualCompactAgentContext {
                session: fixture.agent.session.clone(),
                provider: fixture.agent.provider.clone(),
                model: fixture.agent.model.clone(),
            },
            None,
            None,
        )
        .await
        .expect("the current route must compact after the old adapter is removed")
        .expect("a checkpoint should replace older messages");
    assert!(!result.summary.is_empty());
    let calls = fixture.adapter.calls.lock();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(
        (&*call.provider, &*call.model),
        ("current-account", "plain-model")
    );
    assert_eq!(call.reasoning_effort, None);
    assert_eq!(call.max_tokens, Some(128));
    assert_eq!(call.temperature, None);
    assert_eq!(call.stop, None);
    drop(calls);
    assert_eq!(
        &fixture.agent.session.events()[..before.len()],
        before.as_slice(),
        "model selection and compaction must never rewrite old request headers"
    );
    fixture.dispose().await;
}

#[tokio::test]
async fn current_reasoning_and_execution_mode_are_resolved_for_the_selected_model() {
    let fixture = Fixture::new(BasicCompactionConfig::default()).await;
    fixture.select("reasoning-model", "ultra", Some("ultra"));
    fixture
        .engine
        .summarize(&fixture.agent, vec![], None)
        .await
        .unwrap();
    let calls = fixture.adapter.calls.lock();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].provider, "current-account");
    assert_eq!(calls[0].model, "reasoning-model");
    assert_eq!(calls[0].reasoning_effort.as_ref().unwrap().as_str(), "high");
    drop(calls);
    fixture.dispose().await;
}

#[tokio::test]
async fn independent_summary_override_keeps_its_route_and_model_capabilities() {
    for per_model in [false, true] {
        let mut config = BasicCompactionConfig {
            max_tokens: Some(128),
            summarization_provider: Some("summary-account".into()),
            summarization_model: Some("plain-model".into()),
            ..Default::default()
        };
        if per_model {
            config.summarization_provider = Some("missing-summary-account".into());
            config.summarization_model = Some("missing-summary-model".into());
            config.model_policies.push(ModelCompactPolicyConfig {
                provider: "current-account".into(),
                model: "reasoning-model".into(),
                summarization_provider: Some("summary-account".into()),
                summarization_model: Some("plain-model".into()),
                max_tokens: Some(64),
                ..Default::default()
            });
        }
        let fixture = Fixture::new(config).await;
        fixture.select("reasoning-model", "ultra", Some("ultra"));
        fixture
            .engine
            .summarize(&fixture.agent, vec![], None)
            .await
            .unwrap();
        let calls = fixture.adapter.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].provider, "summary-account");
        assert_eq!(calls[0].model, "plain-model");
        assert_eq!(calls[0].reasoning_effort, None);
        assert_eq!(calls[0].max_tokens, Some(if per_model { 64 } else { 128 }));
        drop(calls);
        fixture.dispose().await;
    }
}

#[tokio::test]
async fn missing_explicit_selection_preserves_header_and_agent_fallbacks() {
    let fixture = Fixture::new(BasicCompactionConfig::default()).await;
    assert_eq!(
        fixture.engine.target(&fixture.agent).unwrap(),
        ("removed-account".into(), "legacy-model".into())
    );
    let session = fixture
        .engine
        .sessions
        .create(&fixture.ctx, None, None)
        .await
        .unwrap();
    let fallback = CompactionAgentContext {
        session,
        ..fixture.agent.clone()
    };
    assert_eq!(
        fixture.engine.target(&fallback).unwrap(),
        ("agent-default".into(), "agent-default-model".into())
    );
    fixture.dispose().await;
}

struct CancelDuringPreparation(Arc<std::sync::atomic::AtomicBool>);

#[async_trait::async_trait]
impl LlmAdapter for CancelDuringPreparation {
    async fn snapshot_for_call(
        &self,
        _: &str,
        _: &str,
        _: Option<&CompactionAbort>,
    ) -> Result<Option<Arc<dyn LlmAdapter>>, dsh_llm::LlmError> {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        Err(dsh_llm::LlmError::new(
            "model preparation cancelled",
            "ABORTED",
            Default::default(),
        ))
    }

    fn stream(&self, _: &GenerateOptions) -> ChunkStream {
        panic!("cancelled preparation must never dispatch a request")
    }
}

#[tokio::test]
async fn cancellation_during_model_preparation_remains_a_cancelled_outcome() {
    let fixture = Fixture::new(BasicCompactionConfig::default()).await;
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    fixture
        .engine
        .llm
        .register_adapter(
            &fixture.ctx,
            vec!["cancelled-account".into()],
            Arc::new(CancelDuringPreparation(cancelled.clone())),
        )
        .unwrap();
    fixture
        .agent
        .session
        .append(
            "model/selection",
            json!({"provider":"cancelled-account","model":"plain-model"}),
            None,
        )
        .unwrap();
    let signal: CompactionAbort =
        Arc::new(move || cancelled.load(std::sync::atomic::Ordering::SeqCst));
    let error = fixture
        .engine
        .summarize(&fixture.agent, vec![], Some(&signal))
        .await
        .unwrap_err();
    assert_eq!(error.code, ManualCompactionErrorCode::Cancelled);
    fixture.dispose().await;
}

#[tokio::test]
async fn archived_compaction_preserves_surface_order_after_replacement() {
    let fixture = Fixture::new(BasicCompactionConfig::default()).await;
    fixture.select("plain-model", "standard", None);
    let original_surface = fixture.agent.session.surface().unwrap().nodes;
    let first = original_surface[0];
    let second = original_surface[1];
    let replacement = create_user_message(
        vec![ContentBlock::Text {
            text: "Replacement requirement".into(),
        }],
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    let replaced = fixture
        .agent
        .session
        .append(
            "user/message",
            serde_json::to_value(replacement).unwrap(),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Replace {
                    start: first,
                    end: first,
                },
                source_event_seqs: Some(vec![first]),
            }),
        )
        .unwrap()
        .seq
        .get();
    assert!(
        replaced > second,
        "surface order differs from event sequence order"
    );
    let mut builder =
        dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
    fixture
        .agent
        .session
        .visit_events(0, None, |event| {
            builder.push(event)?;
            Ok(true)
        })
        .unwrap();
    let archived = Session::from_event_archive(
        fixture.agent.session.id().clone(),
        builder.finish().unwrap(),
        fixture.agent.session.header(),
        fixture.agent.session.inherited_event_count(),
        vec![],
    )
    .unwrap();
    let messages = BasicCompactionEngine::selected_messages(&archived, replaced, second).unwrap();
    let texts: Vec<_> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(ContentBlock::as_text)
        .collect();
    assert_eq!(texts, ["Replacement requirement", "Second requirement"]);
    assert_eq!(
        fixture.engine.select_range(&archived, 0, 1).unwrap(),
        Some((replaced, second))
    );
    let agent = CompactionAgentContext {
        session: archived,
        ..fixture.agent.clone()
    };
    fixture
        .engine
        .summarize(&agent, messages, None)
        .await
        .unwrap();
    assert_eq!(fixture.adapter.calls.lock()[0].provider, "current-account");
    fixture.dispose().await;
}
