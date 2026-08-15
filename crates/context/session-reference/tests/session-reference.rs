//! Rust port of the core `session-reference.spec.ts` behaviors: URI
//! canonicalization, mention parsing, tag-safe serialization, byte-budgeted
//! retention, and the prepare flow through a real session-query engine.

use std::sync::Arc;

use cordis::Context;
use dsh_session::{Session, SessionStore, SurfaceIntent, SurfaceOp, session_id};
use dsh_session_query::{SessionQueryEngine, SessionSurfaceSnapshot, SurfaceEvent};
use dsh_session_reference::{
    DEFAULT_CANDIDATE_LIMIT, DEFAULT_MAX_REFERENCE_BYTES, MAX_REFERENCES, Config,
    SessionReferenceInput, SessionReferenceResolver, decode_session_reference_uri,
    encode_session_reference_uri, format_session_reference_mention, parse_session_reference_text,
    retain_referenced_session, stringify_tag_safe_json,
};

fn user_message(session: &Session, text: &str) {
    let message = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: text.to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    session
        .append(
            "user/message",
            serde_json::to_value(&message).expect("message"),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("user/message");
}

#[test]
fn session_uris_encode_decode_and_reject_noncanonical_forms() {
    let id = session_id("hello world/裸");
    let uri = encode_session_reference_uri(&id);
    assert!(uri.starts_with("dsh-session:"));
    assert_eq!(decode_session_reference_uri(&uri).expect("decode"), id);
    // Non-canonical payloads and wrong schemes fail.
    assert!(decode_session_reference_uri("other:").is_err());
    assert!(decode_session_reference_uri("dsh-session:not base64!").is_err());
    // A re-encoded URI with padding or different casing is not canonical.
    assert!(decode_session_reference_uri(&format!("{uri}A")).is_err());
}

#[test]
fn mentions_round_trip_through_escaped_labels() {
    let id = session_id("a b");
    let reference = SessionReferenceInput {
        session_id: id.clone(),
        label: Some("la]bel\\x".to_string()),
    };
    let mention = format_session_reference_mention(&reference);
    assert!(mention.contains("la\\]bel\\\\x"), "{mention}");
    let parsed = parse_session_reference_text(&format!("see {mention} now")).expect("parse");
    assert_eq!(parsed.text, "see @la]bel\\x now");
    assert_eq!(parsed.references.len(), 1);
    assert_eq!(parsed.references[0].session_id, id);
    assert_eq!(parsed.references[0].label.as_deref(), Some("la]bel\\x"));

    // A bare URI becomes a reference labeled by its session id.
    let uri = encode_session_reference_uri(&id);
    let parsed = parse_session_reference_text(&format!("bare {uri} tail")).expect("bare");
    assert_eq!(parsed.text, "bare @a b tail");
    assert_eq!(parsed.references[0].label.as_deref(), Some("a b"));

    // A malformed mention URI fails loud.
    let broken = parse_session_reference_text("@[x](dsh-session:bad!!)");
    assert!(broken.is_err());
}

#[test]
fn tag_safe_json_escapes_literal_angles() {
    assert_eq!(
        stringify_tag_safe_json(&serde_json::json!({"a": "<script>"})),
        r#"{"a":"\u003cscript>"}"#
    );
}

fn snapshot(session: &Session) -> SessionSurfaceSnapshot {
    SessionSurfaceSnapshot {
        session: session.header().clone(),
        captured_through_seq: session.events().last().map(|event| event.seq),
        events: session
            .events()
            .iter()
            .filter(|event| event.surface_op.is_some())
            .map(|event| SurfaceEvent {
                seq: event.seq,
                event: event.clone(),
            })
            .collect(),
    }
}

#[test]
fn retention_drops_oldest_noncheckpoint_messages_and_truncates_longest() {
    let session = Session::create(session_id("budget"), None, None).expect("session");
    user_message(&session, "short one");
    user_message(&session, "a much longer second message that will need truncation eventually");
    user_message(&session, "final newest message kept");
    let snap = snapshot(&session);

    let (data, stats) = retain_referenced_session(&snap, "labeled", 150)
        .expect("fits after retention");
    assert_eq!(stats.original_messages, 3);
    assert!(stats.retained_messages < 3, "{stats:?}");
    assert!(stats.omitted_messages > 0 || stats.omitted_bytes > 0);
    assert!(stats.truncated);
    let rendered = stringify_tag_safe_json(&serde_json::to_value(&data).expect("data"));
    assert!(rendered.len() <= 150);
    assert_eq!(data.label, "labeled");
    assert_eq!(data.session_id, "budget");

    // A fixed part that cannot fit yields None (the TS undefined).
    assert!(retain_referenced_session(&snap, "labeled", 10).is_none());
}

struct ProbeAgent {
    id: dsh_session::SessionId,
    session: Session,
}

impl ProbeAgent {
    fn new(id: &str) -> Arc<Self> {
        Self::with_cwd(id, None)
    }

    fn with_cwd(id: &str, cwd: Option<&str>) -> Arc<Self> {
        let id = session_id(id);
        let header = dsh_session::SessionHeader {
            version: 0,
            id: id.clone(),
            created_at: 100,
            cwd: cwd.map(str::to_string),
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        let session = Session::create(id.clone(), None, Some(&header)).expect("session");
        Arc::new(Self { id, session })
    }
}

impl dsh_agent::Agent for ProbeAgent {
    fn id(&self) -> &dsh_session::SessionId {
        &self.id
    }

    fn options(&self) -> &dsh_agent::AgentOptions {
        static OPTIONS: std::sync::OnceLock<dsh_agent::AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(dsh_agent::AgentOptions::default)
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &dsh_agent::Inbox {
        static INBOX: std::sync::OnceLock<dsh_agent::Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            dsh_agent::Inbox::new(
                &Session::create(session_id("probe"), None, None).expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }

    fn status(&self) -> dsh_agent::AgentStatus {
        dsh_agent::AgentStatus::Running
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }

    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        static KEY: std::sync::OnceLock<dsh_scope::ScopeKey> = std::sync::OnceLock::new();
        KEY.get_or_init(dsh_scope::ScopeKey::new)
    }

    fn cancel(&self, _cause: dsh_session::AgentCancelCause, _options: Option<&dsh_agent::CancelOptions>) {}

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: dsh_session::UserMessage, _target: dsh_agent::InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}

#[tokio::test(flavor = "current_thread")]
async fn prepares_durable_reference_context_through_the_engine() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let engine = SessionQueryEngine::install(&ctx, &Default::default(), None).expect("engine");

    // The referenced session with some conversation.
    let referenced = store
        .create(
            &ctx,
            Some(session_id("referenced-session")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    user_message(&referenced, "referenced text one");
    user_message(&referenced, "referenced text two");

    let resolver = SessionReferenceResolver::install(
        &ctx,
        engine.clone(),
        &Config {
            max_references: Some(3),
            candidate_limit: Some(10),
            max_reference_bytes: Some(4096),
        },
    )
    .expect("resolver");

    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("target-agent");
    let prepared = resolver
        .prepare(
            &agent,
            &[dsh_llm::ContentBlock::Text {
                text: "original prompt".to_string(),
            }],
            &[SessionReferenceInput {
                session_id: session_id("referenced-session"),
                label: None,
            }],
            None,
        )
        .await
        .expect("prepare");
    assert_eq!(prepared.content.len(), 1);
    let context = prepared.additional_context.expect("context");
    let text = match context.content.as_slice() {
        [dsh_llm::ContentBlock::Text { text }] => text.clone(),
        _ => panic!("single text block"),
    };
    assert!(text.starts_with("## Referenced sessions"), "{text}");
    assert!(text.contains("referenced text one"), "{text}");
    assert!(text.contains("<referenced-sessions>"), "{text}");
    assert!(text.ends_with("</referenced-sessions>"), "{text}");
    let dsh_llm::MessageSource::Plugin { plugin, form, .. } = &context.source else {
        panic!("plugin source");
    };
    assert_eq!(plugin, "session-reference");
    assert_eq!(*form, Some(dsh_llm::ContextForm::Recall));

    // Self-reference is rejected.
    let outcome = resolver
        .prepare(
            &agent,
            &[],
            &[SessionReferenceInput {
                session_id: session_id("target-agent"),
                label: None,
            }],
            None,
        )
        .await;
    assert!(matches!(
        outcome,
        Err(dsh_session_reference::SessionReferenceError { code, .. })
            if code == dsh_session_reference::SessionReferenceErrorCode::SessionReferenceSelfReference
    ));

    // More references than the cap is rejected.
    let outcome = resolver
        .prepare(
            &agent,
            &[],
            &(0..4)
                .map(|index| SessionReferenceInput {
                    session_id: session_id(format!("other-{index}")),
                    label: None,
                })
                .collect::<Vec<_>>(),
            None,
        )
        .await;
    assert!(matches!(
        outcome,
        Err(dsh_session_reference::SessionReferenceError { code, .. })
            if code == dsh_session_reference::SessionReferenceErrorCode::SessionReferenceTooMany
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn lists_ranked_candidates_excluding_self() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let engine = SessionQueryEngine::install(&ctx, &Default::default(), None).expect("engine");
    let resolver = SessionReferenceResolver::install(&ctx, engine, &Default::default())
        .expect("resolver");

    let same_cwd = store
        .create(
            &ctx,
            Some(session_id("candidate-same")),
            Some(dsh_session::CreateSessionOptions {
                seed: None,
                meta: Some(dsh_session::CreateSessionMeta {
                    cwd: Some("D:\\work".to_string()),
                    ..Default::default()
                }),
            }),
        )
        .await
        .expect("create");
    user_message(&same_cwd, "hi");
    let other = store
        .create(
            &ctx,
            Some(session_id("candidate-other")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    user_message(&other, "hi");

    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::with_cwd("self-agent", Some("D:\\work"));
    let candidates = resolver
        .list_candidates(&agent, "", Some(10), None)
        .await
        .expect("candidates");
    assert_eq!(candidates.len(), 2);
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.session_id != session_id("self-agent"))
    );
    // The same-cwd candidate ranks first.
    assert_eq!(candidates[0].session_id, session_id("candidate-same"));
    assert_eq!(candidates[0].label, "candidate-same");
    assert_eq!(candidates[0].cwd.as_deref(), Some("D:\\work"));
    assert_eq!(DEFAULT_CANDIDATE_LIMIT, 50);
    assert_eq!(DEFAULT_MAX_REFERENCE_BYTES, 65_536);
    assert_eq!(MAX_REFERENCES, 3);
}
