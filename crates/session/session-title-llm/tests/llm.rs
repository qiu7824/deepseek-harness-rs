//! Shared LLM title-generation policy. Rust port of the core
//! `packages/session/session-title-llm/tests/llm.spec.ts` behaviors.

use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use dsh_llm::{
    CallId, ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, LlmFailure,
    LlmRuntime, StreamChunk, create_user_message, is_agent_loop_request,
};
use dsh_session::{SessionStore, session_id};
use dsh_session_title::{
    SessionTitleProviderRequest, SessionTitleSignal, SessionTitleUserMessage,
    session_title_provider_id,
};
use dsh_session_title_llm::{
    SESSION_TITLE_TIMEOUT_CODE, SessionTitleLlmConfig, generate_session_title_with_llm,
    resolve_session_title_llm_config,
};
use dsh_timeout::MAX_TIMER_DELAY_MS;

const CONFIG: &str = r#"{
    "targetWords": 5,
    "targetCjkCharacters": 10,
    "maxInputBytes": 1000,
    "maxOutputTokens": 32,
    "timeoutMs": 1000
}"#;

fn config() -> SessionTitleLlmConfig {
    resolve_session_title_llm_config(&serde_json::from_str(CONFIG).unwrap()).unwrap()
}

const TITLE_PROVIDER: &str = "test-title-provider";

fn script() -> Vec<StreamChunk> {
    vec![
        StreamChunk::BlockStart { index: 0, block_type: "text".to_string() },
        StreamChunk::TextDelta { index: 0, text: "  五个字标题  ".to_string() },
        StreamChunk::Finish { reason: FinishReason::Stop, replay_state: None },
    ]
}

struct RecordingAdapter {
    script: Vec<StreamChunk>,
    requests: parking_lot::Mutex<Vec<GenerateOptions>>,
    on_dispatch: Option<Box<dyn Fn() + Send + Sync>>,
}

impl LlmAdapter for RecordingAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        if let Some(on_dispatch) = &self.on_dispatch {
            on_dispatch();
        }
        self.requests.lock().push(options.clone());
        Box::pin(futures::stream::iter(self.script.clone()))
    }
}

struct CooperativeAdapter;

impl LlmAdapter for CooperativeAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        Box::pin(futures::stream::pending::<StreamChunk>())
    }
}

struct DelayedSuccessAdapter {
    delay_ms: u64,
}

impl LlmAdapter for DelayedSuccessAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let delay_ms = self.delay_ms;
        let chunks: std::collections::VecDeque<StreamChunk> = script().into();
        Box::pin(futures::stream::unfold((chunks, false), move |(mut chunks, delayed)| async move {
            if !delayed {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                let first = chunks.pop_front();
                return Some((first.expect("script chunk"), (chunks, true)));
            }
            chunks
                .pop_front()
                .map(|chunk| (chunk, (chunks, true)))
        }))
    }
}

async fn harness() -> (Context, Arc<SessionStore>, Arc<LlmRuntime>) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let llm = LlmRuntime::install(&ctx);
    (ctx, store, llm)
}

async fn request(ctx: &Context, store: &SessionStore, signal: SessionTitleSignal) -> SessionTitleProviderRequest {
    let session = store
        .create(
            ctx,
            Some(session_id(format!("title-call-{}", request_counter()))),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append("turn/start", dsh_session::turn_start_data(1), None)
        .expect("turn/start");
    let first = append_human(&session, "first prompt");
    let second = append_human(&session, "第二个问题");
    session
        .append(
            "turn/end",
            dsh_session::turn_end_data(1, &dsh_session::TurnEndReason::Completed),
            None,
        )
        .expect("turn/end");
    let _ = ctx;
    SessionTitleProviderRequest {
        session,
        messages: vec![
            SessionTitleUserMessage { seq: first.seq, text: "first prompt".to_string() },
            SessionTitleUserMessage { seq: second.seq, text: "第二个问题".to_string() },
        ],
        route: Some(dsh_session_title::SessionTitleModelProvenance {
            provider: "current-route".to_string(),
            model: "current-model".to_string(),
        }),
        signal,
    }
}

fn request_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn append_human(session: &dsh_session::Session, text: &str) -> dsh_session::SessionEvent {
    session
        .append(
            "user/message",
            serde_json::json!({
                "id": format!("m{}", text),
                "role": "user",
                "content": [{"type": "text", "text": text}],
                "source": {"kind": "user"},
            }),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("append")
}

#[tokio::test(flavor = "multi_thread")]
async fn uses_the_exact_logged_route_language_targets_full_framed_input_and_output_token_cap() {
    let (ctx, store, llm) = harness().await;
    let provider_request = request(&ctx, &store, SessionTitleSignal::new()).await;
    let logged_at_dispatch = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let session_for_check = provider_request.session.clone();
    let logged_for_callback = logged_at_dispatch.clone();
    let adapter = Arc::new(RecordingAdapter {
        script: script(),
        requests: parking_lot::Mutex::new(Vec::new()),
        on_dispatch: Some(Box::new(move || {
            logged_for_callback.store(
                session_for_check
                    .events()
                    .iter()
                    .any(|event| event.type_ == "session/title-llm-request"),
                std::sync::atomic::Ordering::SeqCst,
            );
        })),
    });
    llm.register_adapter(&ctx, vec!["current-route".to_string()], adapter.clone())
        .expect("adapter");

    let result = generate_session_title_with_llm(
        &ctx,
        &config(),
        provider_request.clone(),
        provider_request.messages.clone(),
        session_title_provider_id(TITLE_PROVIDER),
    )
    .await
    .expect("generate");

    assert_eq!(result.title, "五个字标题");
    assert_eq!(
        result.message_seqs,
        provider_request.messages.iter().map(|message| message.seq).collect::<Vec<u64>>()
    );
    assert_eq!(
        result.model,
        Some(dsh_session_title::SessionTitleModelProvenance {
            provider: "current-route".to_string(),
            model: "current-model".to_string(),
        })
    );
    assert!(logged_at_dispatch.load(std::sync::atomic::Ordering::SeqCst));

    let requests = adapter.requests.lock();
    assert_eq!(requests.len(), 1);
    let options = &requests[0];
    assert!(!is_agent_loop_request(options));
    assert_eq!(options.provider, "current-route");
    assert_eq!(options.model, "current-model");
    assert_eq!(options.max_tokens, Some(32));
    assert_eq!(options.session_id.as_deref(), Some(provider_request.session.id().as_str()));
    assert_eq!(options.purpose.as_deref(), Some("session-title"));
    let system = options.system.clone().expect("system");
    assert!(system.contains("5 words"), "{system}");
    assert!(system.contains("10 CJK characters"), "{system}");
    let prompt = match &options.messages[0].content[0] {
        ContentBlock::Text { text } => text.clone(),
        other => panic!("expected text block, got {other:?}"),
    };
    assert!(prompt.contains("first prompt"), "{prompt}");
    assert!(prompt.contains("第二个问题"), "{prompt}");
    drop(requests);

    let events = provider_request.session.events();
    let event = events
        .iter()
        .rev()
        .find(|event| event.type_ == "session/title-llm-request")
        .expect("request event");
    assert_eq!(event.data["titleProvider"], serde_json::json!(TITLE_PROVIDER));
    assert_eq!(
        event.data["messageSeqs"],
        serde_json::json!(provider_request.messages.iter().map(|message| message.seq).collect::<Vec<u64>>())
    );
    assert_eq!(
        event.data["route"],
        serde_json::json!({"provider": "current-route", "model": "current-model"})
    );
    assert_eq!(event.data["system"], serde_json::json!(system));
    assert_eq!(event.data["maxTokens"], serde_json::json!(32));
    assert_eq!(event.data["messages"].as_array().expect("messages").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn uses_paired_explicit_overrides_and_bounds_the_final_framed_input_before_model_dispatch() {
    let (ctx, store, llm) = harness().await;
    let adapter = Arc::new(RecordingAdapter {
        script: script(),
        requests: parking_lot::Mutex::new(Vec::new()),
        on_dispatch: None,
    });
    llm.register_adapter(&ctx, vec!["explicit-route".to_string()], adapter.clone())
        .expect("adapter");

    let oversized = request(&ctx, &store, SessionTitleSignal::new()).await;
    let selected = oversized.messages[0].clone();
    let raw_input_bytes = selected.text.len();
    let mut config_value: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
    config_value["provider"] = serde_json::json!("explicit-route");
    config_value["model"] = serde_json::json!("explicit-model");
    config_value["maxInputBytes"] = serde_json::json!(raw_input_bytes);
    let config = resolve_session_title_llm_config(&config_value).unwrap();

    let error = generate_session_title_with_llm(
        &ctx,
        &config,
        oversized.clone(),
        vec![selected.clone()],
        session_title_provider_id(TITLE_PROVIDER),
    )
    .await
    .err()
    .expect("reject");
    assert!(error.message.contains("input"), "{}", error.message);
    assert!(error.message.contains("maxInputBytes"), "{}", error.message);
    assert!(adapter.requests.lock().is_empty());
    assert!(!oversized
        .session
        .events()
        .iter()
        .any(|event| event.type_ == "session/title-llm-request"));

    config_value["maxInputBytes"] = serde_json::json!(1000);
    let within = resolve_session_title_llm_config(&config_value).unwrap();
    let within_request = request(&ctx, &store, SessionTitleSignal::new()).await;
    let first = within_request.messages[0].clone();
    generate_session_title_with_llm(
        &ctx,
        &within,
        within_request,
        vec![first],
        session_title_provider_id(TITLE_PROVIDER),
    )
    .await
    .expect("generate");
    let requests = adapter.requests.lock();
    assert_eq!(requests[0].provider, "explicit-route");
    assert_eq!(requests[0].model, "explicit-model");
}

#[test]
fn requires_every_deployment_limit_and_a_complete_optional_route_pair() {
    assert!(resolve_session_title_llm_config(&serde_json::json!(null)).is_err());
    assert!(resolve_session_title_llm_config(&serde_json::json!("invalid")).is_err());
    let mut extra: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
    extra["extra"] = serde_json::json!(true);
    let error = resolve_session_title_llm_config(&extra).err().unwrap();
    assert!(error.contains("unknown config key \"extra\""), "{error}");

    let mut zero: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
    zero["targetWords"] = serde_json::json!(0);
    let error = resolve_session_title_llm_config(&zero).err().unwrap();
    assert!(error.contains("targetWords"), "{error}");
    assert!(error.contains("positive integer"), "{error}");

    let mut provider_only: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
    provider_only["provider"] = serde_json::json!("only-provider");
    let error = resolve_session_title_llm_config(&provider_only).err().unwrap();
    assert!(error.contains("provider and model must be supplied together"), "{error}");

    let mut model_only: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
    model_only["model"] = serde_json::json!("only-model");
    let error = resolve_session_title_llm_config(&model_only).err().unwrap();
    assert!(error.contains("provider and model must be supplied together"), "{error}");

    let mut empty_provider: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
    empty_provider["provider"] = serde_json::json!("");
    empty_provider["model"] = serde_json::json!("model");
    let error = resolve_session_title_llm_config(&empty_provider).err().unwrap();
    assert!(error.contains("non-empty strings"), "{error}");

    let mut oversized_timeout: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
    oversized_timeout["timeoutMs"] = serde_json::json!(MAX_TIMER_DELAY_MS + 1);
    let error = resolve_session_title_llm_config(&oversized_timeout).err().unwrap();
    assert!(error.contains("timeoutMs must not exceed"), "{error}");

    assert!(resolve_session_title_llm_config(&serde_json::from_str(CONFIG).unwrap()).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_an_absent_route_empty_selection_and_pre_aborted_caller_before_model_dispatch() {
    let (ctx, store, llm) = harness().await;
    let adapter = Arc::new(RecordingAdapter {
        script: script(),
        requests: parking_lot::Mutex::new(Vec::new()),
        on_dispatch: None,
    });
    llm.register_adapter(&ctx, vec!["current-route".to_string()], adapter.clone())
        .expect("adapter");
    let config = config();

    let mut unrouted = request(&ctx, &store, SessionTitleSignal::new()).await;
    unrouted.route = None;
    let error = generate_session_title_with_llm(
        &ctx,
        &config,
        unrouted,
        request(&ctx, &store, SessionTitleSignal::new()).await.messages,
        session_title_provider_id(TITLE_PROVIDER),
    )
    .await
    .err()
    .expect("reject");
    assert!(error.message.contains("no logged request route"), "{}", error.message);

    let empty = request(&ctx, &store, SessionTitleSignal::new()).await;
    let error = generate_session_title_with_llm(
        &ctx,
        &config,
        empty,
        Vec::new(),
        session_title_provider_id(TITLE_PROVIDER),
    )
    .await
    .err()
    .expect("reject");
    assert!(error.message.contains("at least one source message"), "{}", error.message);

    let signal = SessionTitleSignal::new();
    signal.abort("caller stopped");
    let aborted = request(&ctx, &store, signal).await;
    let error = generate_session_title_with_llm(
        &ctx,
        &config,
        aborted.clone(),
        aborted.messages.clone(),
        session_title_provider_id(TITLE_PROVIDER),
    )
    .await
    .err()
    .expect("reject");
    assert_eq!(error.message, "caller stopped");
    assert!(adapter.requests.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn preserves_terminal_failure_details() {
    for (reason, message, code) in [
        (
            FinishReason::Error {
                failure: LlmFailure {
                    message: "provider failed".to_string(),
                    code: "SERVER".to_string(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                },
            },
            "provider failed",
            "SERVER",
        ),
        (
            FinishReason::Aborted {
                failure: LlmFailure {
                    message: "provider aborted".to_string(),
                    code: "ABORTED".to_string(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                },
            },
            "provider aborted",
            "ABORTED",
        ),
    ] {
        let (ctx, store, llm) = harness().await;
        let adapter = Arc::new(RecordingAdapter {
            script: vec![StreamChunk::Finish { reason: reason.clone(), replay_state: None }],
            requests: parking_lot::Mutex::new(Vec::new()),
            on_dispatch: None,
        });
        llm.register_adapter(&ctx, vec!["current-route".to_string()], adapter)
            .expect("adapter");
        let provider_request = request(&ctx, &store, SessionTitleSignal::new()).await;
        let error = generate_session_title_with_llm(
            &ctx,
            &config(),
            provider_request.clone(),
            provider_request.messages,
            session_title_provider_id(TITLE_PROVIDER),
        )
        .await
        .err()
        .expect("reject");
        assert_eq!(error.message, message);
        assert_eq!(error.code.as_deref(), Some(code));
        assert!(provider_request
            .session
            .events()
            .iter()
            .any(|event| event.type_ == "session/title-llm-request"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_the_max_tokens_and_tool_calls_finish_reasons() {
    for (reason, expected) in [
        (FinishReason::MaxTokens, "reached maxOutputTokens"),
        (FinishReason::ToolCalls, "unexpectedly requested a tool"),
    ] {
        let (ctx, store, llm) = harness().await;
        let adapter = Arc::new(RecordingAdapter {
            script: vec![StreamChunk::Finish { reason, replay_state: None }],
            requests: parking_lot::Mutex::new(Vec::new()),
            on_dispatch: None,
        });
        llm.register_adapter(&ctx, vec!["current-route".to_string()], adapter)
            .expect("adapter");
        let provider_request = request(&ctx, &store, SessionTitleSignal::new()).await;
        let error = generate_session_title_with_llm(
            &ctx,
            &config(),
            provider_request.clone(),
            provider_request.messages,
            session_title_provider_id(TITLE_PROVIDER),
        )
        .await
        .err()
        .expect("reject");
        assert!(error.message.contains(expected), "{}", error.message);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_tool_call_blocks_and_a_successful_response_with_no_text() {
    let (ctx, store, llm) = harness().await;
    let tool_script = vec![
        StreamChunk::BlockStart { index: 0, block_type: "tool-call".to_string() },
        StreamChunk::ToolCallDelta {
            index: 0,
            id: CallId::new("title-tool"),
            name: Some("unexpected".to_string()),
            arguments_delta: "{}".to_string(),
        },
        StreamChunk::Finish { reason: FinishReason::Stop, replay_state: None },
    ];
    let tool_adapter = Arc::new(RecordingAdapter {
        script: tool_script,
        requests: parking_lot::Mutex::new(Vec::new()),
        on_dispatch: None,
    });
    llm.register_adapter(&ctx, vec!["current-route".to_string()], tool_adapter)
        .expect("adapter");
    let tool_request = request(&ctx, &store, SessionTitleSignal::new()).await;
    let error = generate_session_title_with_llm(
        &ctx,
        &config(),
        tool_request.clone(),
        tool_request.messages,
        session_title_provider_id(TITLE_PROVIDER),
    )
    .await
    .err()
    .expect("reject");
    assert!(error.message.contains("output must contain text only"), "{}", error.message);

    let reasoning_script = vec![
        StreamChunk::BlockStart { index: 0, block_type: "reasoning".to_string() },
        StreamChunk::ReasoningDelta { index: 0, text: "no final title".to_string() },
        StreamChunk::Finish { reason: FinishReason::Stop, replay_state: None },
    ];
    let (ctx, store, llm) = harness().await;
    let reasoning_adapter = Arc::new(RecordingAdapter {
        script: reasoning_script,
        requests: parking_lot::Mutex::new(Vec::new()),
        on_dispatch: None,
    });
    llm.register_adapter(&ctx, vec!["current-route".to_string()], reasoning_adapter)
        .expect("adapter");
    let reasoning_request = request(&ctx, &store, SessionTitleSignal::new()).await;
    let error = generate_session_title_with_llm(
        &ctx,
        &config(),
        reasoning_request.clone(),
        reasoning_request.messages,
        session_title_provider_id(TITLE_PROVIDER),
    )
    .await
    .err()
    .expect("reject");
    assert!(error.message.contains("produced no text"), "{}", error.message);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn aborts_a_cooperative_model_stream_at_the_configured_deadline() {
    let (ctx, store, llm) = harness().await;
    llm.register_adapter(&ctx, vec!["current-route".to_string()], Arc::new(CooperativeAdapter))
        .expect("adapter");
    let provider_request = request(&ctx, &store, SessionTitleSignal::new()).await;
    let mut config_value: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
    config_value["timeoutMs"] = serde_json::json!(10);
    let config = resolve_session_title_llm_config(&config_value).unwrap();

    let session = provider_request.session.clone();
    let messages = provider_request.messages.clone();
    let pending = tokio::spawn(async move {
        generate_session_title_with_llm(
            &ctx,
            &config,
            SessionTitleProviderRequest {
                session,
                messages: messages.clone(),
                route: Some(dsh_session_title::SessionTitleModelProvenance {
                    provider: "current-route".to_string(),
                    model: "current-model".to_string(),
                }),
                signal: SessionTitleSignal::new(),
            },
            messages,
            session_title_provider_id(TITLE_PROVIDER),
        )
        .await
    });
    tokio::time::advance(Duration::from_millis(10)).await;
    let error = pending.await.expect("task").err().expect("reject");
    assert_eq!(error.code.as_deref(), Some(SESSION_TITLE_TIMEOUT_CODE));
    assert_eq!(error.timeout_ms, Some(10));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn rejects_a_successful_stream_that_completes_after_the_configured_deadline() {
    let (ctx, store, llm) = harness().await;
    llm.register_adapter(
        &ctx,
        vec!["current-route".to_string()],
        Arc::new(DelayedSuccessAdapter { delay_ms: 20 }),
    )
    .expect("adapter");
    let provider_request = request(&ctx, &store, SessionTitleSignal::new()).await;
    let mut config_value: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
    config_value["timeoutMs"] = serde_json::json!(10);
    let config = resolve_session_title_llm_config(&config_value).unwrap();

    let session = provider_request.session.clone();
    let messages = provider_request.messages.clone();
    let pending = tokio::spawn(async move {
        generate_session_title_with_llm(
            &ctx,
            &config,
            SessionTitleProviderRequest {
                session,
                messages: messages.clone(),
                route: Some(dsh_session_title::SessionTitleModelProvenance {
                    provider: "current-route".to_string(),
                    model: "current-model".to_string(),
                }),
                signal: SessionTitleSignal::new(),
            },
            messages,
            session_title_provider_id(TITLE_PROVIDER),
        )
        .await
    });
    tokio::time::advance(Duration::from_millis(20)).await;
    let error = pending.await.expect("task").err().expect("reject");
    assert_eq!(error.code.as_deref(), Some(SESSION_TITLE_TIMEOUT_CODE));
    assert_eq!(error.timeout_ms, Some(10));
}

#[test]
fn framed_messages_keep_user_text_inside_structural_json() {
    let framed = dsh_session_title_llm::frame_messages(&[
        SessionTitleUserMessage { seq: 1, text: "first prompt".to_string() },
        SessionTitleUserMessage { seq: 2, text: "第二个问题".to_string() },
    ]);
    assert!(framed.contains("first prompt"), "{framed}");
    assert!(framed.contains("第二个问题"), "{framed}");
    assert!(framed.contains("\"seq\":1"), "{framed}");
    assert!(framed.starts_with("Generate the session title"), "{framed}");
}

#[test]
fn system_prompt_carries_the_language_targets() {
    let system = dsh_session_title_llm::system_prompt(&config());
    assert!(system.contains("5 words"), "{system}");
    assert!(system.contains("10 CJK characters"), "{system}");
}

#[test]
fn create_user_message_shapes_the_framed_prompt() {
    let message = create_user_message(
        vec![ContentBlock::Text { text: "framed".to_string() }],
        dsh_llm::MessageSource::Plugin {
            plugin: "dsh-session-title-llm".to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    assert_eq!(message.role, dsh_llm::Role::User);
}
