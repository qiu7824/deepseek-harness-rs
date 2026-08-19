//! SessionTitleService provider lifecycle. Rust port of the core
//! `packages/session/session-title/tests/provider.spec.ts` behaviors.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cordis::{Context, Disposer};
use parking_lot::Mutex;
use tokio::sync::oneshot;

use dsh_llm::{GenerateOptions, LlmAdapter, LlmRuntime, StreamChunk, mark_agent_loop_request};
use dsh_session::{Session, SessionStore, session_id};
use dsh_session_title::{
    Config, SessionTitleAutomaticMode, SessionTitleError, SessionTitleModelProvenance,
    SessionTitleProvider, SessionTitleProviderId, SessionTitleProviderRequest,
    SessionTitleProviderResult, SessionTitleService, SessionTitleUserMessage,
    session_title_provider_id,
};

fn config() -> Config {
    Config {
        fallback_max_words: 5,
        fallback_max_bytes: 24,
        max_title_bytes: 24,
    }
}

async fn setup() -> (Context, Arc<SessionStore>, Arc<SessionTitleService>) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let service = SessionTitleService::install(&ctx, config()).expect("install");
    (ctx, store, service)
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

async fn settle_threads() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn append_intent() -> dsh_session::SurfaceIntent {
    dsh_session::SurfaceIntent {
        surface_op: dsh_session::SurfaceOp::Append,
        source_event_seqs: None,
    }
}

fn append_human(session: &Session, id: &str, text: &str) -> dsh_session::SessionEvent {
    session
        .append(
            "user/message",
            serde_json::json!({
                "id": id,
                "role": "user",
                "content": [{"type": "text", "text": text}],
                "source": {"kind": "user"},
            }),
            Some(append_intent()),
        )
        .expect("append")
}

fn append_route(session: &Session, reason: &str) {
    session
        .append(
            "request/header",
            serde_json::json!({
                "header": {"config": {"provider": "main-route", "model": "chat-model"}},
                "reason": reason,
            }),
            None,
        )
        .expect("append");
}

async fn start_session(store: &SessionStore, ctx: &Context, id: &str) -> Session {
    let session = store
        .create(
            &ctx,
            Some(session_id(id)),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append("turn/start", dsh_session::turn_start_data(1), None)
        .expect("turn/start");
    session
}

fn user_message(seq: u64, text: &str) -> SessionTitleUserMessage {
    SessionTitleUserMessage {
        seq,
        text: text.to_string(),
    }
}

/// A provider whose `generate` runs a test-provided closure.
struct FnProvider {
    id: SessionTitleProviderId,
    automatic: SessionTitleAutomaticMode,
    generate_fn: Arc<
        dyn Fn(SessionTitleProviderRequest) -> Result<SessionTitleProviderResult, SessionTitleError>
            + Send
            + Sync,
    >,
}

#[async_trait]
impl SessionTitleProvider for FnProvider {
    fn id(&self) -> &SessionTitleProviderId {
        &self.id
    }

    fn automatic(&self) -> SessionTitleAutomaticMode {
        self.automatic
    }

    async fn generate(
        &self,
        request: SessionTitleProviderRequest,
    ) -> Result<SessionTitleProviderResult, SessionTitleError> {
        (self.generate_fn)(request)
    }
}

/// A provider that parks its FIRST call on a gate until released.
struct ParkingProvider {
    id: SessionTitleProviderId,
    automatic: SessionTitleAutomaticMode,
    observed: Arc<Mutex<Option<dsh_session_title::SessionTitleSignal>>>,
    gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    requests: Arc<Mutex<Vec<SessionTitleProviderRequest>>>,
}

#[async_trait]
impl SessionTitleProvider for ParkingProvider {
    fn id(&self) -> &SessionTitleProviderId {
        &self.id
    }

    fn automatic(&self) -> SessionTitleAutomaticMode {
        self.automatic
    }

    async fn generate(
        &self,
        request: SessionTitleProviderRequest,
    ) -> Result<SessionTitleProviderResult, SessionTitleError> {
        *self.observed.lock() = Some(request.signal.clone());
        self.requests.lock().push(request.clone());
        let is_first = self.requests.lock().len() == 1;
        if is_first {
            let rx = self.gate.lock().take();
            if let Some(rx) = rx {
                let _ = rx.await;
            }
        }
        let messages = self
            .requests
            .lock()
            .last()
            .map(|r| r.messages.clone())
            .unwrap_or_default();
        Ok(SessionTitleProviderResult {
            title: if is_first {
                "Old ignored result"
            } else {
                "Newest complete title"
            }
            .to_string(),
            message_seqs: messages.iter().map(|message| message.seq).collect(),
            model: None,
        })
    }
}

async fn call_disposer(disposer: &Disposer) {
    (disposer)().await;
}

#[tokio::test(flavor = "current_thread")]
async fn inherits_title_events_across_forks_skips_first_prompt_retitling_and_updates_later() {
    let (ctx, store, service) = setup().await;
    let parent = start_session(&store, &ctx, "title-parent").await;
    let inherited_message = append_human(&parent, "p1", "Inherited title prompt");
    settle().await;
    parent
        .append(
            "turn/end",
            dsh_session::turn_end_data(1, &dsh_session::TurnEndReason::Completed),
            None,
        )
        .expect("turn/end");

    let child = store
        .fork(
            &ctx,
            dsh_session::SessionForkSource::Session(parent.clone()),
            None,
            Some(session_id("title-child")),
        )
        .await
        .expect("fork");
    assert_eq!(service.get(&child), service.get(&parent));

    // first-prompt provider must not run on a fork (parentSession exists).
    let first_calls = Arc::new(AtomicUsize::new(0));
    let first_calls_for_fn = first_calls.clone();
    let first_provider = Arc::new(FnProvider {
        id: session_title_provider_id("fork-first"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        generate_fn: Arc::new(move |_request| {
            first_calls_for_fn.fetch_add(1, Ordering::SeqCst);
            Err(SessionTitleError::new("Should not run"))
        }),
    });
    let dispose_first = service.register(&ctx, first_provider).expect("register");
    child
        .append("turn/start", dsh_session::turn_start_data(2), None)
        .expect("turn/start");
    let child_message = append_human(&child, "c1", "Child follow-up prompt");
    settle().await;
    append_route(&child, "initial");
    settle().await;
    child
        .append(
            "turn/end",
            dsh_session::turn_end_data(2, &dsh_session::TurnEndReason::Completed),
            None,
        )
        .expect("turn/end");
    assert_eq!(first_calls.load(Ordering::SeqCst), 0);
    call_disposer(&dispose_first).await;

    let all_provider = Arc::new(FnProvider {
        id: session_title_provider_id("fork-all"),
        automatic: SessionTitleAutomaticMode::AllPrompts,
        generate_fn: Arc::new(move |request| {
            Ok(SessionTitleProviderResult {
                title: "Fork all prompts".to_string(),
                message_seqs: request.messages.iter().map(|message| message.seq).collect(),
                model: None,
            })
        }),
    });
    let _ = service.register(&ctx, all_provider).expect("register");
    child
        .append("turn/start", dsh_session::turn_start_data(3), None)
        .expect("turn/start");
    let latest_message = append_human(&child, "c2", "Retitle the fork now");
    settle().await;
    append_route(&child, "change");
    settle().await;
    child
        .append(
            "turn/end",
            dsh_session::turn_end_data(3, &dsh_session::TurnEndReason::Completed),
            None,
        )
        .expect("turn/end");

    let snapshot = service.get(&child).expect("title");
    assert_eq!(snapshot.title, "Fork all prompts");
    assert_eq!(
        snapshot.message_seqs,
        vec![inherited_message.seq, child_message.seq, latest_message.seq]
    );
    assert!(matches!(
        snapshot.source,
        dsh_session_title::SessionTitleSource::Provider { .. }
    ));
    assert_eq!(
        service.get(&parent).expect("parent title").title,
        "Inherited title prompt"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn runs_a_first_prompt_provider_once_after_the_routed_request_and_retries_only_through_refresh()
 {
    let (ctx, store, service) = setup().await;
    let requests: Arc<Mutex<Vec<SessionTitleProviderRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let requests_for_fn = requests.clone();
    let provider = Arc::new(FnProvider {
        id: session_title_provider_id("first-model"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        generate_fn: Arc::new(move |request| {
            requests_for_fn.lock().push(request.clone());
            Ok(SessionTitleProviderResult {
                title: "\u{1b}[31m  A   model-generated title that is too long  ".to_string(),
                message_seqs: request.messages.iter().map(|message| message.seq).collect(),
                model: Some(SessionTitleModelProvenance {
                    provider: "aux-route".to_string(),
                    model: "title-model".to_string(),
                }),
            })
        }),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let session = start_session(&store, &ctx, "first-provider").await;
    let first = append_human(&session, "f1", "Explain asynchronous title generation");
    settle().await;
    assert_eq!(
        service.get(&session).expect("title").source.kind(),
        "fallback"
    );

    append_route(&session, "initial");
    settle().await;

    assert_eq!(requests.lock().len(), 1);
    {
        let requests = requests.lock();
        assert_eq!(
            requests[0].messages,
            vec![user_message(
                first.seq,
                "Explain asynchronous title generation"
            )]
        );
        assert_eq!(
            requests[0].route,
            Some(SessionTitleModelProvenance {
                provider: "main-route".to_string(),
                model: "chat-model".to_string(),
            })
        );
    }
    let snapshot = service.get(&session).expect("title");
    assert_eq!(snapshot.title, "A model-generated title");
    assert_eq!(snapshot.message_seqs, vec![first.seq]);
    assert!(matches!(
        snapshot.source,
        dsh_session_title::SessionTitleSource::Provider { model: Some(_), .. }
    ));

    let second = append_human(&session, "f2", "A later prompt");
    append_route(&session, "change");
    settle().await;
    assert_eq!(requests.lock().len(), 1);

    service.refresh(&session, None).await.expect("refresh");
    assert_eq!(requests.lock().len(), 2);
    let requests = requests.lock();
    assert_eq!(
        requests[1]
            .messages
            .iter()
            .map(|message| message.seq)
            .collect::<Vec<u64>>(),
        vec![first.seq, second.seq]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_second_provider_and_drains_stale_work_when_the_winner_is_disposed() {
    let (ctx, store, service) = setup().await;
    let observed_signal: Arc<Mutex<Option<dsh_session_title::SessionTitleSignal>>> =
        Arc::new(Mutex::new(None));
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let gate_rx = Arc::new(Mutex::new(Some(gate_rx)));
    let requests: Arc<Mutex<Vec<SessionTitleProviderRequest>>> = Arc::new(Mutex::new(Vec::new()));

    let provider = Arc::new(ParkingProvider {
        id: session_title_provider_id("winner"),
        automatic: SessionTitleAutomaticMode::AllPrompts,
        observed: observed_signal.clone(),
        gate: gate_rx.clone(),
        requests: requests.clone(),
    });
    let dispose = service.register(&ctx, provider).expect("register");
    let duplicate = Arc::new(FnProvider {
        id: session_title_provider_id("duplicate"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        generate_fn: Arc::new(|request| {
            Ok(SessionTitleProviderResult {
                title: "duplicate".to_string(),
                message_seqs: request.messages.iter().map(|message| message.seq).collect(),
                model: None,
            })
        }),
    });
    let error = service.register(&ctx, duplicate).err().expect("reject");
    assert!(error.contains("already registered"), "{error}");

    let session = start_session(&store, &ctx, "dispose-provider").await;
    append_human(&session, "d1", "Generate this title");
    settle().await;
    append_route(&session, "initial");
    settle().await;
    assert!(
        !observed_signal
            .lock()
            .as_ref()
            .expect("signal")
            .is_aborted()
    );

    // Drive the disposer concurrently: it aborts the active call and then
    // drains until the parked provider is released.
    let disposal = dispose();
    let disposed = tokio::spawn(disposal);
    while !observed_signal
        .lock()
        .as_ref()
        .expect("signal")
        .is_aborted()
    {
        tokio::task::yield_now().await;
    }
    let _ = gate_tx.send(());
    disposed.await.expect("disposal completes");
    assert_eq!(
        service.get(&session).expect("title").source.kind(),
        "fallback"
    );

    let replacement = Arc::new(FnProvider {
        id: session_title_provider_id("replacement"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        generate_fn: Arc::new(|request| {
            Ok(SessionTitleProviderResult {
                title: "replacement".to_string(),
                message_seqs: request.messages.iter().map(|message| message.seq).collect(),
                model: None,
            })
        }),
    });
    let dispose_replacement = service.register(&ctx, replacement).expect("register");
    call_disposer(&dispose_replacement).await;
}

#[tokio::test(flavor = "current_thread")]
async fn supersedes_an_older_all_messages_revision_and_cannot_commit_an_ignored_abort() {
    let (ctx, store, service) = setup().await;
    let (first_tx, first_rx) = oneshot::channel::<()>();
    let first_rx = Arc::new(Mutex::new(Some(first_rx)));
    let requests: Arc<Mutex<Vec<SessionTitleProviderRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let observed: Arc<Mutex<Option<dsh_session_title::SessionTitleSignal>>> =
        Arc::new(Mutex::new(None));
    let provider = Arc::new(ParkingProvider {
        id: session_title_provider_id("all-model"),
        automatic: SessionTitleAutomaticMode::AllPrompts,
        observed,
        gate: first_rx.clone(),
        requests: requests.clone(),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let session = start_session(&store, &ctx, "supersede").await;
    let first = append_human(&session, "s1", "First prompt");
    settle().await;
    append_route(&session, "initial");
    settle().await;

    let second = append_human(&session, "s2", "Second prompt");
    assert!(requests.lock()[0].signal.is_aborted());
    append_route(&session, "change");
    settle().await;
    let snapshot = service.get(&session).expect("title");
    assert_eq!(snapshot.title, "Newest complete title");
    assert_eq!(snapshot.message_seqs, vec![first.seq, second.seq]);

    let _ = first_tx.send(());
    settle().await;
    assert_eq!(
        service.get(&session).expect("title").title,
        "Newest complete title"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn runs_an_all_messages_revision_when_the_next_main_request_reuses_its_logged_header() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let llm = LlmRuntime::install(&ctx);
    struct ScriptedAdapter;
    impl LlmAdapter for ScriptedAdapter {
        fn stream(&self, _options: &GenerateOptions) -> dsh_llm::ChunkStream {
            Box::pin(futures::stream::iter(vec![StreamChunk::Finish {
                reason: dsh_llm::FinishReason::Stop,
                replay_state: None,
            }]))
        }
    }
    llm.register_adapter(
        &ctx,
        vec!["main-route".to_string()],
        Arc::new(ScriptedAdapter),
    )
    .expect("adapter");
    let service = SessionTitleService::install(&ctx, config()).expect("install");

    let requests: Arc<Mutex<Vec<SessionTitleProviderRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let requests_for_fn = requests.clone();
    let provider = Arc::new(FnProvider {
        id: session_title_provider_id("unchanged-route"),
        automatic: SessionTitleAutomaticMode::AllPrompts,
        generate_fn: Arc::new(move |request| {
            let count = {
                let mut requests = requests_for_fn.lock();
                requests.push(request.clone());
                requests.len()
            };
            Ok(SessionTitleProviderResult {
                title: format!("Revision {count}"),
                message_seqs: request.messages.iter().map(|message| message.seq).collect(),
                model: None,
            })
        }),
    });
    let _ = service.register(&ctx, provider).expect("register");

    let session = start_session(&store, &ctx, "unchanged-route").await;
    let first = append_human(&session, "u1", "First routed prompt");
    settle().await;
    session
        .append("step/start", dsh_session::step_data(1, 1), None)
        .expect("step/start");
    append_route(&session, "initial");
    settle().await;
    session
        .append("step/end", dsh_session::step_data(1, 1), None)
        .expect("step/end");
    session
        .append(
            "turn/end",
            dsh_session::turn_end_data(1, &dsh_session::TurnEndReason::Completed),
            None,
        )
        .expect("turn/end");

    session
        .append("turn/start", dsh_session::turn_start_data(2), None)
        .expect("turn/start");
    let second = append_human(&session, "u2", "Second prompt on the same route");
    settle().await;
    session
        .append("step/start", dsh_session::step_data(2, 1), None)
        .expect("step/start");
    let mut options = GenerateOptions {
        provider: "main-route".to_string(),
        model: "chat-model".to_string(),
        reasoning_effort: None,
        messages: session.derive_messages().expect("messages"),
        system: None,
        tools: None,
        temperature: None,
        max_tokens: None,
        stop: None,
        signal: None,
        session_id: Some(session.id().to_string()),
        purpose: None,
        agent_loop_request: false,
    };
    mark_agent_loop_request(&mut options);
    let _stream = llm.stream(options);
    settle_threads().await;

    assert_eq!(
        session
            .events()
            .iter()
            .filter(|event| event.type_ == "request/header")
            .count(),
        1
    );
    assert_eq!(requests.lock().len(), 2);
    let requests = requests.lock();
    assert_eq!(
        requests[1]
            .messages
            .iter()
            .map(|message| message.seq)
            .collect::<Vec<u64>>(),
        vec![first.seq, second.seq]
    );
    assert_eq!(
        requests[1].route,
        Some(SessionTitleModelProvenance {
            provider: "main-route".to_string(),
            model: "chat-model".to_string(),
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn ignores_model_streams_that_are_not_a_matching_loop_request() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let llm = LlmRuntime::install(&ctx);
    struct ScriptedAdapter;
    impl LlmAdapter for ScriptedAdapter {
        fn stream(&self, _options: &GenerateOptions) -> dsh_llm::ChunkStream {
            Box::pin(futures::stream::iter(vec![StreamChunk::Finish {
                reason: dsh_llm::FinishReason::Stop,
                replay_state: None,
            }]))
        }
    }
    llm.register_adapter(
        &ctx,
        vec!["main-route".to_string()],
        Arc::new(ScriptedAdapter),
    )
    .expect("adapter");
    let service = SessionTitleService::install(&ctx, config()).expect("install");

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_fn = calls.clone();
    let provider = Arc::new(FnProvider {
        id: session_title_provider_id("request-filter"),
        automatic: SessionTitleAutomaticMode::AllPrompts,
        generate_fn: Arc::new(move |request| {
            calls_for_fn.fetch_add(1, Ordering::SeqCst);
            Ok(SessionTitleProviderResult {
                title: "Unexpected title".to_string(),
                message_seqs: request.messages.iter().map(|message| message.seq).collect(),
                model: None,
            })
        }),
    });
    let _ = service.register(&ctx, provider).expect("register");

    let base = GenerateOptions {
        provider: "main-route".to_string(),
        model: "chat-model".to_string(),
        reasoning_effort: None,
        messages: Vec::new(),
        system: None,
        tools: None,
        temperature: None,
        max_tokens: None,
        stop: None,
        signal: None,
        session_id: None,
        purpose: None,
        agent_loop_request: false,
    };

    // No session id at all.
    let _ = llm.stream(base.clone());
    // Missing session.
    let mut missing = base.clone();
    missing.session_id = Some("missing".to_string());
    mark_agent_loop_request(&mut missing);
    let _ = llm.stream(missing);

    // Live session without pending work.
    let quiet = start_session(&store, &ctx, "quiet").await;
    let mut quiet_options = base.clone();
    quiet_options.session_id = Some(quiet.id().to_string());
    mark_agent_loop_request(&mut quiet_options);
    let _ = llm.stream(quiet_options);

    // Pending work but no matching step/start boundary yet.
    let pending = start_session(&store, &ctx, "unmatched-boundary").await;
    append_human(&pending, "w1", "Wait for a matching request boundary");
    settle().await;
    let mut pending_options = base.clone();
    pending_options.session_id = Some(pending.id().to_string());
    mark_agent_loop_request(&mut pending_options);
    let _ = llm.stream(pending_options);
    settle_threads().await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn contains_automatic_failures_but_lets_explicit_refresh_reject() {
    let (ctx, store, service) = setup().await;
    let provider = Arc::new(FnProvider {
        id: session_title_provider_id("failing"),
        automatic: SessionTitleAutomaticMode::AllPrompts,
        generate_fn: Arc::new(|_request| Err(SessionTitleError::new("title backend failed"))),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let session = start_session(&store, &ctx, "failure").await;
    append_human(&session, "x1", "Keep a fallback");
    settle().await;
    append_route(&session, "initial");
    settle().await;

    assert_eq!(
        service.get(&session).expect("title").source.kind(),
        "fallback"
    );
    let error = service.refresh(&session, None).await.err().expect("reject");
    assert_eq!(error, "title backend failed");
}
