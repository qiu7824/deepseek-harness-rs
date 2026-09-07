use super::*;
use cordis::{BoxFuture, Context};
use dsh_agent::{AgentCancelCause, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionStore, SurfaceIntent, SurfaceOp, session_id};

struct FixtureAgent {
    ctx: Context,
    session: Session,
    inbox: Inbox,
    key: ScopeKey,
    options: AgentOptions,
}
impl Agent for FixtureAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn inbox(&self) -> &Inbox {
        &self.inbox
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn scope_key(&self) -> &ScopeKey {
        &self.key
    }
    fn cancel(&self, _: AgentCancelCause, _: Option<&CancelOptions>) {}
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        task: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()> {
        task()
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}
fn append(session: &Session, text: &str) {
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
async fn setup() -> (Context, Session, Arc<dyn Agent>, Arc<SessionQueryEngine>) {
    let ctx = Context::root();
    let sessions = SessionStore::install(&ctx);
    let source = sessions
        .create(&ctx, Some(session_id("source")), None)
        .await
        .unwrap();
    let target = sessions
        .create(&ctx, Some(session_id("recipient")), None)
        .await
        .unwrap();
    let inbox = Inbox::new(&target, Default::default()).unwrap();
    let agent: Arc<dyn Agent> = Arc::new(FixtureAgent {
        ctx: ctx.clone(),
        session: target,
        inbox,
        key: ScopeKey::new(),
        options: Default::default(),
    });
    let query = SessionQueryEngine::build(&ctx, &Default::default(), None).unwrap();
    (ctx, source, agent, query)
}
fn reference(source: &Session) -> Vec<SessionReferenceInput> {
    vec![SessionReferenceInput {
        session_id: source.id().clone(),
        label: Some("中文引用".into()),
    }]
}
fn reference_json(prepared: PreparedReferencedMessage) -> serde_json::Value {
    let message = prepared.additional_context.unwrap();
    let ContentBlock::Text { text } = &message.content[0] else {
        panic!("text")
    };
    serde_json::from_str(
        text.strip_prefix(PROMPT_PREFIX)
            .unwrap()
            .strip_suffix(PROMPT_SUFFIX)
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn truncated_unicode_reference_has_immutable_readable_snapshot() {
    let (ctx, source, agent, query) = setup().await;
    let root = std::env::temp_dir().join(format!("dsh-reference-中文-{}", std::process::id()));
    let store = dsh_spill_local::LocalSpillStore::install(
        &ctx,
        dsh_spill_local::Config {
            root: Some(root.to_string_lossy().into()),
        },
    )
    .unwrap();
    append(&source, "hidden_fact=钢琴最终速度是123");
    for i in 0..10_000 {
        append(&source, &format!("{i}: {}", "长会话<背景>".repeat(8)));
    }
    let resolver = SessionReferenceResolver::build(
        query.clone(),
        &Config {
            max_reference_bytes: Some(4096),
            ..Default::default()
        },
    )
    .unwrap();
    let start = std::time::Instant::now();
    let value = reference_json(
        resolver
            .prepare(&agent, &[], &reference(&source), None)
            .await
            .unwrap(),
    );
    eprintln!(
        "10k reference preparation: {}ms",
        start.elapsed().as_millis()
    );
    assert_eq!(value[0]["fullSnapshot"]["status"], "saved");
    assert!(value[0]["retention"]["omitted_messages"].as_u64().unwrap() > 0);
    let locator = value[0]["fullSnapshot"]["locator"].as_str().unwrap();
    assert!(std::path::Path::new(locator).starts_with(store.root()));
    let full = std::fs::read_to_string(locator).unwrap();
    assert!(full.contains("钢琴最终速度是123"));
    let full_value: serde_json::Value = serde_json::from_str(&full).unwrap();
    assert_eq!(
        full_value["capturedThroughSeq"],
        value[0]["capturedThroughSeq"]
    );
    append(&source, "a later message must not enter the saved snapshot");
    assert_eq!(std::fs::read_to_string(locator).unwrap(), full);
    assert!(!full.contains("a later message"));
    let _ = std::fs::remove_file(locator);
    if let Some(parent) = std::path::Path::new(locator).parent() {
        let _ = std::fs::remove_dir(parent);
    }
    let _ = std::fs::remove_dir(root);
}

#[tokio::test]
async fn unavailable_storage_and_small_references_have_honest_metadata() {
    let (_ctx, source, agent, query) = setup().await;
    append(&source, &"超长行".repeat(10_000));
    let resolver = SessionReferenceResolver::build(
        query,
        &Config {
            max_reference_bytes: Some(4096),
            ..Default::default()
        },
    )
    .unwrap();
    let value = reference_json(
        resolver
            .prepare(&agent, &[], &reference(&source), None)
            .await
            .unwrap(),
    );
    assert_eq!(value[0]["fullSnapshot"]["status"], "unavailable");
    assert!(value[0]["fullSnapshot"].get("locator").is_none());
    let snapshot = resolver.query.read_surface(source.id()).await.unwrap();
    assert_eq!(
        full_projection_text(&snapshot, "long", 1024).unwrap_err(),
        "snapshot exceeds storage limit"
    );
    let (_ctx, short, agent, query) = setup().await;
    append(&short, "short context");
    let resolver = SessionReferenceResolver::build(query, &Default::default()).unwrap();
    let value = reference_json(
        resolver
            .prepare(&agent, &[], &reference(&short), None)
            .await
            .unwrap(),
    );
    assert_eq!(value[0]["fullSnapshot"]["status"], "not-needed");
}

#[test]
fn request_budget_tracks_context_capacity_and_current_input() {
    assert!(
        reference_request_budget(Some(128_000), 0) < reference_request_budget(Some(1_000_000), 0)
    );
    assert_eq!(
        reference_request_budget(None, 0),
        DEFAULT_MAX_REFERENCE_BYTES
    );
    assert!(
        reference_request_budget(Some(128_000), 10_000)
            < reference_request_budget(Some(128_000), 0)
    );
}
