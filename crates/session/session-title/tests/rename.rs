//! SessionTitleService.rename: user-source acceptance, normalization/
//! rejection boundaries, and the pin. Rust port of the core
//! `packages/session/session-title/tests/rename.spec.ts` behaviors.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::Context;
use tokio::sync::oneshot;

use dsh_session::{Session, SessionStore, session_id};
use dsh_session_title::{
    Config, RenameFailure, SessionTitleAutomaticMode, SessionTitleError, SessionTitleProvider,
    SessionTitleProviderId, SessionTitleProviderRequest, SessionTitleProviderResult,
    SessionTitleService, SessionTitleSource, fold_session_title, session_title_provider_id,
};

fn config() -> Config {
    Config {
        fallback_max_words: 5,
        fallback_max_bytes: 40,
        max_title_bytes: 40,
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

#[tokio::test(flavor = "current_thread")]
async fn appends_a_normalized_user_source_title() {
    let (_ctx, store, service) = setup().await;
    let session = start_session(&store, &_ctx, "rename-accept").await;
    append_human(&session, "r1", "Original prompt text");
    settle().await;

    let accepted = service
        .rename(&session, "  Hand\tpicked   name  ")
        .expect("rename");
    assert_eq!(accepted.title, "Hand picked name");
    assert!(accepted.message_seqs.is_empty());
    assert!(matches!(accepted.source, SessionTitleSource::User));

    let events = session.events();
    let event = events
        .iter()
        .rev()
        .find(|event| event.type_ == "session/title")
        .expect("title event");
    assert_eq!(event.data["title"], "Hand picked name");
    assert_eq!(event.data["messageSeqs"], serde_json::json!([]));
    assert_eq!(event.data["source"], serde_json::json!({"kind": "user"}));
    assert_eq!(
        fold_session_title(&events).expect("fold").source,
        SessionTitleSource::User
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_titles_that_normalize_to_empty_and_dead_sessions() {
    let (_ctx, store, service) = setup().await;
    let session = start_session(&store, &_ctx, "rename-reject").await;
    let error = service
        .rename(&session, "  \u{1b}[31m  ")
        .err()
        .expect("reject");
    assert!(error.to_string().contains("visible characters"), "{error}");
    assert!(error.is_invalid());

    let detached = dsh_session::Session::create(session_id("detached"), None, None).expect("create");
    let error = service.rename(&detached, "name").err().expect("reject");
    assert!(error.to_string().contains("not live in this store"), "{error}");
    assert!(!error.is_invalid());
}

#[tokio::test(flavor = "current_thread")]
async fn pins_the_title_later_user_messages_schedule_no_automatic_revision_refresh_unpins() {
    let (ctx, store, service) = setup().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_for_fn = calls.clone();
    let provider = Arc::new(FnProvider {
        id: session_title_provider_id("pin-provider"),
        automatic: SessionTitleAutomaticMode::AllPrompts,
        generate_fn: Arc::new(move |request| {
            calls_for_fn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(SessionTitleProviderResult {
                title: "Provider title".to_string(),
                message_seqs: request.messages.iter().map(|message| message.seq).collect(),
                model: None,
            })
        }),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let session = start_session(&store, &ctx, "rename-pin").await;
    append_human(&session, "p1", "First prompt");
    settle().await;
    service.rename(&session, "Pinned by hand").expect("rename");

    // A later eligible prompt must schedule nothing while the pin stands.
    append_human(&session, "p2", "Second prompt after the pin");
    settle().await;
    session
        .append(
            "request/header",
            serde_json::json!({
                "header": {"config": {"provider": "main-route", "model": "chat-model"}},
                "reason": "change",
            }),
            None,
        )
        .expect("append");
    settle().await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(service.get(&session).expect("title").title, "Pinned by hand");

    // Explicit refresh remains the deliberate unpin.
    let refreshed = service.refresh(&session, None).await.expect("refresh");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(refreshed.expect("title").title, "Provider title");
    assert_eq!(service.get(&session).expect("title").source.kind(), "provider");
}

#[tokio::test(flavor = "current_thread")]
async fn fallback_only_refresh_also_unpins_the_user_title_yields_to_a_re_derived_fallback() {
    let (_ctx, store, service) = setup().await;
    let session = start_session(&store, &_ctx, "rename-unpin-fallback").await;
    append_human(&session, "u1", "Derivable prompt words");
    settle().await;
    service.rename(&session, "Pinned without provider").expect("rename");
    assert_eq!(service.get(&session).expect("title").source.kind(), "user");

    let refreshed = service.refresh(&session, None).await.expect("refresh");
    let refreshed = refreshed.expect("title");
    assert_eq!(refreshed.title, "Derivable prompt words");
    assert!(matches!(refreshed.source, SessionTitleSource::Fallback));
    assert_eq!(service.get(&session).expect("title").source.kind(), "fallback");
}

#[tokio::test(flavor = "current_thread")]
async fn supersedes_in_flight_automatic_generation_a_late_provider_result_cannot_override() {
    let (ctx, store, service) = setup().await;
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let gate = Arc::new(parking_lot::Mutex::new(Some(gate_rx)));
    let observed: Arc<parking_lot::Mutex<Option<dsh_session_title::SessionTitleSignal>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let provider = Arc::new(GateProvider {
        id: session_title_provider_id("deferred-provider"),
        automatic: SessionTitleAutomaticMode::AllPrompts,
        gate: gate.clone(),
        observed: observed.clone(),
    });
    let _ = service.register(&ctx, provider).expect("register");
    let session = start_session(&store, &ctx, "rename-supersede").await;
    append_human(&session, "s1", "Prompt that triggers generation");
    session
        .append(
            "request/header",
            serde_json::json!({
                "header": {"config": {"provider": "main-route", "model": "chat-model"}},
                "reason": "change",
            }),
            None,
        )
        .expect("append");
    settle().await;
    assert!(!observed.lock().as_ref().expect("signal").is_aborted());

    service.rename(&session, "User wins").expect("rename");
    // The active call's signal aborted synchronously (supersede).
    assert!(observed.lock().as_ref().expect("signal").is_aborted());
    let _ = gate_tx.send(());
    settle().await;
    let events = session.events();
    let latest = events
        .iter()
        .rev()
        .find(|event| event.type_ == "session/title")
        .expect("title event");
    assert_eq!(latest.data["title"], "User wins");
    assert_eq!(latest.data["source"], serde_json::json!({"kind": "user"}));
}

struct GateProvider {
    id: SessionTitleProviderId,
    automatic: SessionTitleAutomaticMode,
    gate: Arc<parking_lot::Mutex<Option<oneshot::Receiver<()>>>>,
    observed: Arc<parking_lot::Mutex<Option<dsh_session_title::SessionTitleSignal>>>,
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
        Ok(SessionTitleProviderResult {
            title: "Late provider title".to_string(),
            message_seqs: request.messages.iter().map(|message| message.seq).collect(),
            model: None,
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fallback_only_refresh_keeps_the_user_title_when_no_fallback_is_derivable() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    // A 3-byte fallback cap cannot hold the 4-byte emoji prompt: the
    // re-derived fallback is empty, so the pinned title survives.
    let mut capped = config();
    capped.fallback_max_bytes = 3;
    let service = SessionTitleService::install(&ctx, capped).expect("install");
    let session = start_session(&store, &ctx, "rename-unpin-empty").await;
    append_human(&session, "e1", "😀😀");
    settle().await;
    service.rename(&session, "Sticky emoji pin").expect("rename");

    let refreshed = service.refresh(&session, None).await.expect("refresh");
    assert_eq!(refreshed.expect("title").title, "Sticky emoji pin");
    assert_eq!(service.get(&session).expect("title").source.kind(), "user");
}

#[test]
fn rename_failure_display() {
    let failure = RenameFailure::Invalid(dsh_session_title::SessionTitleInvalidError::new(
        "session title must contain visible characters",
    ));
    assert_eq!(
        failure.to_string(),
        "session title must contain visible characters"
    );
    assert!(failure.is_invalid());
}
