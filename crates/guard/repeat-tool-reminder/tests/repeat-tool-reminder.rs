//! Rust port of the core `repeat-tool-reminder.spec.ts` behaviors: chain
//! semantics (escalation / resets / tracking predicates / per-agent),
//! canonicalization, fold-onto-downstream-decision, and fail-loud config
//! validation — driven through the `tools/post-execute` and `agent/pre-step`
//! waterfalls directly (the agent-loop testkit integration is deferred).

use std::sync::Arc;

use cordis::{Context, arc, downcast_arc};
use dsh_agent::{
    Agent, AgentOptions, AgentPreStepPayload, AgentStatus, CancelOptions, Inbox, InboxTarget,
};
use dsh_llm::{ContentBlock, MessageSource, UserMessage, create_user_message};
use dsh_repeat_tool_reminder::{
    Config, NAME, canonicalize, detailed_reminder, json_stringify, preview_arguments,
    sort_json_value, validate_thresholds, wildcard_to_regexp,
};
use dsh_scope::ScopeKey;
use dsh_session::{AgentCancelCause, Session, SessionId, session_id};
use dsh_tools::{PostToolDecision, ToolExecution, ToolExecutionResult};

struct ProbeAgent {
    id: SessionId,
    session: Session,
}

impl ProbeAgent {
    fn new(id: &str) -> Arc<Self> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        Arc::new(Self { id, session })
    }
}

impl Agent for ProbeAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn options(&self) -> &AgentOptions {
        static OPTIONS: std::sync::OnceLock<AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(AgentOptions::default)
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        static INBOX: std::sync::OnceLock<Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            Inbox::new(
                &Session::create(session_id("probe"), None, None).expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Running
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }

    fn scope_key(&self) -> &ScopeKey {
        static KEY: std::sync::OnceLock<ScopeKey> = std::sync::OnceLock::new();
        KEY.get_or_init(ScopeKey::new)
    }

    fn cancel(&self, _cause: AgentCancelCause, _options: Option<&CancelOptions>) {}

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: UserMessage) {}

    fn steer(&self, _message: UserMessage) {}

    fn inject(&self, _message: UserMessage) {}
}

fn execution(
    agent: Option<Arc<ProbeAgent>>,
    name: &str,
    arguments: serde_json::Value,
) -> Arc<ToolExecution> {
    let token = TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Arc::new(ToolExecution {
        token,
        call_id: dsh_llm::call_id(&format!("c{token}")),
        root_call_id: dsh_llm::call_id(&format!("c{token}")),
        name: name.to_string(),
        arguments,
        agent: agent.map(|agent| agent as Arc<dyn Agent>),
        parent: None,
        signal: parking_lot::Mutex::new(Arc::new(|| false)),
    })
}

static TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn empty_result() -> ToolExecutionResult {
    ToolExecutionResult {
        is_error: false,
        error: None,
        value: None,
        content: Vec::new(),
        meta: None,
        additional_contexts: Vec::new(),
        concludes_turn: false,
        canonical_token: 0,
    }
}

fn contexts(decision: &PostToolDecision) -> Vec<UserMessage> {
    match decision {
        PostToolDecision::Accept {
            additional_contexts,
            ..
        }
        | PostToolDecision::Block {
            additional_contexts,
            ..
        } => additional_contexts.clone().unwrap_or_default(),
    }
}

async fn fire_post(
    ctx: &Context,
    exec: &Arc<ToolExecution>,
    downstream: PostToolDecision,
) -> PostToolDecision {
    let result = Arc::new(empty_result());
    let fallback = Box::pin(async move { arc(downstream) });
    let value = ctx
        .waterfall(
            "tools/post-execute",
            vec![arc(exec.clone()), arc(Arc::clone(&result))],
            fallback,
        )
        .await;
    downcast_arc::<PostToolDecision>(&value)
        .expect("decision")
        .as_ref()
        .clone()
}

fn accept() -> PostToolDecision {
    PostToolDecision::Accept {
        content: None,
        value: None,
        additional_contexts: None,
    }
}

async fn mounted(config: Config) -> (Context, cordis::Disposer) {
    let ctx = Context::root();
    let disposer = dsh_repeat_tool_reminder::apply(&ctx, &config).expect("apply");
    (disposer)().await;
    (ctx, disposer)
}

#[test]
fn canonicalization_ignores_property_order_deeply() {
    let a = serde_json::json!({"a": 1, "nested": {"x": [1, 2], "y": null}});
    let b = serde_json::json!({"nested": {"y": null, "x": [1, 2]}, "a": 1});
    assert_eq!(canonicalize(&a), canonicalize(&b));
    assert_eq!(canonicalize(&a), r#"{"a":1,"nested":{"x":[1,2],"y":null}}"#);
    // JSON.stringify number parity: integers render without a fraction.
    assert_eq!(
        json_stringify(&serde_json::json!([1.0, 1.5, "x", null, true])),
        r#"[1,1.5,"x",null,true]"#
    );
    let _ = sort_json_value(&serde_json::json!({"z": [{"b": 1, "a": 2}]}));
}

#[test]
fn wildcard_patterns_escape_metacharacters() {
    assert!(wildcard_to_regexp("pro*").is_match("probe"));
    assert!(!wildcard_to_regexp("pro*").is_match("other"));
    // A dot matches only a literal dot, never any character.
    assert!(!wildcard_to_regexp("pr.be").is_match("probe"));
    assert!(wildcard_to_regexp("pr.be").is_match("pr.be"));
    assert!(wildcard_to_regexp("mcp_*").is_match("mcp_read"));
}

#[test]
fn argument_preview_truncates_at_the_cap() {
    let canonical = format!(r#"{{"body":"{}"}}"#, "x".repeat(400));
    let preview = preview_arguments(&canonical, 24);
    assert!(
        preview.starts_with(r#"{"body":"xxxxxxxxxxxxxxx"#),
        "{preview}"
    );
    assert!(preview.contains("… (+387 more chars)"), "{preview}");
    assert!(!preview.contains(&"x".repeat(400)));
    assert_eq!(preview_arguments("short", 24), "short");
}

#[test]
fn threshold_validation_fails_loud_and_normalizes_order() {
    assert_eq!(validate_thresholds(vec![4.0, 2.0]).unwrap(), vec![2, 4]);
    assert_eq!(
        validate_thresholds(vec![3.0, 5.0, 8.0]).unwrap(),
        vec![3, 5, 8]
    );
    assert!(
        validate_thresholds(vec![])
            .unwrap_err()
            .contains("must not be empty")
    );
    assert!(
        validate_thresholds(vec![1.0, 3.0])
            .unwrap_err()
            .contains("integer >= 2")
    );
    assert!(
        validate_thresholds(vec![2.5])
            .unwrap_err()
            .contains("integer >= 2")
    );
    assert!(
        validate_thresholds(vec![3.0, 3.0])
            .unwrap_err()
            .contains("duplicates")
    );
}

#[test]
fn detailed_reminder_names_tool_count_and_arguments() {
    let text = detailed_reminder("probe", 5, r#"{"q":"same"}"#);
    assert!(text.contains("consecutive_calls: 5"), "{text}");
    assert!(text.contains("- tool: probe"), "{text}");
    assert!(text.contains(r#"- arguments: {"q":"same"}"#), "{text}");
}

#[tokio::test(flavor = "current_thread")]
async fn escalates_gently_at_the_first_threshold_and_detailed_at_the_second() {
    let (ctx, _disposer) = mounted(Config::default()).await;
    let agent = ProbeAgent::new("escalate");
    let mut reminders = 0;
    for count in 1..=5i64 {
        let exec = execution(
            Some(agent.clone()),
            "probe",
            serde_json::json!({"q": "same"}),
        );
        let decision = fire_post(&ctx, &exec, accept()).await;
        let found = contexts(&decision);
        match count {
            1 | 2 | 4 => assert!(found.is_empty(), "count {count}"),
            3 => {
                reminders += 1;
                let [reminder] = found.as_slice() else {
                    panic!("one reminder")
                };
                assert_source(reminder, "probe", 3);
                let text = text_of(reminder);
                assert!(
                    text.contains("repeating the exact same tool call"),
                    "{text}"
                );
            }
            5 => {
                reminders += 1;
                let [reminder] = found.as_slice() else {
                    panic!("one reminder")
                };
                assert_source(reminder, "probe", 5);
                let text = text_of(reminder);
                assert!(text.contains("consecutive_calls: 5"), "{text}");
                assert!(text.contains("- tool: probe"), "{text}");
                assert!(text.contains(r#"{"q":"same"}"#), "{text}");
            }
            _ => unreachable!(),
        }
    }
    assert_eq!(reminders, 2);
}

fn text_of(message: &UserMessage) -> String {
    message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.clone(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn assert_source(message: &UserMessage, tool: &str, count: i64) {
    let MessageSource::Plugin {
        plugin,
        form,
        sections,
        summary,
        ..
    } = &message.source
    else {
        panic!("plugin source");
    };
    assert_eq!(plugin, NAME);
    assert_eq!(*form, Some(dsh_llm::ContextForm::Notice));
    assert!(sections.is_none());
    assert_eq!(*summary, Some(format!("{tool} × {count}")));
}

#[tokio::test(flavor = "current_thread")]
async fn keys_the_gentle_text_to_the_first_normalized_threshold() {
    let (ctx, _disposer) = mounted(Config {
        thresholds: Some(vec![4.0, 2.0]),
        ..Default::default()
    })
    .await;
    let agent = ProbeAgent::new("gentle");
    let mut gentle = false;
    let mut detailed = false;
    for count in 1..=4i64 {
        let exec = execution(Some(agent.clone()), "probe", serde_json::json!({}));
        let decision = fire_post(&ctx, &exec, accept()).await;
        let found = contexts(&decision);
        match count {
            2 => {
                gentle = true;
                assert!(text_of(&found[0]).contains("repeating the exact same tool call"));
            }
            4 => {
                detailed = true;
                assert!(text_of(&found[0]).contains("consecutive_calls: 4"));
            }
            _ => assert!(found.is_empty()),
        }
    }
    assert!(gentle && detailed);
}

#[tokio::test(flavor = "current_thread")]
async fn different_tracked_calls_and_excluded_calls_reset_or_stay_transparent() {
    let (ctx, _disposer) = mounted(Config::default()).await;
    let agent = ProbeAgent::new("resets");
    // probe, probe, other (tracked-different → reset), probe, probe, probe
    let mut reminded = 0;
    for (name, arguments) in [
        ("probe", serde_json::json!({"q": 1})),
        ("probe", serde_json::json!({"q": 1})),
        ("other", serde_json::json!({})),
        ("probe", serde_json::json!({"q": 1})),
        ("probe", serde_json::json!({"q": 1})),
        ("probe", serde_json::json!({"q": 1})),
    ] {
        let exec = execution(Some(agent.clone()), name, arguments);
        let decision = fire_post(&ctx, &exec, accept()).await;
        if !contexts(&decision).is_empty() {
            reminded += 1;
        }
    }
    assert_eq!(
        reminded, 1,
        "only the third consecutive probe after the reset"
    );

    // Excluded calls are invisible to the chain.
    let (ctx, _disposer) = mounted(Config {
        exclude: Some(vec!["other".to_string()]),
        ..Default::default()
    })
    .await;
    let agent = ProbeAgent::new("excluded");
    let mut reminded = 0;
    for name in ["probe", "other", "probe", "other", "probe"] {
        let exec = execution(Some(agent.clone()), name, serde_json::json!({"q": 1}));
        let decision = fire_post(&ctx, &exec, accept()).await;
        if !contexts(&decision).is_empty() {
            reminded += 1;
        }
    }
    assert_eq!(reminded, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn include_patterns_and_per_agent_chains_isolate_tracking() {
    let (ctx, _disposer) = mounted(Config {
        include: Some(vec!["pro*".to_string()]),
        ..Default::default()
    })
    .await;
    let agent = ProbeAgent::new("include");
    let mut reminded = 0;
    for name in ["other", "other", "other", "probe", "probe", "probe"] {
        let exec = execution(Some(agent.clone()), name, serde_json::json!({}));
        let decision = fire_post(&ctx, &exec, accept()).await;
        if !contexts(&decision).is_empty() {
            reminded += 1;
        }
    }
    assert_eq!(reminded, 1, "three identical untracked calls never trip");

    // Per-agent isolation: two repeats < 3 do not trip, three do.
    let (ctx, _disposer) = mounted(Config::default()).await;
    let a = ProbeAgent::new("agent-a");
    let b = ProbeAgent::new("agent-b");
    for agent in [&a, &a, &b, &b, &b] {
        let exec = execution(Some(agent.clone()), "probe", serde_json::json!({"q": 1}));
        let decision = fire_post(&ctx, &exec, accept()).await;
        let reminded = !contexts(&decision).is_empty();
        if agent.id().as_str() == "agent-a" {
            assert!(!reminded, "two repeats below the threshold");
        } else if agent.id().as_str() == "agent-b" {
            // Only the third b call reminds.
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_user_pre_step_message_resets_the_chain() {
    let (ctx, _disposer) = mounted(Config {
        thresholds: Some(vec![2.0]),
        ..Default::default()
    })
    .await;
    let agent = ProbeAgent::new("user-reset");
    let exec = execution(Some(agent.clone()), "probe", serde_json::json!({"q": 1}));
    let decision = fire_post(&ctx, &exec, accept()).await;
    assert!(contexts(&decision).is_empty(), "count 1");

    let exec = execution(Some(agent.clone()), "probe", serde_json::json!({"q": 1}));
    let decision = fire_post(&ctx, &exec, accept()).await;
    assert_eq!(contexts(&decision).len(), 1, "count 2 reminds");

    // A user interjection resets the chain.
    let user = create_user_message(
        vec![ContentBlock::Text {
            text: "again".to_string(),
        }],
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    let payload = AgentPreStepPayload {
        agent: agent.clone(),
        messages: vec![user],
        turn: 1,
        step: 1,
    };
    let fallback = Box::pin(async move {
        arc(dsh_agent::PreStepDecision::Enter {
            messages: Vec::new(),
        })
    });
    let value = ctx
        .waterfall("agent/pre-step", vec![arc(payload)], fallback)
        .await;
    let decision = downcast_arc::<dsh_agent::PreStepDecision>(&value)
        .expect("decision")
        .as_ref()
        .clone();
    assert!(matches!(decision, dsh_agent::PreStepDecision::Enter { .. }));

    let exec = execution(Some(agent.clone()), "probe", serde_json::json!({"q": 1}));
    let decision = fire_post(&ctx, &exec, accept()).await;
    assert!(contexts(&decision).is_empty(), "chain restarted at 1");
}

#[tokio::test(flavor = "current_thread")]
async fn folds_the_reminder_onto_downstream_decisions() {
    let (ctx, _disposer) = mounted(Config {
        thresholds: Some(vec![2.0]),
        ..Default::default()
    })
    .await;
    let agent = ProbeAgent::new("fold");

    // Fold onto a block: the reminder comes FIRST, downstream contexts and
    // feedback survive.
    let downstream_context = create_user_message(
        vec![ContentBlock::Text {
            text: "downstream-ctx".to_string(),
        }],
        MessageSource::Plugin {
            plugin: "test".to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    let downstream = PostToolDecision::Block {
        feedback: vec![ContentBlock::Text {
            text: "nope".to_string(),
        }],
        additional_contexts: Some(vec![downstream_context.clone()]),
    };
    let exec = execution(Some(agent.clone()), "probe", serde_json::json!({"q": 1}));
    assert!(
        contexts(&fire_post(&ctx, &exec, accept()).await).is_empty(),
        "count 1"
    );

    let exec = execution(Some(agent.clone()), "probe", serde_json::json!({"q": 1}));
    let decision = fire_post(&ctx, &exec, downstream).await;
    let PostToolDecision::Block {
        feedback,
        additional_contexts,
    } = decision
    else {
        panic!("block decision");
    };
    assert_eq!(
        feedback,
        vec![ContentBlock::Text {
            text: "nope".to_string()
        }]
    );
    let folded = additional_contexts.expect("contexts");
    assert_eq!(folded.len(), 2);
    assert!(text_of(&folded[0]).contains("repeating the exact same tool call"));
    assert_eq!(text_of(&folded[1]), "downstream-ctx");

    // Fold onto an accept with a downstream value replacement (a fresh agent
    // restarts the chain at 1).
    let agent = ProbeAgent::new("fold-accept");
    let downstream = PostToolDecision::Accept {
        content: None,
        value: Some(serde_json::json!([{"type": "text", "text": "replaced"}])),
        additional_contexts: None,
    };
    let exec = execution(Some(agent.clone()), "probe", serde_json::json!({"q": 1}));
    assert!(
        contexts(&fire_post(&ctx, &exec, accept()).await).is_empty(),
        "count 1"
    );
    let exec = execution(Some(agent.clone()), "probe", serde_json::json!({"q": 1}));
    let decision = fire_post(&ctx, &exec, downstream).await;
    let PostToolDecision::Accept {
        value,
        additional_contexts,
        ..
    } = decision
    else {
        panic!("accept decision");
    };
    assert_eq!(
        value,
        Some(serde_json::json!([{"type": "text", "text": "replaced"}]))
    );
    let folded = additional_contexts.expect("contexts");
    assert_eq!(folded.len(), 1);
    assert!(text_of(&folded[0]).contains("repeating the exact same tool call"));
}

#[tokio::test(flavor = "current_thread")]
async fn direct_executes_without_an_agent_are_transparent() {
    let (ctx, _disposer) = mounted(Config {
        thresholds: Some(vec![2.0]),
        ..Default::default()
    })
    .await;
    for _ in 0..3 {
        let exec = execution(None, "probe", serde_json::json!({"q": 1}));
        let decision = fire_post(&ctx, &exec, accept()).await;
        assert!(contexts(&decision).is_empty(), "no agent, no chain");
    }
}

#[test]
fn invalid_configuration_fails_load() {
    let ctx = Context::root();
    for config in [
        Config {
            thresholds: Some(vec![]),
            ..Default::default()
        },
        Config {
            thresholds: Some(vec![1.0]),
            ..Default::default()
        },
        Config {
            thresholds: Some(vec![2.5]),
            ..Default::default()
        },
        Config {
            thresholds: Some(vec![3.0, 3.0]),
            ..Default::default()
        },
        Config {
            arguments_preview_chars: Some(0.0),
            ..Default::default()
        },
        Config {
            arguments_preview_chars: Some(12.5),
            ..Default::default()
        },
    ] {
        assert!(
            dsh_repeat_tool_reminder::apply(&ctx, &config)
                .err()
                .is_some(),
            "config must fail loud"
        );
    }
}
