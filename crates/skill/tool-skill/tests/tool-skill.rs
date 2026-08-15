//! Rust port of the runtime-skill subset of `tool-skill.spec.ts`: the
//! `skill` tool schema/disposal, the durable session catalog (stable
//! digest, replacement/tombstone, proposed-catalog dedup, resume from
//! entries, compaction re-establishment, restriction/shadow gating), tool
//! error/policy paths, and the user-explicit invocation injection.
//!
//! # Deferred
//!
//! - The skill-filesystem-dependent cases (cwd project skills, body
//!   refresh) land with that provider's port.
//! - Malformed durable catalog seeds are unrepresentable in the typed
//!   `MessageSource` (documented deviation).

use std::sync::Arc;

use cordis::{Context, arc, downcast_arc};
use dsh_agent::{AgentPreStepPayload, AgentEventDispatch, PreStepDecision};
use dsh_llm::{ContentBlock, ContextForm, MessageSource, UserMessage, call_id, create_user_message};
use dsh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use dsh_session::{Session, SessionId, SurfaceIntent, SurfaceOp, session_id};
use dsh_skill::{SkillInvocationPolicy, SkillRegistration, SkillRegistry};
use dsh_tools::{ToolCallKind, ToolCallView, ToolDefinition, ToolExecutionInput, ToolOutputDefinition, ToolRuntime};
use dsh_tool_skill::{Config, apply};

// ---- helpers ----

struct ProbeAgent {
    id: SessionId,
    session: Session,
    scope_key: ScopeKey,
}

impl ProbeAgent {
    fn new(id: &str, session: Session) -> Arc<Self> {
        Arc::new(Self {
            id: session_id(id),
            session,
            scope_key: ScopeKey::new(),
        })
    }
}

impl dsh_agent::Agent for ProbeAgent {
    fn id(&self) -> &SessionId {
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

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
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

    fn inject(&self, _message: dsh_session::UserMessage) {
        panic!("step-boundary catalog must not use agent.inject()")
    }
}

fn session_with_agent(id: &str) -> (Session, Arc<dyn dsh_agent::Agent>) {
    let session = Session::create(session_id(id), None, None).expect("session");
    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new(id, session.clone());
    (session, agent)
}

fn open_message_turn(session: &Session) {
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");
    let message = create_user_message(
        vec![ContentBlock::Text {
            text: "turn 1".to_string(),
        }],
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    append_message(session, &message);
}

fn append_message(session: &Session, message: &UserMessage) {
    session
        .append(
            "user/message",
            serde_json::to_value(message).expect("serialize"),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("append");
}

fn skills_of(ctx: &Context) -> Arc<SkillRegistry> {
    ctx.get_typed::<Arc<SkillRegistry>>("skills", false)
        .map(|slot| slot.as_ref().clone())
        .expect("skills")
}

fn tools_of(ctx: &Context) -> Arc<ToolRuntime> {
    ctx.get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools")
}

async fn setup(config: Option<Config>) -> (Context, cordis::Disposer) {
    let ctx = Context::root();
    let _system_prompt =
        dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
            .expect("systemPrompt");
    let _tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let _skills = SkillRegistry::install(&ctx, dsh_skill::Config::default()).expect("skills");
    let disposer = apply(&ctx, config.unwrap_or_default()).await.expect("apply");
    (ctx, disposer)
}

fn runtime_skill(name: &str, description: &str) -> SkillRegistration {
    SkillRegistration {
        name: name.to_string(),
        description: description.to_string(),
        when_to_use: None,
        source: "runtime".to_string(),
        resource_base: None,
        content: format!("{name} body."),
        path: None,
        metadata: None,
        invocation: None,
        provider: None,
    }
}

/// Fire one `agent/pre-step` waterfall in the agent's scope and append the
/// entered messages as surface user messages (TS `fireStep`).
async fn fire_step(
    ctx: &Context,
    agent: &Arc<dyn dsh_agent::Agent>,
    messages: Vec<UserMessage>,
) -> PreStepDecision {
    let decision = propose_step(ctx, agent, messages).await;
    if let PreStepDecision::Enter { messages } = &decision {
        for message in messages {
            append_message(agent.session(), message);
        }
    }
    decision
}

/// Propose a step without appending (TS `proposeStep`). The fallback
/// echoes the claimed batch, exactly like the TS harness.
async fn propose_step(
    ctx: &Context,
    agent: &Arc<dyn dsh_agent::Agent>,
    messages: Vec<UserMessage>,
) -> PreStepDecision {
    let dispatch = AgentEventDispatch::new(ctx, agent.clone());
    let payload_messages = messages.clone();
    let fallback_messages = messages.clone();
    let decision_value = dispatch
        .waterfall(
            "agent/pre-step",
            move |agent| {
                arc(AgentPreStepPayload {
                    agent: agent.clone(),
                    messages: payload_messages,
                    turn: 1,
                    step: 1,
                })
            },
            Box::pin(async move {
                arc(PreStepDecision::Enter {
                    messages: fallback_messages,
                })
            }),
        )
        .await;
    downcast_arc::<PreStepDecision>(&decision_value)
        .expect("decision")
        .as_ref()
        .clone()
}

fn catalog_messages(session: &Session) -> Vec<UserMessage> {
    session
        .events()
        .iter()
        .filter(|event| event.type_ == "user/message")
        .filter_map(|event| serde_json::from_value::<UserMessage>(event.data.clone()).ok())
        .filter(|message| matches!(message.source, MessageSource::SkillCatalog { .. }))
        .collect()
}

fn gesture(text: &str) -> UserMessage {
    create_user_message(
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    )
}

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

// ---- skill tool ----

#[tokio::test(flavor = "current_thread")]
async fn registers_the_skill_tool_schema_and_removes_it_on_dispose() {
    let (ctx, disposer) = setup(None).await;
    skills_of(&ctx).register(
        &ctx,
        runtime_skill("lifecycle-skill", "Lifecycle"),
    );
    let tools = tools_of(&ctx);
    let names: Vec<String> = tools
        .schemas(None)
        .into_iter()
        .map(|schema| schema.name)
        .collect();
    assert_eq!(names, vec!["skill".to_string()]);

    let definition = tools.get("skill", None).expect("skill tool");
    let view = definition
        .present_call
        .as_ref()
        .expect("presentCall")
        .clone()(&serde_json::json!({ "name": "project-skill" }));
    assert_eq!(
        view,
        Some(ToolCallView::Generic {
            title: "Load skill project-skill".to_string(),
            kind: Some(ToolCallKind::Read),
            raw_input: Some(serde_json::json!("project-skill")),
            content: None,
            locations: None,
        })
    );

    let session = Session::create(session_id("schema-lifecycle"), None, None).expect("session");
    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("schema-lifecycle", session.clone());
    open_message_turn(&session);
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 1);

    (disposer)().await;
    assert!(tools.schemas(None).is_empty());
    let session = Session::create(session_id("schema-removed"), None, None).expect("session");
    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("schema-removed", session.clone());
    open_message_turn(&session);
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 0);

    let _disposer2 = apply(&ctx, Config::default()).await.expect("re-apply");
    assert_eq!(tools.schemas(None).len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn injects_a_stable_durable_catalog_at_the_first_step() {
    let (ctx, _disposer) = setup(Some(Config {
        catalog_description_max_length: Some(50),
    }))
    .await;
    let skills = skills_of(&ctx);
    let _ = skills.register(
        &ctx,
        SkillRegistration {
            description: "Long   description ".repeat(5),
            when_to_use: Some("Never render this routing hint.".to_string()),
            source: "secret-source".to_string(),
            provider: Some("runtime".to_string()),
            resource_base: Some(dsh_skill::SkillResourceBase::Directory {
                path: "/secret/path".to_string(),
            }),
            content: "Secret body.".to_string(),
            ..runtime_skill("z-skill", "z")
        },
    );
    let _ = skills.register(&ctx, runtime_skill("a-skill", "Use {{placeholder}} <safely> & carefully."));
    let _ = skills.register(
        &ctx,
        SkillRegistration {
            invocation: Some(SkillInvocationPolicy { model_invocable: true, user_invocable: false }),
            ..runtime_skill("model-only-skill", "Model-only skill.")
        },
    );
    let _ = skills.register(
        &ctx,
        SkillRegistration {
            invocation: Some(SkillInvocationPolicy { model_invocable: false, user_invocable: true }),
            ..runtime_skill("user-only-skill", "User-only skill.")
        },
    );
    // A later listener appends after the catalog (TS registration order).
    let _ = futures::executor::block_on(ctx.on(
        "agent/pre-step",
        Arc::new(|_ctx, args| {
            Box::pin(async move {
                let next = downcast_arc::<cordis::NextFn>(args.last().expect("next"))
                    .expect("next");
                let decision_value = next.call().await;
                let decision = downcast_arc::<PreStepDecision>(&decision_value)
                    .expect("decision")
                    .as_ref()
                    .clone();
                if matches!(decision, PreStepDecision::Reject) {
                    return Some(decision_value);
                }
                let PreStepDecision::Enter { mut messages } = decision else {
                    unreachable!()
                };
                messages.push(create_user_message(
                    vec![ContentBlock::Text {
                        text: "later contribution".to_string(),
                    }],
                    MessageSource::Plugin {
                        plugin: "later-contribution".to_string(),
                        form: None,
                        sections: None,
                        summary: None,
                        compaction_id: None,
                        source_command_id: None,
                    },
                ));
                Some(arc(PreStepDecision::Enter { messages }))
            })
        }),
        cordis::EventOptions::default(),
    ));

    let session = Session::create(session_id("catalog"), None, None).expect("session");
    let agent: Arc<dyn dsh_agent::Agent> = ProbeAgent::new("catalog", session.clone());
    open_message_turn(&session);
    let decision = fire_step(&ctx, &agent, Vec::new()).await;
    let PreStepDecision::Enter { messages } = decision else {
        panic!("enter expected");
    };
    assert_eq!(messages.len(), 2);
    // The catalog (registered before the later listener) appends AFTER it.
    assert!(matches!(
        messages[0].source,
        MessageSource::Plugin { ref plugin, .. } if plugin == "later-contribution"
    ));
    let MessageSource::SkillCatalog { entries, update, .. } = &messages[1].source else {
        panic!("catalog source expected");
    };
    assert!(update.is_none());
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, vec!["a-skill", "model-only-skill", "z-skill"]);
    assert_eq!(entries[2].description, "Long description Long description Long descript...");
    let text = match &messages[1].content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text block"),
    };
    assert!(text.contains("- `a-skill`: Use {{placeholder}} &lt;safely&gt; &amp; carefully."));
    assert!(!text.contains("whenToUse"));
    assert!(!text.contains("secret-source"));
    assert!(!text.contains("/secret/path"));
    assert!(!text.contains("Secret body"));
    assert!(!text.contains("user-only-skill"));
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_inject_a_catalog_when_no_model_invocable_skills_are_available() {
    let (ctx, _disposer) = setup(None).await;
    skills_of(&ctx).register(
        &ctx,
        SkillRegistration {
            invocation: Some(SkillInvocationPolicy { model_invocable: false, user_invocable: true }),
            ..runtime_skill("user-only-skill", "User-only skill")
        },
    );
    let (session, agent) = session_with_agent("empty-catalog");
    open_message_turn(&session);
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 0);
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn omits_an_incomplete_initial_catalog_and_retries_on_a_later_request_boundary() {
    let (ctx, _disposer) = setup(None).await;
    let failing = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let failing_for_provider = failing.clone();
    let invalidate: Arc<parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let invalidate_for_factory = invalidate.clone();
    struct RecoveringProvider {
        failing: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait::async_trait]
    impl dsh_skill::SkillProvider for RecoveringProvider {
        fn name(&self) -> &str {
            "recovering"
        }
        async fn list(
            &self,
            _options: &dsh_skill::SkillLookupOptions,
        ) -> Result<dsh_skill::SkillProviderObservation, String> {
            if self.failing.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("temporarily unavailable".to_string());
            }
            Ok(dsh_skill::SkillProviderObservation::default())
        }
        async fn get(
            &self,
            _candidate: &dsh_skill::SkillCandidate,
            _options: &dsh_skill::SkillLookupOptions,
        ) -> Result<Option<dsh_skill::SkillDefinition>, String> {
            Ok(None)
        }
    }
    skills_of(&ctx).register_provider(
        &ctx,
        Arc::new(move |control| {
            *invalidate_for_factory.lock() = Some(control.invalidate.clone());
            Arc::new(RecoveringProvider { failing: failing_for_provider.clone() })
        }),
    );
    let (session, agent) = session_with_agent("incomplete-prefix");
    open_message_turn(&session);
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 0);
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    (invalidate.lock().as_ref().expect("control").clone())();
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn records_an_empty_baseline_across_repeated_step_observations() {
    let (ctx, _disposer) = setup(None).await;
    let (session, agent) = session_with_agent("empty-step");
    open_message_turn(&session);
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn deduplicates_or_replaces_a_catalog_already_proposed_for_the_same_step() {
    let (ctx, _disposer) = setup(None).await;
    let skills = skills_of(&ctx);
    let dispose_first = skills.register(&ctx, runtime_skill("first-skill", "First skill"));
    let (session, agent) = session_with_agent("proposed-catalog");
    open_message_turn(&session);
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    let initial = catalog_messages(&session).remove(0);

    let duplicate = propose_step(&ctx, &agent, vec![initial.clone()]).await;
    assert_eq!(duplicate, PreStepDecision::Enter { messages: Vec::new() });

    let _ = skills.register(&ctx, runtime_skill("second-skill", "Second skill"));
    let companion = create_user_message(
        vec![ContentBlock::Text {
            text: "keep this message".to_string(),
        }],
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    let replaced = propose_step(&ctx, &agent, vec![companion.clone(), initial.clone()]).await;
    let PreStepDecision::Enter { messages } = replaced else {
        panic!("enter expected");
    };
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].id, companion.id);
    assert_ne!(messages[1].id, initial.id);
    let text = match &messages[1].content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text block"),
    };
    assert!(text.contains("second-skill"));
    (dispose_first)().await;
}

#[tokio::test(flavor = "current_thread")]
async fn removes_a_stale_proposed_catalog_before_the_first_empty_baseline() {
    let (ctx, _disposer) = setup(None).await;
    let stale = create_user_message(
        vec![ContentBlock::Text {
            text: "- `stale-skill`: Stale skill".to_string(),
        }],
        MessageSource::SkillCatalog {
            form: ContextForm::Catalog,
            update: None,
            entries: vec![dsh_llm::SkillCatalogEntry {
                name: "stale-skill".to_string(),
                description: "Stale skill".to_string(),
            }],
        },
    );
    let (_session, agent) = session_with_agent("proposed-empty-catalog");
    let decision = propose_step(&ctx, &agent, vec![stale]).await;
    assert_eq!(decision, PreStepDecision::Enter { messages: Vec::new() });
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_a_proposed_catalog_that_already_matches_the_current_snapshot() {
    let (ctx, _disposer) = setup(None).await;
    skills_of(&ctx).register(&ctx, runtime_skill("first-skill", "First skill"));
    let proposed = create_user_message(
        vec![ContentBlock::Text {
            text: "- `first-skill`: First skill".to_string(),
        }],
        MessageSource::SkillCatalog {
            form: ContextForm::Catalog,
            update: None,
            entries: vec![dsh_llm::SkillCatalogEntry {
                name: "first-skill".to_string(),
                description: "First skill".to_string(),
            }],
        },
    );
    let (_session, agent) = session_with_agent("matching-proposal");
    let decision = propose_step(&ctx, &agent, vec![proposed.clone()]).await;
    assert_eq!(decision, PreStepDecision::Enter { messages: vec![proposed] });
}

#[tokio::test(flavor = "current_thread")]
async fn injects_complete_replacement_catalogs_and_an_empty_tombstone_for_removals() {
    let (ctx, _disposer) = setup(None).await;
    let skills = skills_of(&ctx);
    let dispose_first = skills.register(&ctx, runtime_skill("first-skill", "First skill"));
    let (session, agent) = session_with_agent("dynamic-catalog");
    open_message_turn(&session);

    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    let initial_text = text_of(&catalog_messages(&session)[0]);
    assert!(initial_text.contains("first-skill"));
    assert_eq!(catalog_messages(&session).len(), 1);

    let dispose_second = skills.register(&ctx, runtime_skill("second-skill", "Second skill"));
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    let addition_text = text_of(&catalog_messages(&session)[1]);
    assert!(addition_text.contains("first-skill"));
    assert!(addition_text.contains("second-skill"));

    (dispose_second)().await;
    (dispose_first)().await;
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    let removal_text = text_of(&catalog_messages(&session)[2]);
    assert!(removal_text.contains("No skills are currently available"));
    assert!(!removal_text.contains("first-skill"));
    assert!(!removal_text.contains("second-skill"));

    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn resumes_from_the_durable_entries_of_the_latest_visible_catalog() {
    let (ctx, _disposer) = setup(None).await;
    skills_of(&ctx).register(&ctx, runtime_skill("resumed-skill", "Resumed skill"));
    let (session, agent) = session_with_agent("catalog-resume");
    open_message_turn(&session);
    // A seeded catalog whose entries differ from the live snapshot.
    append_message(
        &session,
        &create_user_message(
            vec![ContentBlock::Text {
                text: "prose a reader cannot rely on".to_string(),
            }],
            MessageSource::SkillCatalog {
                form: ContextForm::Catalog,
                update: None,
                entries: vec![dsh_llm::SkillCatalogEntry {
                    name: "old-skill".to_string(),
                    description: "Old skill".to_string(),
                }],
            },
        ),
    );
    // A foreign-sourced lookalike neither counts as published nor
    // suppresses the replacement.
    append_message(
        &session,
        &create_user_message(
            vec![ContentBlock::Text {
                text: "- `resumed-skill`: Resumed skill".to_string(),
            }],
            MessageSource::Plugin {
                plugin: "dsh-tool-skill".to_string(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        ),
    );

    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    let catalogs = catalog_messages(&session);
    assert_eq!(catalogs.len(), 2);
    let latest = &catalogs[1];
    let MessageSource::SkillCatalog { entries, update, .. } = &latest.source else {
        panic!("catalog expected");
    };
    assert_eq!(update, &Some(true));
    assert_eq!(
        entries,
        &vec![dsh_llm::SkillCatalogEntry {
            name: "resumed-skill".to_string(),
            description: "Resumed skill".to_string(),
        }]
    );
    assert!(text_of(latest).contains("resumed-skill"));

    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn re_establishes_the_current_catalog_after_compaction_hides_its_durable_message() {
    let (ctx, _disposer) = setup(None).await;
    skills_of(&ctx).register(&ctx, runtime_skill("first-skill", "First skill"));
    let (session, agent) = session_with_agent("catalog-compaction");
    open_message_turn(&session);
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert!(text_of(&catalog_messages(&session)[0]).contains("first-skill"));
    let initial_seq = session
        .events()
        .iter()
        .find(|event| {
            serde_json::from_value::<UserMessage>(event.data.clone())
                .is_ok_and(|m| matches!(m.source, MessageSource::SkillCatalog { .. }))
        })
        .expect("catalog event")
        .seq;
    let compact = create_user_message(
        vec![ContentBlock::Text {
            text: "compacted history".to_string(),
        }],
        MessageSource::Plugin {
            plugin: "compact".to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    session
        .append(
            "user/message",
            serde_json::to_value(compact).expect("serialize"),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Replace {
                    start: initial_seq,
                    end: initial_seq,
                },
                source_event_seqs: Some(vec![initial_seq]),
            }),
        )
        .expect("append");

    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    let catalogs = catalog_messages(&session);
    assert_eq!(catalogs.len(), 2);
    assert!(text_of(&catalogs[1]).contains("first-skill"));
}

#[tokio::test(flavor = "current_thread")]
async fn returns_is_error_for_unknown_invalid_and_model_disabled_skills() {
    let (ctx, _disposer) = setup(None).await;
    skills_of(&ctx).register(
        &ctx,
        SkillRegistration {
            invocation: Some(SkillInvocationPolicy { model_invocable: true, user_invocable: false }),
            ..runtime_skill("model-only-skill", "Model-only skill")
        },
    );
    let tools = tools_of(&ctx);
    let (session, agent) = session_with_agent("tool-errors");

    let unknown = tools
        .execute(ToolExecutionInput {
            call_id: call_id("c1"),
            root_call_id: None,
            name: "skill".to_string(),
            arguments: serde_json::json!({ "name": "missing" }),
            agent: Some(agent.clone()),
            parent: None,
            signal: never_abort(),
        })
        .await;
    let invalid = tools
        .execute(ToolExecutionInput {
            call_id: call_id("c2"),
            root_call_id: None,
            name: "skill".to_string(),
            arguments: serde_json::json!({ "name": "Bad_Name" }),
            agent: Some(agent.clone()),
            parent: None,
            signal: never_abort(),
        })
        .await;
    let model_only = tools
        .execute(ToolExecutionInput {
            call_id: call_id("c4"),
            root_call_id: None,
            name: "skill".to_string(),
            arguments: serde_json::json!({ "name": "model-only-skill" }),
            agent: Some(agent.clone()),
            parent: None,
            signal: never_abort(),
        })
        .await;

    assert!(unknown.is_error);
    assert!(invalid.is_error);
    assert!(!model_only.is_error);
    assert_eq!(model_only.value.as_ref().expect("value")["content"], "model-only-skill body.");
    let unknown_text = text_block(&unknown.content[0]);
    assert!(unknown_text.contains("skill \"missing\" is unknown or no longer available"), "{unknown_text}");
    let _ = session;
}

#[tokio::test(flavor = "current_thread")]
async fn checks_model_policy_before_provider_loading_and_rechecks_the_loaded_definition() {
    let (ctx, _disposer) = setup(None).await;
    let get_calls: Arc<parking_lot::Mutex<Vec<String>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let get_calls_for_provider = get_calls.clone();
    struct PolicyProbeProvider {
        calls: Arc<parking_lot::Mutex<Vec<String>>>,
    }
    #[async_trait::async_trait]
    impl dsh_skill::SkillProvider for PolicyProbeProvider {
        fn name(&self) -> &str {
            "policy-probe"
        }
        async fn list(
            &self,
            _options: &dsh_skill::SkillLookupOptions,
        ) -> Result<dsh_skill::SkillProviderObservation, String> {
            let candidate = |name: &str, invocation: SkillInvocationPolicy| {
                dsh_skill::SkillCandidate {
                    name: name.to_string(),
                    description: format!("{name} description"),
                    when_to_use: None,
                    invocation,
                    provider: "policy-probe".to_string(),
                    source: "test".to_string(),
                    resource_base: None,
                    rank: 1,
                    locator: arc(name.to_string()),
                    path: None,
                    metadata: None,
                }
            };
            Ok(dsh_skill::SkillProviderObservation {
                candidates: vec![
                    candidate(
                        "denied-skill",
                        SkillInvocationPolicy { model_invocable: false, user_invocable: true },
                    ),
                    candidate("policy-race-skill", SkillInvocationPolicy::BOTH),
                    candidate("vanishing-skill", SkillInvocationPolicy::BOTH),
                ],
                complete: true,
            })
        }
        async fn get(
            &self,
            candidate: &dsh_skill::SkillCandidate,
            _options: &dsh_skill::SkillLookupOptions,
        ) -> Result<Option<dsh_skill::SkillDefinition>, String> {
            self.calls.lock().push(candidate.name.clone());
            if candidate.name == "vanishing-skill" {
                return Ok(None);
            }
            Ok(Some(dsh_skill::SkillDefinition {
                name: candidate.name.clone(),
                description: candidate.description.clone(),
                when_to_use: None,
                invocation: SkillInvocationPolicy { model_invocable: false, user_invocable: true },
                source: candidate.source.clone(),
                provider: candidate.provider.clone(),
                resource_base: None,
                content: "Instructions must not be disclosed.".to_string(),
                path: None,
                metadata: None,
            }))
        }
    }
    skills_of(&ctx).register_provider(
        &ctx,
        Arc::new(move |_control| Arc::new(PolicyProbeProvider { calls: get_calls_for_provider.clone() })),
    );
    let tools = tools_of(&ctx);
    let (session, agent) = session_with_agent("policy-before-load");
    let input = |id: &str, name: &str| ToolExecutionInput {
        call_id: call_id(id),
        root_call_id: None,
        name: "skill".to_string(),
        arguments: serde_json::json!({ "name": name }),
        agent: Some(agent.clone()),
        parent: None,
        signal: never_abort(),
    };

    let denied = tools.execute(input("c6", "denied-skill")).await;
    let raced = tools.execute(input("c7", "policy-race-skill")).await;
    let vanished = tools.execute(input("c8", "vanishing-skill")).await;

    assert!(denied.is_error);
    assert!(raced.is_error);
    assert!(vanished.is_error);
    assert_eq!(
        *get_calls.lock(),
        vec!["policy-race-skill".to_string(), "vanishing-skill".to_string()]
    );
    for result in [&denied, &raced] {
        let text = text_block(&result.content[0]);
        assert!(text.contains("is not available for model invocation"), "{text}");
        assert!(!text.contains("Instructions must not be disclosed."));
    }
    let vanished_text = text_block(&vanished.content[0]);
    assert!(
        vanished_text.contains("skill \"vanishing-skill\" is unknown or no longer available"),
        "{vanished_text}"
    );
    let _ = session;
}

#[tokio::test(flavor = "current_thread")]
async fn renders_provider_managed_resource_hints_for_non_local_skills() {
    let (ctx, _disposer) = setup(None).await;
    let skills = skills_of(&ctx);
    let _ = skills.register(
        &ctx,
        SkillRegistration {
            resource_base: Some(dsh_skill::SkillResourceBase::Opaque {
                description: "runtime memory".to_string(),
            }),
            ..runtime_skill("opaque-skill", "Opaque skill")
        },
    );
    let _ = skills.register(
        &ctx,
        SkillRegistration {
            resource_base: Some(dsh_skill::SkillResourceBase::Url {
                url: "https://skills.example.test/url-skill".to_string(),
            }),
            ..runtime_skill("url-skill", "URL skill")
        },
    );
    let _ = skills.register(&ctx, runtime_skill("provider-skill", "Provider skill"));
    let tools = tools_of(&ctx);
    let (session, agent) = session_with_agent("resource-hints");
    let input = |id: &str, name: &str| ToolExecutionInput {
        call_id: call_id(id),
        root_call_id: None,
        name: "skill".to_string(),
        arguments: serde_json::json!({ "name": name }),
        agent: Some(agent.clone()),
        parent: None,
        signal: never_abort(),
    };

    let opaque = tools.execute(input("c2", "opaque-skill")).await;
    let url = tools.execute(input("c3", "url-skill")).await;
    let provider = tools.execute(input("c4", "provider-skill")).await;

    let opaque_text = text_block(&opaque.content[0]);
    assert!(opaque_text.contains(
        "<skill_resources>\nResources for this skill: runtime memory\nLoad referenced resources only as needed.\n</skill_resources>"
    ));
    let url_text = text_block(&url.content[0]);
    assert!(url_text.contains(
        "<skill_resources>\nBase URL for this skill: https://skills.example.test/url-skill\nResolve relative URLs mentioned by this skill against the base URL before using them. Load referenced resources only as needed.\n</skill_resources>"
    ));
    let provider_text = text_block(&provider.content[0]);
    assert!(provider_text.contains(
        "<skill_resources>\nResources for this skill are managed by provider \"runtime\".\nLoad referenced resources only as needed.\n</skill_resources>"
    ));
    let _ = session;
}

#[tokio::test(flavor = "current_thread")]
async fn validates_the_catalog_description_cap() {
    let error = apply(
        &Context::root(),
        Config {
            catalog_description_max_length: Some(2),
        },
    )
    .await
    .err()
    .expect("invalid cap");
    assert!(error.contains("greater than or equal to 3"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn omits_catalog_guidance_when_the_calling_agent_restricts_away_the_shipped_skill_tool() {
    let (ctx, _disposer) = setup(None).await;
    skills_of(&ctx).register(&ctx, runtime_skill("listed-skill", "Listed"));
    let (session, agent) = session_with_agent("restricted-catalog");
    open_message_turn(&session);
    let scope = create_scope(&ctx, agent.scope_key().clone(), &CreateScopeOptions::default());
    tools_of(&ctx)
        .restrict(
            &scope.ctx,
            dsh_tools::ToolRestriction {
                allow: None,
                deny: Some(vec!["skill".to_string()]),
            },
        )
        .expect("restrict");

    assert!(tools_of(&ctx).get("skill", Some(agent.scope_key())).is_none());
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 0);
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 0);

    let (other_session, other_agent) = session_with_agent("unrestricted");
    open_message_turn(&other_session);
    let _ = fire_step(&ctx, &other_agent, Vec::new()).await;
    assert_eq!(catalog_messages(&other_session).len(), 1);
    (scope.dispose)().await;
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_attach_shipped_catalog_guidance_to_a_scoped_same_name_tool_shadow() {
    let (ctx, _disposer) = setup(None).await;
    skills_of(&ctx).register(&ctx, runtime_skill("listed-skill", "Listed"));
    let (session, agent) = session_with_agent("shadowed-catalog");
    open_message_turn(&session);
    let scope = create_scope(&ctx, agent.scope_key().clone(), &CreateScopeOptions::default());
    let shadow = ToolDefinition {
        name: "skill".to_string(),
        description: "A scoped tool with unrelated semantics.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {},
            "required": []
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({ "type": "string" }),
            render: Arc::new(|_args, value| {
                Ok(vec![ContentBlock::Text {
                    text: value.as_str().unwrap_or_default().to_string(),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(|_args, _exec| {
            Box::pin(async move { Ok(serde_json::json!("shadow")) })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    };
    tools_of(&ctx).register(&scope.ctx, shadow).expect("shadow register");

    assert_ne!(
        Arc::as_ptr(tools_of(&ctx).get("skill", Some(agent.scope_key())).as_ref().expect("shadow")),
        Arc::as_ptr(tools_of(&ctx).get("skill", None).as_ref().expect("shipped"))
    );
    let _ = fire_step(&ctx, &agent, Vec::new()).await;
    assert_eq!(catalog_messages(&session).len(), 0);

    let (other_session, other_agent) = session_with_agent("unshadowed");
    open_message_turn(&other_session);
    let _ = fire_step(&ctx, &other_agent, Vec::new()).await;
    assert_eq!(catalog_messages(&other_session).len(), 1);
    (scope.dispose)().await;
}

// ---- user-explicit invocation injection ----

async fn invoke_harness() -> (Context, Arc<dyn dsh_agent::Agent>) {
    let (ctx, _disposer) = setup(None).await;
    let skills = skills_of(&ctx);
    let _ = skills.register(
        &ctx,
        SkillRegistration {
            content: "Say the magic word: PINEAPPLE.".to_string(),
            invocation: Some(SkillInvocationPolicy { model_invocable: false, user_invocable: true }),
            ..runtime_skill("hidden-demo", "User-only demo")
        },
    );
    let _ = skills.register(
        &ctx,
        SkillRegistration {
            content: "Shared instructions.".to_string(),
            ..runtime_skill("shared-skill", "Ordinary skill")
        },
    );
    let _ = skills.register(
        &ctx,
        SkillRegistration {
            content: "Model-only instructions.".to_string(),
            invocation: Some(SkillInvocationPolicy { model_invocable: true, user_invocable: false }),
            ..runtime_skill("model-only-skill", "Model only")
        },
    );
    let (_session, agent) = session_with_agent("invoke");
    (ctx, agent)
}

fn invoked_names(messages: &[UserMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match &message.source {
            MessageSource::SkillInvocation { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn injects_a_user_invocable_skill_named_by_a_leading_token_after_every_other_injection() {
    let (ctx, agent) = invoke_harness().await;
    let decision = propose_step(
        &ctx,
        &agent,
        vec![
            gesture("/hidden-demo what does this do"),
            gesture("plain follow-up prose"),
        ],
    )
    .await;
    let PreStepDecision::Enter { messages } = decision else {
        panic!("enter expected");
    };
    let kinds: Vec<&str> = messages.iter().map(|message| message.source.kind()).collect();
    assert_eq!(&kinds[..2], &["user", "user"]);
    assert_eq!(kinds.last(), Some(&"skill-invocation"));
    let catalog_index = kinds.iter().position(|kind| *kind == "skill-catalog").expect("catalog");
    let invocation_index = kinds.iter().position(|kind| *kind == "skill-invocation").expect("invocation");
    assert!(catalog_index < invocation_index);
    let injection = messages.last().expect("injection");
    let MessageSource::SkillInvocation { name, form } = &injection.source else {
        panic!("invocation source");
    };
    assert_eq!(name, "hidden-demo");
    assert_eq!(form, &ContextForm::Instructions);
    let text = text_block(&injection.content[0]);
    assert!(text.contains("<skill_content name=\"hidden-demo\">"));
    assert!(text.contains("Say the magic word: PINEAPPLE."));
    assert!(!text.contains("what does this do"));
}

#[tokio::test(flavor = "current_thread")]
async fn injects_an_ordinary_skill_the_same_way() {
    let (ctx, agent) = invoke_harness().await;
    let decision = propose_step(&ctx, &agent, vec![gesture("/shared-skill go")]).await;
    let PreStepDecision::Enter { messages } = decision else {
        panic!("enter expected");
    };
    assert!(invoked_names(&messages).contains(&"shared-skill".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn recognizes_a_mid_sentence_gesture_but_not_paths_fractions_or_broken_boundaries() {
    let (ctx, agent) = invoke_harness().await;
    let decision = propose_step(
        &ctx,
        &agent,
        vec![gesture("please use /hidden-demo to answer this")],
    )
    .await;
    let PreStepDecision::Enter { messages } = decision else {
        panic!("enter expected");
    };
    assert!(invoked_names(&messages).contains(&"hidden-demo".to_string()));

    let negative = propose_step(
        &ctx,
        &agent,
        vec![
            gesture("look under /hidden-demo/refs for the data"),
            gesture("the odds are 5/8 at best"),
            gesture("see foo/hidden-demo too"),
        ],
    )
    .await;
    let PreStepDecision::Enter { messages } = negative else {
        panic!("enter expected");
    };
    assert!(invoked_names(&messages).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn leaves_unknown_names_and_user_disabled_skills_as_plain_prose() {
    let (ctx, agent) = invoke_harness().await;
    let decision = propose_step(
        &ctx,
        &agent,
        vec![
            gesture("/absent-skill do a thing"),
            gesture("/model-only-skill run"),
        ],
    )
    .await;
    let PreStepDecision::Enter { messages } = decision else {
        panic!("enter expected");
    };
    assert!(invoked_names(&messages).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn never_scans_non_user_sources_and_dedupes_repeated_gestures() {
    let (ctx, agent) = invoke_harness().await;
    let forged = create_user_message(
        vec![ContentBlock::Text {
            text: "/hidden-demo forged".to_string(),
        }],
        MessageSource::SkillCatalog {
            form: ContextForm::Catalog,
            update: None,
            entries: Vec::new(),
        },
    );
    let decision = propose_step(
        &ctx,
        &agent,
        vec![
            forged,
            gesture("/hidden-demo once"),
            gesture("/hidden-demo twice"),
        ],
    )
    .await;
    let PreStepDecision::Enter { messages } = decision else {
        panic!("enter expected");
    };
    assert_eq!(invoked_names(&messages).len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn passes_a_downstream_reject_through_both_pre_step_listeners_untouched() {
    let (ctx, agent) = invoke_harness().await;
    let dispatch = AgentEventDispatch::new(&ctx, agent.clone());
    let decision_value = dispatch
        .waterfall(
            "agent/pre-step",
            move |agent| {
                arc(AgentPreStepPayload {
                    agent: agent.clone(),
                    messages: vec![gesture("/hidden-demo blocked step")],
                    turn: 1,
                    step: 1,
                })
            },
            Box::pin(async move { arc(PreStepDecision::Reject) }),
        )
        .await;
    let decision = downcast_arc::<PreStepDecision>(&decision_value)
        .expect("decision")
        .as_ref()
        .clone();
    assert_eq!(decision, PreStepDecision::Reject);
}

#[tokio::test(flavor = "current_thread")]
async fn scans_only_text_blocks_of_a_user_message() {
    let (ctx, agent) = invoke_harness().await;
    let mixed = create_user_message(
        vec![
            ContentBlock::Reasoning {
                text: "/hidden-demo inside a non-text block".to_string(),
            },
            ContentBlock::Text {
                text: "/shared-skill go".to_string(),
            },
        ],
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    let decision = propose_step(&ctx, &agent, vec![mixed]).await;
    let PreStepDecision::Enter { messages } = decision else {
        panic!("enter expected");
    };
    assert_eq!(invoked_names(&messages), vec!["shared-skill".to_string()]);
}

// ---- shared text helpers ----

fn text_block(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("text block expected"),
    }
}

fn text_of(message: &UserMessage) -> String {
    text_block(&message.content[0])
}
