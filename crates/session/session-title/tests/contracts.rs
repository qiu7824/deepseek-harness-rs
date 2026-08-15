//! SessionTitleService configuration and refresh boundaries. Rust port of
//! the core `packages/session/session-title/tests/service-contracts.spec.ts`
//! behaviors (type-level inexpressible cases collapsed; disposal tests run
//! through the plugin fiber).

use std::sync::Arc;

use async_trait::async_trait;
use cordis::{Context, arc};
use parking_lot::Mutex;
use tokio::sync::oneshot;

use dsh_session::{Session, SessionEvent, SessionStore, session_id};
use dsh_session_title::{
    Config, SessionTitleAutomaticMode, SessionTitleError, SessionTitlePlugin,
    SessionTitleProvider, SessionTitleProviderId, SessionTitleProviderRequest,
    SessionTitleProviderResult, SessionTitleService, SessionTitleSignal,
    SessionTitleSource, session_title_provider_id,
};

fn config() -> Config {
    Config {
        fallback_max_words: 5,
        fallback_max_bytes: 40,
        max_title_bytes: 80,
    }
}

async fn setup() -> (Context, Arc<SessionStore>, Arc<SessionTitleService>) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let service = SessionTitleService::install(&ctx, config()).expect("install");
    (ctx, store, service)
}

/// Mount the service through its plugin fiber (disposal tests).
async fn setup_plugin() -> (Context, Arc<SessionStore>, Arc<SessionTitleService>, Arc<cordis::FiberCore>) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let fiber = ctx.plugin(Arc::new(SessionTitlePlugin), arc(config()));
    fiber.settle().await.expect("settle");
    let service = ctx
        .get_typed::<Arc<SessionTitleService>>("sessionTitle", false)
        .expect("sessionTitle service")
        .as_ref()
        .clone();
    (ctx, store, service, fiber)
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
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

async fn enter_session(store: &SessionStore, id: &str) -> (Session, cordis::Disposer) {
    let prepared = store
        .prepare(
            Some(session_id(id)),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .expect("prepare");
    let detach = store.enter(&prepared).expect("enter");
    store.announce(&prepared).await.expect("announce");
    (prepared, detach)
}

fn title_event(seq: u64, title: &str, source: serde_json::Value) -> SessionEvent {
    SessionEvent {
        type_: "session/title".to_string(),
        seq,
        time: 0,
        data: serde_json::json!({
            "title": title,
            "messageSeqs": if source["kind"] == "user" { serde_json::json!([]) } else { serde_json::json!([1]) },
            "source": source,
        }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

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

struct GateProvider {
    id: SessionTitleProviderId,
    automatic: SessionTitleAutomaticMode,
    observed: Arc<Mutex<Option<SessionTitleSignal>>>,
    gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    result: SessionTitleProviderResult,
}

#[async_trait]
impl SessionTitleProvider for GateProvider {
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
        let rx = self.gate.lock().take();
        if let Some(rx) = rx {
            let _ = rx.await;
        }
        Ok(self.result.clone())
    }
}

fn parked_result() -> SessionTitleProviderResult {
    SessionTitleProviderResult {
        title: "ignored".to_string(),
        message_seqs: vec![0],
        model: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn returns_no_title_for_empty_input_and_rejects_detached_or_pre_aborted_refreshes() {
    // Fallback-only path.
    let (_ctx, store, service) = setup().await;
    let empty = start_session(&store, &_ctx, "empty-fallback").await;
    assert!(service.refresh(&empty, None).await.expect("refresh").is_none());

    // Provider path: no eligible message → no call, no title.
    let (ctx, store, service) = setup().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_for_fn = calls.clone();
    let provider = Arc::new(FnProvider {
        id: session_title_provider_id("empty-provider"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        generate_fn: Arc::new(move |_request| {
            calls_for_fn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(SessionTitleProviderResult {
                title: "unused".to_string(),
                message_seqs: vec![0],
                model: None,
            })
        }),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let provider_empty = start_session(&store, &ctx, "empty-provider").await;
    assert!(service.refresh(&provider_empty, None).await.expect("refresh").is_none());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Detached session.
    let detached = Session::create(session_id("detached"), None, None).expect("create");
    let error = service.refresh(&detached, None).await.err().expect("reject");
    assert!(error.contains("not live in this store"), "{error}");

    // Pre-aborted caller.
    let signal = SessionTitleSignal::new();
    signal.abort("already cancelled");
    let error = service
        .refresh(&provider_empty, Some(&signal))
        .await
        .err()
        .expect("reject");
    assert_eq!(error, "already cancelled");
}

#[tokio::test(flavor = "current_thread")]
async fn passes_an_absent_route_and_caller_cancellation_into_explicit_generation() {
    let (ctx, store, service) = setup().await;
    let observed: Arc<Mutex<Option<SessionTitleProviderRequest>>> = Arc::new(Mutex::new(None));
    let observed_for_fn = observed.clone();
    let provider = Arc::new(FnProvider {
        id: session_title_provider_id("explicit-no-route"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        generate_fn: Arc::new(move |request| {
            let seqs = request.messages.iter().map(|message| message.seq).collect::<Vec<u64>>();
            *observed_for_fn.lock() = Some(request.clone());
            Ok(SessionTitleProviderResult {
                title: "Explicit title".to_string(),
                message_seqs: seqs,
                model: None,
            })
        }),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let session = start_session(&store, &ctx, "explicit-no-route").await;
    append_human(&session, "e1", "Refresh before any request header");
    settle().await;
    let signal = SessionTitleSignal::new();

    let refreshed = service
        .refresh(&session, Some(&signal))
        .await
        .expect("refresh")
        .expect("title");
    assert_eq!(refreshed.title, "Explicit title");
    let observed = observed.lock().clone().expect("observed");
    assert!(observed.route.is_none());
    assert!(!observed.signal.is_aborted());
}

#[tokio::test(flavor = "current_thread")]
async fn propagates_explicit_cancellation_and_session_disposal_to_active_work() {
    // Caller cancellation.
    let (ctx, store, service) = setup().await;
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let observed: Arc<Mutex<Option<SessionTitleSignal>>> = Arc::new(Mutex::new(None));
    let provider = Arc::new(GateProvider {
        id: session_title_provider_id("caller-cancel"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        observed: observed.clone(),
        gate: Arc::new(Mutex::new(Some(gate_rx))),
        result: parked_result(),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let session = start_session(&store, &ctx, "caller-cancel").await;
    append_human(&session, "c1", "Cancel this refresh");
    settle().await;
    let signal = SessionTitleSignal::new();
    let session_for_task = session.clone();
    let signal_for_task = signal.clone();
    let service_for_task = service.clone();
    let refresh = tokio::spawn(async move {
        service_for_task
            .refresh(&session_for_task, Some(&signal_for_task))
            .await
    });
    tokio::task::yield_now().await;
    signal.abort("caller cancelled");
    let _ = gate_tx.send(());
    let error = refresh.await.expect("task").err().expect("reject");
    assert_eq!(error, "caller cancelled");
    assert!(observed.lock().as_ref().expect("signal").is_aborted());

    // Session disposal.
    let (ctx, store, service) = setup().await;
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let observed: Arc<Mutex<Option<SessionTitleSignal>>> = Arc::new(Mutex::new(None));
    let provider = Arc::new(GateProvider {
        id: session_title_provider_id("session-dispose"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        observed: observed.clone(),
        gate: Arc::new(Mutex::new(Some(gate_rx))),
        result: parked_result(),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let (disposed, detach) = enter_session(&store, "session-dispose").await;
    disposed
        .append("turn/start", dsh_session::turn_start_data(1), None)
        .expect("turn/start");
    append_human(&disposed, "d1", "Dispose this session");
    settle().await;
    let disposed_for_task = disposed.clone();
    let service_for_task = service.clone();
    let refresh = tokio::spawn(async move { service_for_task.refresh(&disposed_for_task, None).await });
    tokio::task::yield_now().await;
    detach().await;
    let _ = gate_tx.send(());
    let error = refresh.await.expect("task").err().expect("reject");
    assert!(error.contains("session disposed"), "{error}");
    assert!(observed.lock().as_ref().expect("signal").is_aborted());
}

#[tokio::test(flavor = "current_thread")]
async fn shares_one_fallback_across_concurrent_refreshes() {
    let (_ctx, store, service) = setup().await;
    let seed = Session::create(session_id("fallback-concurrency-seed"), None, None).expect("seed");
    seed.append("turn/start", dsh_session::turn_start_data(1), None).expect("turn/start");
    let source = append_human(&seed, "s1", "Create exactly one fallback title");
    seed.append(
        "turn/end",
        dsh_session::turn_end_data(1, &dsh_session::TurnEndReason::Completed),
        None,
    )
    .expect("turn/end");

    let seed_events = seed.events().as_ref().clone();
    let session = store
        .create(
            &_ctx,
            Some(session_id("fallback-concurrency")),
            Some(dsh_session::CreateSessionOptions {
                seed: Some(seed_events),
                meta: None,
            }),
        )
        .await
        .expect("create");

    let (a, b) = futures::join!(
        service.refresh(&session, None),
        service.refresh(&session, None)
    );
    assert_eq!(a, b);
    assert_eq!(
        session
            .events()
            .iter()
            .filter(|event| event.type_ == "session/title")
            .count(),
        1
    );
    let events = session.events();
    let event_types: Vec<&str> = events.iter().map(|event| event.type_.as_str()).collect();
    assert_eq!(
        event_types,
        vec![
            "turn/start",
            "user/message",
            "turn/end",
            "session/end-seed",
            "session/title",
        ]
    );
    assert_eq!(
        service.get(&session).expect("title").message_seqs,
        vec![source.seq]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn reuses_a_title_accepted_before_the_queued_fallback_commits() {
    let (_ctx, store, service) = setup().await;
    let session = start_session(&store, &_ctx, "fallback-already-accepted").await;
    let source = append_human(&session, "r1", "Reuse the title that wins the fallback race");

    // The refresh future is not polled until awaited: the direct append
    // below lands before the queued fallback runs.
    let refresh = service.refresh(&session, None);
    session
        .append(
            "session/title",
            serde_json::json!({
                "title": "Already accepted",
                "messageSeqs": [source.seq],
                "source": {"kind": "fallback"},
            }),
            None,
        )
        .expect("append");

    let refreshed = refresh.await.expect("refresh").expect("title");
    assert_eq!(refreshed.title, "Already accepted");
    assert_eq!(
        session
            .events()
            .iter()
            .filter(|event| event.type_ == "session/title")
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lets_the_newest_overlapping_explicit_refresh_win() {
    let (ctx, store, service) = setup().await;
    let session = start_session(&store, &ctx, "refresh-order").await;
    let source = append_human(&session, "r1", "Keep the newest explicit refresh");
    settle().await;
    session
        .append(
            "turn/end",
            dsh_session::turn_end_data(1, &dsh_session::TurnEndReason::Completed),
            None,
        )
        .expect("turn/end");

    let gates: Arc<Mutex<Vec<oneshot::Sender<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let requests: Arc<Mutex<Vec<SessionTitleProviderRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let gates_for_fn = gates.clone();
    let requests_for_fn = requests.clone();
    let provider = Arc::new(OrderedGateProvider {
        id: session_title_provider_id("refresh-order"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        gates: gates_for_fn,
        requests: requests_for_fn,
        source_seq: source.seq,
    });
    let _ = service.register(&ctx, provider).expect("register");

    let session_for_older = session.clone();
    let service_for_older = service.clone();
    let older = tokio::spawn(async move { service_for_older.refresh(&session_for_older, None).await });
    tokio::task::yield_now().await;
    let session_for_newer = session.clone();
    let service_for_newer = service.clone();
    let newer = tokio::spawn(async move { service_for_newer.refresh(&session_for_newer, None).await });
    tokio::task::yield_now().await;

    assert_eq!(requests.lock().len(), 2);
    assert!(requests.lock()[0].signal.is_aborted());
    assert!(!requests.lock()[1].signal.is_aborted());

    let first_gate = gates.lock().remove(0);
    let _ = first_gate.send(());
    let error = older.await.expect("task").err().expect("reject");
    assert!(error.contains("superseded"), "{error}");

    let second_gate = gates.lock().remove(0);
    let _ = second_gate.send(());
    let newest = newer.await.expect("task").expect("refresh").expect("title");
    assert_eq!(newest.title, "Newest explicit title");
}

struct OrderedGateProvider {
    id: SessionTitleProviderId,
    automatic: SessionTitleAutomaticMode,
    gates: Arc<Mutex<Vec<oneshot::Sender<()>>>>,
    requests: Arc<Mutex<Vec<SessionTitleProviderRequest>>>,
    source_seq: u64,
}

#[async_trait]
impl SessionTitleProvider for OrderedGateProvider {
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
        self.requests.lock().push(request.clone());
        let is_first = self.requests.lock().len() == 1;
        let (tx, rx) = oneshot::channel::<()>();
        self.gates.lock().push(tx);
        let _ = rx.await;
        Ok(SessionTitleProviderResult {
            title: if is_first { "Obsolete title" } else { "Newest explicit title" }.to_string(),
            message_seqs: vec![self.source_seq],
            model: None,
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancels_a_queued_fallback_when_the_session_title_service_unloads() {
    let (_ctx, store, service, fiber) = setup_plugin().await;
    let session = start_session(&store, &_ctx, "service-dispose-fallback").await;
    append_human(&session, "u1", "Do not publish after service disposal");

    fiber.dispose().await;

    assert!(!session
        .events()
        .iter()
        .any(|event| event.type_ == "session/title"));
    let error = service.refresh(&session, None).await.err().expect("reject");
    assert_eq!(error, "session-title service disposed");
}

#[tokio::test(flavor = "current_thread")]
async fn aborts_pending_and_active_provider_work_and_drains_ignored_cancellation_during_service_unload() {
    let (ctx, store, service, fiber) = setup_plugin().await;
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let requests: Arc<Mutex<Vec<SessionTitleProviderRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let requests_for_fn = requests.clone();
    let provider = Arc::new(RequestParkingProvider {
        id: session_title_provider_id("service-unload"),
        automatic: SessionTitleAutomaticMode::AllPrompts,
        requests: requests_for_fn,
        gate: Arc::new(Mutex::new(Some(gate_rx))),
    });
    let _ = service.register(&ctx, provider).expect("register");

    let active = start_session(&store, &ctx, "service-unload-active").await;
    append_human(&active, "a1", "Active provider work");
    settle().await;
    let active_for_task = active.clone();
    let service_for_task = service.clone();
    let refresh = tokio::spawn(async move { service_for_task.refresh(&active_for_task, None).await });
    tokio::task::yield_now().await;
    assert_eq!(requests.lock().len(), 1);

    let pending = start_session(&store, &ctx, "service-unload-pending").await;
    append_human(&pending, "p1", "Pending provider work");

    let fiber_for_task = fiber.clone();
    let disposal = tokio::spawn(async move { fiber_for_task.dispose().await });
    while !requests.lock()[0].signal.is_aborted() {
        tokio::task::yield_now().await;
    }
    let _ = gate_tx.send(());
    disposal.await.expect("disposal");
    let error = refresh.await.expect("task").err().expect("reject");
    assert_eq!(error, "session-title service disposed");
}

struct RequestParkingProvider {
    id: SessionTitleProviderId,
    automatic: SessionTitleAutomaticMode,
    requests: Arc<Mutex<Vec<SessionTitleProviderRequest>>>,
    gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

#[async_trait]
impl SessionTitleProvider for RequestParkingProvider {
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
        self.requests.lock().push(request.clone());
        let rx = self.gate.lock().take();
        if let Some(rx) = rx {
            let _ = rx.await;
        }
        Ok(SessionTitleProviderResult {
            title: "Ignored service abort".to_string(),
            message_seqs: vec![1],
            model: None,
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn warns_when_a_detached_session_prevents_queued_fallback_publication() {
    let (ctx, store, service) = setup().await;
    let (session, detach) = enter_session(&store, "fallback-detach").await;
    let session_for_listener = session.clone();
    let detach: Arc<Mutex<Option<cordis::Disposer>>> = Arc::new(Mutex::new(Some(detach)));
    let detach_for_listener = detach.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args: Vec<cordis::ArcValue>| {
        let subject = cordis::downcast::<Session>(&args[0]).cloned();
        let event = cordis::downcast::<SessionEvent>(&args[1]).cloned();
        let session = session_for_listener.clone();
        let detach = detach_for_listener.clone();
        Box::pin(async move {
            if let (Some(subject), Some(event)) = (subject, event) {
                if subject.ptr_eq(&session) && event.type_ == "user/message" {
                    let disposer = detach.lock().take();
                    if let Some(disposer) = disposer {
                        disposer().await;
                    }
                }
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on("session/event", listener, cordis::EventOptions::default()));
    session
        .append("turn/start", dsh_session::turn_start_data(1), None)
        .expect("turn/start");
    append_human(&session, "d1", "Detach before the fallback microtask");
    settle().await;

    assert!(service.get(&session).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn leaves_a_title_absent_when_the_byte_cap_cannot_hold_the_first_code_point() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let service = SessionTitleService::install(
        &ctx,
        Config {
            fallback_max_words: 5,
            fallback_max_bytes: 1,
            max_title_bytes: 2,
        },
    )
    .expect("install");
    let session = start_session(&store, &ctx, "no-code-point").await;
    append_human(&session, "n1", "😀");
    settle().await;
    assert!(service.get(&session).is_none());
    assert!(service.refresh(&session, None).await.expect("refresh").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_malformed_provider_results_without_replacing_the_fallback() {
    let (ctx, store, service) = setup().await;
    let result: Arc<Mutex<SessionTitleProviderResult>> = Arc::new(Mutex::new(
        SessionTitleProviderResult {
            title: "valid".to_string(),
            message_seqs: vec![1],
            model: None,
        },
    ));
    let result_for_fn = result.clone();
    let provider = Arc::new(FnProvider {
        id: session_title_provider_id("invalid-results"),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        generate_fn: Arc::new(move |_request| Ok(result_for_fn.lock().clone())),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let session = start_session(&store, &ctx, "invalid-results").await;
    let first = append_human(&session, "i1", "First source");
    settle().await;
    let second = append_human(&session, "i2", "Second source");
    settle().await;

    let cases: Vec<(SessionTitleProviderResult, &str)> = vec![
        (
            SessionTitleProviderResult {
                title: "\u{1b}[31m".to_string(),
                message_seqs: vec![first.seq],
                model: None,
            },
            "empty title",
        ),
        (
            SessionTitleProviderResult { title: "valid".to_string(), message_seqs: vec![], model: None },
            "at least one source message",
        ),
        (
            SessionTitleProviderResult {
                title: "valid".to_string(),
                message_seqs: vec![999],
                model: None,
            },
            "unique, ordered seqs",
        ),
        (
            SessionTitleProviderResult {
                title: "valid".to_string(),
                message_seqs: vec![first.seq, first.seq],
                model: None,
            },
            "unique, ordered seqs",
        ),
        (
            SessionTitleProviderResult {
                title: "valid".to_string(),
                message_seqs: vec![second.seq, first.seq],
                model: None,
            },
            "unique, ordered seqs",
        ),
        (
            SessionTitleProviderResult {
                title: "valid".to_string(),
                message_seqs: vec![first.seq],
                model: Some(dsh_session_title::SessionTitleModelProvenance {
                    provider: String::new(),
                    model: "m".to_string(),
                }),
            },
            "provider result model",
        ),
        (
            SessionTitleProviderResult {
                title: "valid".to_string(),
                message_seqs: vec![first.seq],
                model: Some(dsh_session_title::SessionTitleModelProvenance {
                    provider: "p".to_string(),
                    model: String::new(),
                }),
            },
            "provider result model",
        ),
    ];
    for (malformed, expected) in cases {
        *result.lock() = malformed;
        let error = service.refresh(&session, None).await.err().expect("reject");
        assert!(error.contains(expected), "expected {expected:?}, got {error}");
        assert_eq!(
            service.get(&session).expect("title").source.kind(),
            "fallback"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_malformed_provider_registration_before_publishing_it() {
    let (ctx, _store, service) = setup().await;
    let empty_id = Arc::new(FnProvider {
        id: session_title_provider_id(""),
        automatic: SessionTitleAutomaticMode::FirstPrompt,
        generate_fn: Arc::new(|request| {
            Ok(SessionTitleProviderResult {
                title: "title".to_string(),
                message_seqs: request.messages.iter().map(|message| message.seq).collect(),
                model: None,
            })
        }),
    });
    let error = service.register(&ctx, empty_id).err().expect("reject");
    assert!(error.contains("id must be a non-empty string"), "{error}");
    let _ = title_event(0, "unused", serde_json::json!({"kind": "user"}));
}

#[test]
fn title_source_kinds() {
    assert_eq!(SessionTitleSource::Fallback.kind(), "fallback");
    assert_eq!(SessionTitleSource::User.kind(), "user");
    assert_eq!(
        SessionTitleSource::Provider {
            provider: session_title_provider_id("p"),
            model: None,
        }
        .kind(),
        "provider"
    );
}
