//! Runtime-layer integration tests: Rust port of
//! `packages/llm/llm/tests/service.spec.ts` (adapter registry, streaming
//! boundary, prepared calls, exact-model resolution).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc};
use dsh_llm::*;
use futures::StreamExt;

fn script() -> Vec<StreamChunk> {
    vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::TextDelta {
            index: 0,
            text: "hi".to_string(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: "hi".to_string(),
            },
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
    ]
}

async fn collect(stream: ChunkStream) -> Vec<StreamChunk> {
    let mut stream = stream;
    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk);
    }
    chunks
}

fn options_for(config: &LlmCallConfig) -> GenerateOptions {
    GenerateOptions {
        provider: config.provider.clone(),
        model: config.model.clone(),
        reasoning_effort: config.reasoning_effort.clone(),
        temperature: config.temperature,
        max_tokens: config.max_tokens,
        stop: config.stop.clone(),
        signal: None,
        session_id: None,
        purpose: None,
        agent_loop_request: false,
        messages: Vec::new(),
        system: None,
        tools: None,
    }
}

struct ScriptedAdapter {
    script: Vec<StreamChunk>,
}

impl LlmAdapter for ScriptedAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        Box::pin(futures::stream::iter(self.script.clone()))
    }
}

struct ThrowingAdapter {
    message: String,
}

impl LlmAdapter for ThrowingAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        panic!("{}", self.message)
    }
}

struct RecordingAdapter {
    script: Vec<StreamChunk>,
    last: Arc<parking_lot::Mutex<Option<GenerateOptions>>>,
}

impl LlmAdapter for RecordingAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        *self.last.lock() = Some(options.clone());
        Box::pin(futures::stream::iter(self.script.clone()))
    }
}

struct CatalogAdapter {
    provider: LlmProviderInfo,
    models: Vec<LlmModelInfo>,
    contexts: HashMap<String, LlmModelContext>,
    reasoning: HashMap<String, LlmModelReasoningInfo>,
    default_max_tokens: HashMap<String, u64>,
    resolutions: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl LlmAdapter for CatalogAdapter {
    fn provider_info(&self, _provider: &str) -> LlmProviderInfo {
        self.provider.clone()
    }

    async fn list_models(&self, _provider: &str) -> Vec<LlmModelInfo> {
        self.models.clone()
    }

    async fn resolve_model(
        &self,
        provider: &str,
        model: &str,
        _signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> LlmResolvedModelInfo {
        self.resolutions.fetch_add(1, Ordering::SeqCst);
        LlmResolvedModelInfo {
            provider: provider.to_string(),
            id: model.to_string(),
            name: model.to_string(),
            description: None,
            input_modalities: None,
            context: self.contexts.get(model).cloned(),
            default_max_tokens: self.default_max_tokens.get(model).copied(),
            reasoning: self.reasoning.get(model).cloned(),
        }
    }

    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        Box::pin(futures::stream::iter(script()))
    }
}

fn error_code(result: &Result<impl Sized, LlmError>) -> &'static str {
    match result {
        Ok(_) => "<ok>",
        Err(error) => error.code,
    }
}

#[tokio::test]
async fn routes_stream_to_registered_adapter() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["test-provider".to_string()],
            Arc::new(ScriptedAdapter { script: script() }),
        )
        .expect("register");

    let chunks = collect(runtime.stream(GenerateOptions {
        provider: "test-provider".to_string(),
        model: "test-model".to_string(),
        ..options_for(&LlmCallConfig::default())
    }))
    .await;
    assert_eq!(chunks, script());
}

#[tokio::test]
async fn unregistered_provider_becomes_terminal_failure() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);

    let chunks = collect(runtime.stream(GenerateOptions {
        provider: "nope".to_string(),
        model: "any-model".to_string(),
        ..options_for(&LlmCallConfig::default())
    }))
    .await;
    let Some(StreamChunk::Finish { reason, .. }) = chunks.last() else {
        panic!("expected a terminal finish chunk");
    };
    let FinishReason::Error { failure } = reason else {
        panic!("expected an error finish, got {:?}", reason.kind());
    };
    assert_eq!(failure.code, "NO_ADAPTER");
    assert!(failure.message.contains("no adapter registered"));
}

#[tokio::test]
async fn adapter_dispatch_failure_becomes_failure_chunk() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["test".to_string()],
            Arc::new(ThrowingAdapter {
                message: "dispatch failed".to_string(),
            }),
        )
        .expect("register");

    let chunks = collect(runtime.stream(GenerateOptions {
        provider: "test".to_string(),
        model: "test".to_string(),
        ..options_for(&LlmCallConfig::default())
    }))
    .await;
    assert_eq!(
        chunks.last(),
        Some(&StreamChunk::Finish {
            reason: FinishReason::Error {
                failure: LlmFailure {
                    message: "dispatch failed".to_string(),
                    code: "UNKNOWN".to_string(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                },
            },
            replay_state: None,
        })
    );
}

#[tokio::test]
async fn aborted_signal_maps_adapter_failure_to_aborted() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["test".to_string()],
            Arc::new(ThrowingAdapter {
                message: "stopped".to_string(),
            }),
        )
        .expect("register");

    let signal: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| true);
    let chunks = collect(runtime.stream(GenerateOptions {
        provider: "test".to_string(),
        model: "test".to_string(),
        signal: Some(signal),
        ..options_for(&LlmCallConfig::default())
    }))
    .await;
    let Some(StreamChunk::Finish { reason, .. }) = chunks.last() else {
        panic!("expected a terminal finish chunk");
    };
    let FinishReason::Aborted { failure } = reason else {
        panic!("expected an aborted finish, got {:?}", reason.kind());
    };
    assert_eq!(failure.message, "stopped");
}

#[tokio::test]
async fn captures_provider_retry_policy_and_defaults_omission() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let configured = resolve_retry_policy(
        Some(&serde_json::json!({ "mode": "always" })),
        "test retryPolicy",
    )
    .expect("configured policy");

    struct PolicyAdapter;
    impl LlmAdapter for PolicyAdapter {
        fn provider_retry_policy(&self, provider: &str) -> Option<ResolvedRetryPolicy> {
            (provider == "configured").then(|| {
                resolve_retry_policy(Some(&serde_json::json!({ "mode": "always" })), "p")
                    .expect("configured")
            })
        }
        fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
            Box::pin(futures::stream::iter(script()))
        }
    }

    runtime
        .register_adapter(
            &ctx,
            vec!["configured".to_string(), "defaulted".to_string()],
            Arc::new(PolicyAdapter),
        )
        .expect("register");

    assert_eq!(
        runtime.provider_retry_policy("configured").expect("policy"),
        configured
    );
    let defaulted = runtime.provider_retry_policy("defaulted").expect("policy");
    assert_eq!(defaulted.mode(), "normal");
    let ResolvedRetryPolicy::Normal { max_retries, .. } = defaulted else {
        panic!("expected a normal policy");
    };
    assert_eq!(max_retries, 2);
    assert_eq!(
        error_code(&runtime.provider_retry_policy("missing")),
        "NO_ADAPTER"
    );
}

#[tokio::test]
async fn prepared_call_pins_registration_across_route_replacement() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let old_policy = resolve_retry_policy(Some(&serde_json::json!({ "mode": "always" })), "old")
        .expect("old policy");
    let new_policy = resolve_retry_policy(
        Some(&serde_json::json!({ "mode": "normal", "maxRetries": 0 })),
        "new",
    )
    .expect("new policy");

    struct PolicyAdapter {
        policy: ResolvedRetryPolicy,
        script: Vec<StreamChunk>,
        throwing: bool,
    }
    impl LlmAdapter for PolicyAdapter {
        fn provider_retry_policy(&self, _provider: &str) -> Option<ResolvedRetryPolicy> {
            Some(self.policy.clone())
        }
        fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
            if self.throwing {
                panic!("old route failed");
            }
            Box::pin(futures::stream::iter(self.script.clone()))
        }
    }

    let dispose_old = runtime
        .register_adapter(
            &ctx,
            vec!["route".to_string()],
            Arc::new(PolicyAdapter {
                policy: old_policy.clone(),
                script: script(),
                throwing: true,
            }),
        )
        .expect("register");
    let prepared = runtime
        .prepare_call(
            &LlmCallConfig {
                provider: "route".to_string(),
                model: "model".to_string(),
                ..LlmCallConfig::default()
            },
            None,
        )
        .await
        .expect("prepare");

    (dispose_old.dispose)().await;
    runtime
        .register_adapter(
            &ctx,
            vec!["route".to_string()],
            Arc::new(PolicyAdapter {
                policy: new_policy.clone(),
                script: script(),
                throwing: false,
            }),
        )
        .expect("register");

    let chunks = collect((prepared.stream)(options_for(&prepared.config)).expect("dispatch")).await;
    assert_eq!(
        chunks.last(),
        Some(&StreamChunk::Finish {
            reason: FinishReason::Error {
                failure: LlmFailure {
                    message: "old route failed".to_string(),
                    code: "UNKNOWN".to_string(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                },
            },
            replay_state: None,
        })
    );
    assert_eq!(prepared.retry_policy, old_policy);
    assert_eq!(
        runtime.provider_retry_policy("route").expect("policy"),
        new_policy
    );
}

#[tokio::test]
async fn prepared_call_rejects_config_drift_and_second_dispatch() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["route".to_string()],
            Arc::new(CatalogAdapter {
                provider: LlmProviderInfo {
                    id: "route".to_string(),
                    name: "Route".to_string(),
                },
                models: Vec::new(),
                contexts: HashMap::new(),
                reasoning: HashMap::from([(
                    "model".to_string(),
                    LlmModelReasoningInfo {
                        efforts: vec![LlmReasoningEffortInfo {
                            id: reasoning_effort_id("high"),
                            name: "High".to_string(),
                            description: None,
                        }],
                        default_effort: Some(reasoning_effort_id("high")),
                    },
                )]),
                default_max_tokens: HashMap::new(),
                resolutions: Arc::new(AtomicU32::new(0)),
            }),
        )
        .expect("register");
    let prepared = runtime
        .prepare_call(
            &LlmCallConfig {
                provider: "route".to_string(),
                model: "model".to_string(),
                ..LlmCallConfig::default()
            },
            None,
        )
        .await
        .expect("prepare");
    assert_eq!(
        prepared.adapter_defaults,
        LlmCallConfigAdapterDefaults {
            reasoning_effort: Some(true),
            max_tokens: None
        }
    );

    // Config drift rejects before dispatch.
    let mut drifted = options_for(&prepared.config);
    drifted.model = "other".to_string();
    let error = (prepared.stream)(drifted).err().expect("drift must reject");
    assert_eq!(error.code, "INVALID_PREPARED_CALL");

    // One dispatch succeeds...
    collect((prepared.stream)(options_for(&prepared.config)).expect("dispatch")).await;
    // ...and the second rejects as reused.
    let error = (prepared.stream)(options_for(&prepared.config))
        .err()
        .expect("reuse must reject");
    assert_eq!(error.code, "INVALID_PREPARED_CALL");
}

#[tokio::test]
async fn reuses_one_exact_model_lookup_for_prepared_config_and_context() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let resolutions = Arc::new(AtomicU32::new(0));
    runtime
        .register_adapter(
            &ctx,
            vec!["route".to_string()],
            Arc::new(CatalogAdapter {
                provider: LlmProviderInfo {
                    id: "route".to_string(),
                    name: "Route".to_string(),
                },
                models: Vec::new(),
                contexts: HashMap::from([(
                    "model".to_string(),
                    LlmModelContext {
                        context_window: 128_000,
                    },
                )]),
                reasoning: HashMap::from([(
                    "model".to_string(),
                    LlmModelReasoningInfo {
                        efforts: vec![LlmReasoningEffortInfo {
                            id: reasoning_effort_id("high"),
                            name: "High".to_string(),
                            description: None,
                        }],
                        default_effort: Some(reasoning_effort_id("high")),
                    },
                )]),
                default_max_tokens: HashMap::new(),
                resolutions: Arc::clone(&resolutions),
            }),
        )
        .expect("register");

    let prepared = runtime
        .prepare_call(
            &LlmCallConfig {
                provider: "route".to_string(),
                model: "model".to_string(),
                ..LlmCallConfig::default()
            },
            None,
        )
        .await
        .expect("prepare");
    assert_eq!(
        prepared.config.reasoning_effort,
        Some(reasoning_effort_id("high"))
    );
    assert_eq!(
        prepared.context,
        Some(LlmModelContext {
            context_window: 128_000
        })
    );
    collect((prepared.stream)(options_for(&prepared.config)).expect("dispatch")).await;
    assert_eq!(resolutions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn materializes_reasoning_and_max_tokens_defaults() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["route".to_string()],
            Arc::new(CatalogAdapter {
                provider: LlmProviderInfo {
                    id: "route".to_string(),
                    name: "Route".to_string(),
                },
                models: Vec::new(),
                contexts: HashMap::new(),
                reasoning: HashMap::from([(
                    "model".to_string(),
                    LlmModelReasoningInfo {
                        efforts: vec![
                            LlmReasoningEffortInfo {
                                id: reasoning_effort_id("standard"),
                                name: "Standard".to_string(),
                                description: None,
                            },
                            LlmReasoningEffortInfo {
                                id: reasoning_effort_id("ultra"),
                                name: "Ultra".to_string(),
                                description: Some("Largest budget".to_string()),
                            },
                        ],
                        default_effort: Some(reasoning_effort_id("standard")),
                    },
                )]),
                default_max_tokens: HashMap::from([("model".to_string(), 256_000)]),
                resolutions: Arc::new(AtomicU32::new(0)),
            }),
        )
        .expect("register");

    let resolved = runtime
        .resolve_call_config(
            &LlmCallConfig {
                provider: "route".to_string(),
                model: "model".to_string(),
                ..LlmCallConfig::default()
            },
            None,
        )
        .await
        .expect("resolve");
    assert_eq!(
        resolved,
        LlmCallConfig {
            provider: "route".to_string(),
            model: "model".to_string(),
            reasoning_effort: Some(reasoning_effort_id("standard")),
            temperature: None,
            max_tokens: Some(256_000),
            stop: None,
        }
    );

    let explicit = LlmCallConfig {
        provider: "route".to_string(),
        model: "model".to_string(),
        reasoning_effort: Some(reasoning_effort_id("ultra")),
        max_tokens: Some(8_192),
        ..LlmCallConfig::default()
    };
    assert_eq!(
        runtime
            .resolve_call_config(&explicit, None)
            .await
            .expect("resolve"),
        explicit
    );
    let prepared_explicit = runtime
        .prepare_call(&explicit, None)
        .await
        .expect("prepare");
    assert_eq!(
        prepared_explicit.adapter_defaults,
        LlmCallConfigAdapterDefaults::default()
    );
}

#[tokio::test]
async fn rejects_unsupported_reasoning_effort() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["route".to_string()],
            Arc::new(CatalogAdapter {
                provider: LlmProviderInfo {
                    id: "route".to_string(),
                    name: "Route".to_string(),
                },
                models: Vec::new(),
                contexts: HashMap::new(),
                reasoning: HashMap::from([(
                    "model".to_string(),
                    LlmModelReasoningInfo {
                        efforts: vec![LlmReasoningEffortInfo {
                            id: reasoning_effort_id("ultra"),
                            name: "Ultra".to_string(),
                            description: None,
                        }],
                        default_effort: None,
                    },
                )]),
                default_max_tokens: HashMap::new(),
                resolutions: Arc::new(AtomicU32::new(0)),
            }),
        )
        .expect("register");

    let error = runtime
        .resolve_call_config(
            &LlmCallConfig {
                provider: "route".to_string(),
                model: "model".to_string(),
                reasoning_effort: Some(reasoning_effort_id("standard")),
                ..LlmCallConfig::default()
            },
            None,
        )
        .await
        .expect_err("unsupported effort must reject");
    assert_eq!(error.code, "UNSUPPORTED_REASONING_EFFORT");

    let error = runtime
        .resolve_call_config(
            &LlmCallConfig {
                provider: "route".to_string(),
                model: "plain".to_string(),
                reasoning_effort: Some(reasoning_effort_id("standard")),
                ..LlmCallConfig::default()
            },
            None,
        )
        .await
        .expect_err("plain model must reject");
    assert_eq!(error.code, "UNSUPPORTED_REASONING_EFFORT");
}

#[tokio::test]
async fn rejects_invalid_exact_model_metadata() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);

    // Mismatched provider identity.
    runtime
        .register_adapter(
            &ctx,
            vec!["route".to_string()],
            Arc::new(CatalogAdapter {
                provider: LlmProviderInfo {
                    id: "route".to_string(),
                    name: "Route".to_string(),
                },
                models: Vec::new(),
                contexts: HashMap::new(),
                reasoning: HashMap::new(),
                default_max_tokens: HashMap::new(),
                resolutions: Arc::new(AtomicU32::new(0)),
            }),
        )
        .expect("register");
    struct MismatchAdapter;
    #[async_trait::async_trait]
    impl LlmAdapter for MismatchAdapter {
        async fn resolve_model(
            &self,
            _provider: &str,
            model: &str,
            _signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
        ) -> LlmResolvedModelInfo {
            LlmResolvedModelInfo {
                provider: "other".to_string(),
                id: model.to_string(),
                name: model.to_string(),
                description: None,
                input_modalities: None,
                context: None,
                default_max_tokens: None,
                reasoning: None,
            }
        }
        fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
            Box::pin(futures::stream::iter(script()))
        }
    }
    runtime
        .register_adapter(
            &ctx,
            vec!["mismatch".to_string()],
            Arc::new(MismatchAdapter),
        )
        .expect("register");
    let error = runtime
        .resolve_model_info("mismatch", "model", None)
        .await
        .expect_err("mismatched provider must reject");
    assert_eq!(error.code, "INVALID_MODEL_INFO");

    // Invalid context / maxTokens / reasoning metadata.
    runtime
        .register_adapter(
            &ctx,
            vec!["invalid".to_string()],
            Arc::new(CatalogAdapter {
                provider: LlmProviderInfo {
                    id: "invalid".to_string(),
                    name: "Invalid".to_string(),
                },
                models: Vec::new(),
                contexts: HashMap::from([(
                    "ctx".to_string(),
                    LlmModelContext { context_window: 0 },
                )]),
                reasoning: HashMap::from([
                    (
                        "empty".to_string(),
                        LlmModelReasoningInfo {
                            efforts: Vec::new(),
                            default_effort: None,
                        },
                    ),
                    (
                        "dup".to_string(),
                        LlmModelReasoningInfo {
                            efforts: vec![
                                LlmReasoningEffortInfo {
                                    id: reasoning_effort_id("same"),
                                    name: "One".to_string(),
                                    description: None,
                                },
                                LlmReasoningEffortInfo {
                                    id: reasoning_effort_id("same"),
                                    name: "Two".to_string(),
                                    description: None,
                                },
                            ],
                            default_effort: None,
                        },
                    ),
                    (
                        "unknown-default".to_string(),
                        LlmModelReasoningInfo {
                            efforts: vec![LlmReasoningEffortInfo {
                                id: reasoning_effort_id("valid"),
                                name: "Valid".to_string(),
                                description: None,
                            }],
                            default_effort: Some(reasoning_effort_id("other")),
                        },
                    ),
                ]),
                default_max_tokens: HashMap::from([("tokens".to_string(), 0)]),
                resolutions: Arc::new(AtomicU32::new(0)),
            }),
        )
        .expect("register");
    for (model, code) in [
        ("ctx", "INVALID_MODEL_CONTEXT"),
        ("tokens", "INVALID_MODEL_MAX_TOKENS"),
        ("empty", "INVALID_MODEL_REASONING"),
        ("dup", "INVALID_MODEL_REASONING"),
        ("unknown-default", "INVALID_MODEL_REASONING"),
    ] {
        let error = runtime
            .resolve_model_info("invalid", model, None)
            .await
            .expect_err(model);
        assert_eq!(error.code, code, "model {model}");
    }
}

#[tokio::test]
async fn defaults_adapters_to_route_name_and_empty_catalog() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["plain".to_string()],
            Arc::new(ScriptedAdapter { script: script() }),
        )
        .expect("register");

    assert_eq!(
        runtime.list_providers(),
        vec![LlmProviderInfo {
            id: "plain".to_string(),
            name: "plain".to_string()
        }]
    );
    assert!(
        runtime
            .list_models("plain")
            .await
            .expect("models")
            .is_empty()
    );
    assert_eq!(
        error_code(&runtime.list_models("missing").await),
        "NO_ADAPTER"
    );
    assert_eq!(
        runtime
            .resolve_model_info("plain", "unlisted", None)
            .await
            .expect("resolve"),
        LlmResolvedModelInfo {
            provider: "plain".to_string(),
            id: "unlisted".to_string(),
            name: "unlisted".to_string(),
            description: None,
            input_modalities: None,
            context: None,
            default_max_tokens: None,
            reasoning: None,
        }
    );
    assert_eq!(
        error_code(&runtime.resolve_model_info("missing", "m", None).await),
        "NO_ADAPTER"
    );
}

#[tokio::test]
async fn rejects_invalid_or_duplicate_model_catalog_metadata() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["route".to_string()],
            Arc::new(CatalogAdapter {
                provider: LlmProviderInfo {
                    id: "route".to_string(),
                    name: "Route".to_string(),
                },
                models: vec![LlmModelInfo {
                    provider: "other".to_string(),
                    id: "m".to_string(),
                    name: "M".to_string(),
                    description: None,
                    input_modalities: None,
                }],
                contexts: HashMap::new(),
                reasoning: HashMap::new(),
                default_max_tokens: HashMap::new(),
                resolutions: Arc::new(AtomicU32::new(0)),
            }),
        )
        .expect("register");
    assert_eq!(
        error_code(&runtime.list_models("route").await),
        "INVALID_CATALOG"
    );

    runtime
        .register_adapter(
            &ctx,
            vec!["dup".to_string()],
            Arc::new(CatalogAdapter {
                provider: LlmProviderInfo {
                    id: "dup".to_string(),
                    name: "Dup".to_string(),
                },
                models: vec![
                    LlmModelInfo {
                        provider: "dup".to_string(),
                        id: "same".to_string(),
                        name: "Same".to_string(),
                        description: None,
                        input_modalities: None,
                    },
                    LlmModelInfo {
                        provider: "dup".to_string(),
                        id: "same".to_string(),
                        name: "Same".to_string(),
                        description: None,
                        input_modalities: None,
                    },
                ],
                contexts: HashMap::new(),
                reasoning: HashMap::new(),
                default_max_tokens: HashMap::new(),
                resolutions: Arc::new(AtomicU32::new(0)),
            }),
        )
        .expect("register");
    assert_eq!(
        error_code(&runtime.list_models("dup").await),
        "INVALID_CATALOG"
    );
}

#[tokio::test]
async fn llm_stream_listener_wraps_underlying_stream() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["test-model".to_string()],
            Arc::new(ScriptedAdapter { script: script() }),
        )
        .expect("register");

    let listener: Arc<cordis::Listener> = Arc::new(|_ctx, args| {
        let next = cordis::downcast_arc::<cordis::NextFn>(&args[1])
            .expect("next")
            .clone();
        Box::pin(async move {
            let value = next.call().await;
            let inner = cordis::downcast_arc::<StreamFactory>(&value)
                .expect("factory")
                .as_ref()
                .clone();
            let wrapped: StreamFactory = Arc::new(move |options| {
                let prefix = futures::stream::iter(vec![
                    StreamChunk::BlockStart {
                        index: 99,
                        block_type: "text".to_string(),
                    },
                    StreamChunk::BlockEnd {
                        index: 99,
                        block: ContentBlock::Text {
                            text: String::new(),
                        },
                    },
                ]);
                Box::pin(prefix.chain(inner(options)))
            });
            Some(cordis::arc(wrapped))
        })
    });
    ctx.on("llm/stream", listener, cordis::EventOptions::default())
        .await;

    let chunks = collect(runtime.stream(GenerateOptions {
        provider: "test-model".to_string(),
        model: "dynamic-model".to_string(),
        ..options_for(&LlmCallConfig::default())
    }))
    .await;
    assert_eq!(chunks.len(), 6);
    assert_eq!(
        chunks[0],
        StreamChunk::BlockStart {
            index: 99,
            block_type: "text".to_string()
        }
    );
}

#[tokio::test]
async fn llm_stream_listener_routes_provider_before_adapter_selection() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let last = Arc::new(parking_lot::Mutex::new(None));
    runtime
        .register_adapter(
            &ctx,
            vec!["routed".to_string()],
            Arc::new(RecordingAdapter {
                script: script(),
                last: Arc::clone(&last),
            }),
        )
        .expect("register");

    let listener: Arc<cordis::Listener> = Arc::new(|_ctx, args| {
        let cell = cordis::downcast_arc::<Arc<parking_lot::Mutex<GenerateOptions>>>(&args[0])
            .expect("cell")
            .clone();
        let next = cordis::downcast_arc::<cordis::NextFn>(&args[1])
            .expect("next")
            .clone();
        Box::pin(async move {
            cell.lock().provider = "routed".to_string();
            Some(next.call().await)
        })
    });
    ctx.on("llm/stream", listener, cordis::EventOptions::default())
        .await;

    collect(runtime.stream(GenerateOptions {
        provider: "initial".to_string(),
        model: "m".to_string(),
        ..options_for(&LlmCallConfig::default())
    }))
    .await;
    assert_eq!(last.lock().as_ref().expect("recorded").provider, "routed");
}

#[tokio::test]
async fn replay_state_kept_for_same_adapter_stripped_for_other_adapter() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let last_same = Arc::new(parking_lot::Mutex::new(None));
    let adapter: Arc<dyn LlmAdapter> = Arc::new(RecordingAdapter {
        script: script(),
        last: Arc::clone(&last_same),
    });
    runtime
        .register_adapter(
            &ctx,
            vec!["historical".to_string(), "target".to_string()],
            Arc::clone(&adapter),
        )
        .expect("register");

    let historical_message = create_assistant_message(
        vec![ContentBlock::Text {
            text: "old response".to_string(),
        }],
        ModelMessageSource {
            provider: "historical".to_string(),
            model: "old-model".to_string(),
            replay_state: Some(serde_json::json!({ "private": "state" })),
        },
    );
    collect(runtime.stream(GenerateOptions {
        provider: "target".to_string(),
        model: "new-model".to_string(),
        messages: vec![historical_message.clone()],
        ..options_for(&LlmCallConfig::default())
    }))
    .await;
    let seen = last_same.lock().as_ref().expect("recorded").messages[0].clone();
    assert_eq!(
        seen.source,
        MessageSource::Model {
            provider: "historical".to_string(),
            model: "old-model".to_string(),
            replay_state: Some(serde_json::json!({ "private": "state" })),
        }
    );

    // A different adapter instance owns the target route: replay state is
    // stripped while provider/model survive.
    let last_target = Arc::new(parking_lot::Mutex::new(None));
    runtime
        .register_adapter(
            &ctx,
            vec!["other-target".to_string()],
            Arc::new(RecordingAdapter {
                script: script(),
                last: Arc::clone(&last_target),
            }),
        )
        .expect("register");
    collect(runtime.stream(GenerateOptions {
        provider: "other-target".to_string(),
        model: "new-model".to_string(),
        messages: vec![historical_message],
        ..options_for(&LlmCallConfig::default())
    }))
    .await;
    let seen = last_target.lock().as_ref().expect("recorded").messages[0].clone();
    assert_eq!(
        seen.source,
        MessageSource::Model {
            provider: "historical".to_string(),
            model: "old-model".to_string(),
            replay_state: None,
        }
    );
}

#[tokio::test]
async fn duplicate_empty_and_invalid_adapter_registrations_reject_atomically() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["m1".to_string()],
            Arc::new(ScriptedAdapter { script: script() }),
        )
        .expect("register");

    let error = runtime
        .register_adapter(
            &ctx,
            vec!["m1".to_string()],
            Arc::new(ScriptedAdapter { script: script() }),
        )
        .err()
        .expect("duplicate must reject");
    assert_eq!(error.code, "DUPLICATE_ADAPTER");
    assert!(error.message.contains("already registered"));

    let dormant = runtime
        .register_adapter(
            &ctx,
            Vec::new(),
            Arc::new(ScriptedAdapter { script: script() }),
        )
        .expect("empty route set is a live dormant registration");
    (dormant.replace)(vec!["later".to_string()]).expect("activate dormant registration");
    assert!(
        runtime
            .list_providers()
            .iter()
            .any(|entry| entry.id == "later")
    );
    (dormant.replace)(Vec::new()).expect("return registration to dormant state");
    assert_eq!(
        error_code(&runtime.register_adapter(
            &ctx,
            vec![String::new()],
            Arc::new(ScriptedAdapter { script: script() })
        )),
        "INVALID_ADAPTER"
    );
    assert_eq!(
        error_code(&runtime.register_adapter(
            &ctx,
            vec!["first".to_string(), "first".to_string()],
            Arc::new(ScriptedAdapter { script: script() })
        )),
        "DUPLICATE_ADAPTER"
    );
    assert_eq!(
        runtime.list_providers(),
        vec![LlmProviderInfo {
            id: "m1".to_string(),
            name: "m1".to_string()
        }]
    );
}

#[tokio::test]
async fn replace_routes_atomically_and_guard_disposed_registration() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let handle = runtime
        .register_adapter(
            &ctx,
            vec!["m1".to_string()],
            Arc::new(ScriptedAdapter { script: script() }),
        )
        .expect("register");

    // An empty replacement is legal (the settings-section-emptied case).
    (handle.replace)(Vec::new()).expect("replace");
    assert!(runtime.list_providers().is_empty());
    (handle.replace)(vec!["m2".to_string()]).expect("replace");
    assert_eq!(
        runtime.list_providers(),
        vec![LlmProviderInfo {
            id: "m2".to_string(),
            name: "m2".to_string()
        }]
    );

    (handle.dispose)().await;
    assert!(runtime.list_providers().is_empty());
    let error =
        (handle.replace)(vec!["leaked".to_string()]).expect_err("disposed replace must refuse");
    assert_eq!(error.code, "REGISTRATION_DISPOSED");
}

struct RegisteringPlugin {
    adapter: Arc<dyn LlmAdapter>,
}

#[async_trait::async_trait]
impl Plugin for RegisteringPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("test-adapter-registration")
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["llm"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let llm = ctx
            .get_typed::<Arc<LlmRuntime>>("llm", false)
            .expect("llm service");
        llm.register_adapter(
            ctx,
            vec!["scoped-model".to_string()],
            Arc::clone(&self.adapter),
        )
        .expect("register");
        Ok(())
    }
}

#[tokio::test]
async fn unregisters_adapters_when_owning_fiber_disposes() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let fiber = ctx.plugin(
        Arc::new(RegisteringPlugin {
            adapter: Arc::new(ScriptedAdapter { script: script() }),
        }),
        arc(()),
    );
    fiber.settle().await.expect("fiber settles");
    assert_eq!(
        runtime.list_providers(),
        vec![LlmProviderInfo {
            id: "scoped-model".to_string(),
            name: "scoped-model".to_string()
        }]
    );
    fiber.dispose().await;
    assert!(runtime.list_providers().is_empty());
}
