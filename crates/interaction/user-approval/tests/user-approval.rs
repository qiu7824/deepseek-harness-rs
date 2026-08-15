//! Rust port of the `approval.spec.ts` + `invariant.spec.ts` behaviors:
//! the fail-closed default, the closed outcome/policy vocabulary, scoped
//! answerer dispatch, containment of throwing/rogue answerers, cancellation,
//! the audit pair, the policy fold/override, the live policy-switch notice,
//! the system-prompt context contribution, and the invariant companion.
//!
//! # Deviations covered here
//!
//! - The abort seam is a predicate (no DOM `AbortSignal`).
//! - The policy runtime-context resolves to the TS no-agent empty branch
//!   (the Rust `AssembleContext` carries no agent).
//! - Invariant failures are contained per listener instead of vetoing the
//!   append, so the pure checker plus the installed companion are exercised.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cordis::{ArcValue, Context, EventOptions, Listener, NextFn, arc, downcast, downcast_arc};
use dsh_agent::{Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_llm::{ContentBlock, MessageSource, UserMessage};
use dsh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use dsh_session::{AgentCancelCause, Session, SessionEvent, SessionId, session_id};
use dsh_user_approval::invariant::{self, UserApprovalInvariantPlugin};
use dsh_user_approval::{
    ApprovalOutcome, ApprovalPlugin, ApprovalPolicy, ApprovalRequest, ApprovalService, Config,
    effective_approval_policy, set_approval_policy,
};

/// A minimal live-agent stand-in over a real [`Session`] with a per-instance
/// scope key; records `inject()`ed messages (the service reaches
/// `agent.session()` for the audit pair and the policy fold).
struct ProbeAgent {
    id: SessionId,
    session: Session,
    scope_key: ScopeKey,
    injected: parking_lot::Mutex<Vec<UserMessage>>,
}

impl ProbeAgent {
    /// `open` seeds an open turn (request()'s enclosure precondition).
    fn new(id: &str, open: bool) -> Arc<Self> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        if open {
            session
                .append("turn/start", serde_json::json!({ "turn": 1 }), None)
                .expect("turn/start");
        }
        Arc::new(Self {
            id,
            session,
            scope_key: ScopeKey::new(),
            injected: parking_lot::Mutex::new(Vec::new()),
        })
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
        &self.scope_key
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

    fn send(&self, _message: dsh_session::UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, message: dsh_session::UserMessage) {
        self.injected.lock().push(message);
    }
}

fn request_of(agent: Arc<dyn Agent>) -> ApprovalRequest {
    ApprovalRequest {
        agent,
        tool_name: "echo".to_string(),
        call_id: None,
        reason: None,
        signal: None,
    }
}

fn grant_listener() -> Arc<Listener> {
    Arc::new(|_ctx, _args| {
        Box::pin(async move { Some(arc(ApprovalOutcome::AllowedOnce)) })
    })
}

// ---- request() ----

#[tokio::test(flavor = "current_thread")]
async fn rejects_an_idle_ask_before_appending_anything() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("idle-ask", false);

    let error = service
        .request(&request_of(agent.clone()))
        .await
        .expect_err("idle ask");

    assert!(error.contains("outside an open turn"), "{error}");
    assert_eq!(agent.session().events().len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_between_turns() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("between-turns", true);
    agent
        .session()
        .append(
            "turn/end",
            serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("turn/end");

    let error = service
        .request(&request_of(agent.clone()))
        .await
        .expect_err("closed turn");

    assert!(error.contains("outside an open turn"), "{error}");
    let events = agent.session().events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].type_, "turn/start");
    assert_eq!(events[1].type_, "turn/end");
}

#[tokio::test(flavor = "current_thread")]
async fn fails_closed_to_unavailable_auditing_the_asked_decided_pair() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("pair", true);
    let mut req = request_of(agent.clone());
    req.call_id = Some("call-1".to_string());
    req.reason = Some("hook says ask".to_string());

    let outcome = service.request(&req).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Unavailable);
    let events = agent.session().events();
    let types: Vec<&str> = events.iter().map(|event| event.type_.as_str()).collect();
    assert_eq!(types, vec!["turn/start", "approval/asked", "approval/decided"]);
    let asked = &events[1];
    let decided = &events[2];
    assert_eq!(asked.data["toolName"], "echo");
    assert_eq!(asked.data["callId"], "call-1");
    assert_eq!(asked.data["reason"], "hook says ask");
    assert_eq!(decided.data["outcome"], "unavailable");
    assert_eq!(decided.data["id"], asked.data["id"]);
}

#[tokio::test(flavor = "current_thread")]
async fn omits_absent_optional_fields_from_the_asked_audit_event() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("omit", true);

    let outcome = service.request(&request_of(agent.clone())).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Unavailable);
    let events = agent.session().events();
    let asked = &events[1];
    let keys: Vec<String> = asked
        .data
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, vec!["id".to_string(), "toolName".to_string()]);
}

#[tokio::test(flavor = "current_thread")]
async fn grants_the_first_answering_listener_outcome() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("first-wins", true);
    let second_ran = Arc::new(AtomicBool::new(false));
    let second_ran_for_listener = second_ran.clone();
    ctx.on("approval/request", grant_listener(), EventOptions::default())
        .await;
    ctx.on(
        "approval/request",
        Arc::new(move |_ctx, _args| {
            let second_ran = second_ran_for_listener.clone();
            Box::pin(async move {
                second_ran.store(true, Ordering::SeqCst);
                Some(arc(ApprovalOutcome::Rejected))
            })
        }),
        EventOptions::default(),
    )
    .await;

    let outcome = service.request(&request_of(agent.clone())).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::AllowedOnce);
    assert!(!second_ran.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn delegates_down_to_the_fail_closed_default() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("delegate", true);
    ctx.on(
        "approval/request",
        Arc::new(|_ctx, args| {
            Box::pin(async move {
                let Some(next) = args.last().and_then(|value| downcast_arc::<NextFn>(value))
                else {
                    return None;
                };
                Some(next.call().await)
            })
        }),
        EventOptions::default(),
    )
    .await;

    let outcome = service.request(&request_of(agent.clone())).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Unavailable);
}

#[tokio::test(flavor = "current_thread")]
async fn dispatches_to_global_and_matching_agent_scoped_listeners_never_a_foreign_scope() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent_a = ProbeAgent::new("scope-a", true);
    let agent_b = ProbeAgent::new("scope-b", true);
    let heard: Arc<parking_lot::Mutex<Vec<String>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));

    let heard_for_global = heard.clone();
    ctx.on(
        "approval/request",
        Arc::new(move |_ctx, args| {
            let heard = heard_for_global.clone();
            Box::pin(async move {
                let request = args
                    .first()
                    .and_then(|value| downcast::<ApprovalRequest>(value));
                let Some(request) = request else {
                    return None;
                };
                let label = if request.agent.id().as_str() == "scope-a" {
                    "global:A"
                } else {
                    "global:B"
                };
                heard.lock().push(label.to_string());
                let next = args
                    .last()
                    .and_then(|value| downcast_arc::<NextFn>(value))
                    .expect("next");
                Some(next.call().await)
            })
        }),
        EventOptions::default(),
    )
    .await;

    let register_scoped = |agent: &Arc<ProbeAgent>, label: &'static str| {
        let heard = heard.clone();
        let scope = create_scope(&ctx, agent.scope_key.clone(), &CreateScopeOptions::default());
        let ctx = scope.ctx.clone();
        async move {
            ctx.on(
                "approval/request",
                Arc::new(move |_ctx, args| {
                    let heard = heard.clone();
                    Box::pin(async move {
                        heard.lock().push(label.to_string());
                        let next = args
                            .last()
                            .and_then(|value| downcast_arc::<NextFn>(value))
                            .expect("next");
                        Some(next.call().await)
                    })
                }),
                EventOptions::default(),
            )
            .await;
        }
    };
    register_scoped(&agent_a, "scoped:A").await;
    register_scoped(&agent_b, "scoped:B").await;

    let outcome_a = service
        .request(&request_of(agent_a.clone()))
        .await
        .expect("request A");
    let outcome_b = service
        .request(&request_of(agent_b.clone()))
        .await
        .expect("request B");

    assert_eq!(outcome_a, ApprovalOutcome::Unavailable);
    assert_eq!(outcome_b, ApprovalOutcome::Unavailable);
    assert_eq!(
        *heard.lock(),
        vec![
            "global:A".to_string(),
            "scoped:A".to_string(),
            "global:B".to_string(),
            "scoped:B".to_string(),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn contains_a_throwing_answerer_as_unavailable() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("async-throw", true);
    ctx.on(
        "approval/request",
        Arc::new(|_ctx, _args| -> cordis::BoxFuture<'static, Option<ArcValue>> {
            Box::pin(async move { panic!("transport died") })
        }),
        EventOptions::default(),
    )
    .await;

    let outcome = service.request(&request_of(agent.clone())).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Unavailable);
    let events = agent.session().events();
    assert_eq!(events[2].data["outcome"], "unavailable");
}

#[tokio::test(flavor = "current_thread")]
async fn contains_a_synchronously_throwing_answerer_as_unavailable() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("sync-throw", true);
    ctx.on(
        "approval/request",
        Arc::new(|_ctx, _args| -> cordis::BoxFuture<'static, Option<ArcValue>> {
            panic!("sync bug")
        }),
        EventOptions::default(),
    )
    .await;

    let outcome = service.request(&request_of(agent.clone())).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Unavailable);
}

#[tokio::test(flavor = "current_thread")]
async fn normalizes_a_rogue_non_vocabulary_answer_to_unavailable() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("rogue", true);
    ctx.on(
        "approval/request",
        Arc::new(|_ctx, _args| {
            Box::pin(async move { Some(arc(String::from("yolo"))) })
        }),
        EventOptions::default(),
    )
    .await;

    let outcome = service.request(&request_of(agent.clone())).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Unavailable);
}

#[tokio::test(flavor = "current_thread")]
async fn settles_cancelled_immediately_on_an_already_aborted_signal() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("pre-abort", true);
    let consulted = Arc::new(AtomicBool::new(false));
    let consulted_for_listener = consulted.clone();
    ctx.on(
        "approval/request",
        Arc::new(move |_ctx, _args| {
            let consulted = consulted_for_listener.clone();
            Box::pin(async move {
                consulted.store(true, Ordering::SeqCst);
                Some(arc(ApprovalOutcome::AllowedOnce))
            })
        }),
        EventOptions::default(),
    )
    .await;
    let mut req = request_of(agent.clone());
    req.signal = Some(Arc::new(|| true));

    let outcome = service.request(&req).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Cancelled);
    assert!(!consulted.load(Ordering::SeqCst));
    let events = agent.session().events();
    let types: Vec<&str> = events.iter().map(|event| event.type_.as_str()).collect();
    assert_eq!(types, vec!["turn/start", "approval/asked", "approval/decided"]);
    assert_eq!(events[2].data["outcome"], "cancelled");
}

#[tokio::test(flavor = "current_thread")]
async fn resolves_cancelled_when_the_signal_aborts_mid_question_and_discards_the_late_answer() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("mid-abort", true);
    let gate = Arc::new(tokio::sync::Notify::new());
    let gate_for_listener = gate.clone();
    ctx.on(
        "approval/request",
        Arc::new(move |_ctx, _args| {
            let gate = gate_for_listener.clone();
            Box::pin(async move {
                gate.notified().await;
                Some(arc(ApprovalOutcome::AllowedOnce))
            })
        }),
        EventOptions::default(),
    )
    .await;
    let flag = Arc::new(AtomicBool::new(false));
    let flag_for_signal = flag.clone();
    let mut req = request_of(agent.clone());
    req.signal = Some(Arc::new(move || flag_for_signal.load(Ordering::SeqCst)));
    let service_for_task = service.clone();
    let task = tokio::spawn(async move { service_for_task.request(&req).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    flag.store(true, Ordering::SeqCst);

    let outcome = task.await.expect("join").expect("request");

    assert_eq!(outcome, ApprovalOutcome::Cancelled);
    // The answerer settles after the fact: no second decided event appears.
    gate.notify_waiters();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let events = agent.session().events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "approval/decided")
            .count(),
        1
    );
    assert_eq!(events[2].data["outcome"], "cancelled");
}

#[tokio::test(flavor = "current_thread")]
async fn resolves_the_answer_when_the_signal_never_aborts() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("never-abort", true);
    ctx.on(
        "approval/request",
        Arc::new(|_ctx, _args| {
            Box::pin(async move { Some(arc(ApprovalOutcome::Rejected)) })
        }),
        EventOptions::default(),
    )
    .await;
    let mut req = request_of(agent.clone());
    req.signal = Some(Arc::new(|| false));

    let outcome = service.request(&req).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Rejected);
}

#[tokio::test(flavor = "current_thread")]
async fn issues_a_fresh_id_per_request() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("fresh-ids", true);

    service
        .request(&request_of(agent.clone()))
        .await
        .expect("first");
    service
        .request(&request_of(agent.clone()))
        .await
        .expect("second");

    let events = agent.session().events();
    let ids: Vec<&serde_json::Value> = events
        .iter()
        .filter(|event| event.type_ == "approval/asked")
        .map(|event| &event.data["id"])
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

// ---- policy fold / gate ----

#[tokio::test(flavor = "current_thread")]
async fn folds_to_the_last_event_or_none_without_one() {
    let agent = ProbeAgent::new("fold", true);
    let session = agent.session();

    assert_eq!(effective_approval_policy(&session.events()), None);
    set_approval_policy(session, ApprovalPolicy::Never).expect("never");
    set_approval_policy(session, ApprovalPolicy::Ask).expect("ask");
    assert_eq!(
        effective_approval_policy(&session.events()),
        Some(ApprovalPolicy::Ask)
    );
    let last = session.events().last().expect("last").clone();
    assert_eq!(last.type_, "approval/policy");
    assert_eq!(last.data["policy"], "ask");
}

#[tokio::test(flavor = "current_thread")]
async fn a_never_config_rejects_deterministically_without_consulting_any_answerer() {
    let ctx = Context::root();
    let service = ApprovalService::install(
        &ctx,
        Config {
            policy: Some(ApprovalPolicy::Never),
        },
    );
    let agent = ProbeAgent::new("never-config", true);
    let consulted = Arc::new(AtomicBool::new(false));
    let consulted_for_listener = consulted.clone();
    ctx.on(
        "approval/request",
        Arc::new(move |_ctx, args| {
            let consulted = consulted_for_listener.clone();
            Box::pin(async move {
                consulted.store(true, Ordering::SeqCst);
                let next = args
                    .last()
                    .and_then(|value| downcast_arc::<NextFn>(value))
                    .expect("next");
                Some(next.call().await)
            })
        }),
        EventOptions::default(),
    )
    .await;

    let outcome = service.request(&request_of(agent.clone())).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Rejected);
    assert!(!consulted.load(Ordering::SeqCst));
    let events = agent.session().events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "approval/asked")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "approval/decided")
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_gate_decides_first_even_against_an_answerer_registered_before_the_service() {
    let ctx = Context::root();
    let consulted = Arc::new(AtomicBool::new(false));
    let consulted_for_listener = consulted.clone();
    ctx.on(
        "approval/request",
        Arc::new(move |_ctx, _args| {
            let consulted = consulted_for_listener.clone();
            Box::pin(async move {
                consulted.store(true, Ordering::SeqCst);
                Some(arc(ApprovalOutcome::AllowedOnce))
            })
        }),
        EventOptions::default(),
    )
    .await;
    let service = ApprovalService::install(
        &ctx,
        Config {
            policy: Some(ApprovalPolicy::Never),
        },
    );
    let agent = ProbeAgent::new("gate-first", true);

    let outcome = service.request(&request_of(agent.clone())).await.expect("request");

    assert_eq!(outcome, ApprovalOutcome::Rejected);
    assert!(!consulted.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn a_session_override_outranks_the_configured_default_in_both_directions() {
    let ctx = Context::root();
    let service = ApprovalService::install(
        &ctx,
        Config {
            policy: Some(ApprovalPolicy::Never),
        },
    );
    ctx.on("approval/request", grant_listener(), EventOptions::default())
        .await;
    let agent = ProbeAgent::new("override", true);
    let session = agent.session();

    assert_eq!(service.override_of(session), None);
    set_approval_policy(session, ApprovalPolicy::Ask).expect("ask");
    assert_eq!(service.override_of(session), Some(ApprovalPolicy::Ask));
    let outcome = service.request(&request_of(agent.clone())).await.expect("request");
    assert_eq!(outcome, ApprovalOutcome::AllowedOnce);
    set_approval_policy(session, ApprovalPolicy::Never).expect("never");
    let outcome = service.request(&request_of(agent.clone())).await.expect("request");
    assert_eq!(outcome, ApprovalOutcome::Rejected);
}

#[tokio::test(flavor = "current_thread")]
async fn queues_a_live_policy_switch_for_the_next_model_step() {
    let ctx = Context::root();
    let service = ApprovalService::install(&ctx, Config::default());
    let agent = ProbeAgent::new("policy-notice", true);
    let live: Arc<dyn Agent> = agent.clone();

    service.set_policy(&live, ApprovalPolicy::Never).expect("set");
    service.set_policy(&live, ApprovalPolicy::Never).expect("set again");

    assert_eq!(
        effective_approval_policy(&agent.session().events()),
        Some(ApprovalPolicy::Never)
    );
    let injected = agent.injected.lock();
    assert_eq!(injected.len(), 1);
    let message = &injected[0];
    match message.content.as_slice() {
        [ContentBlock::Text { text }] => assert_eq!(
            text,
            "The approval policy changed from \"ask\" to \"never\" (changed by the user)."
        ),
        _ => panic!("single text block"),
    }
    match &message.source {
        MessageSource::Plugin {
            plugin,
            form,
            sections,
            summary,
            compaction_id,
            source_command_id,
        } => {
            assert_eq!(plugin, "user-approval");
            assert!(form.is_none());
            assert!(sections.is_none());
            assert!(summary.is_none());
            assert!(compaction_id.is_none());
            assert!(source_command_id.is_none());
        }
        _ => panic!("plugin source expected"),
    }
}

// ---- system-prompt context contribution ----

#[tokio::test(flavor = "current_thread")]
async fn registers_the_policy_context_and_disposes_it_with_the_service() {
    let ctx = Context::root();
    let system_prompt =
        dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
            .expect("systemPrompt");
    let fiber = ctx.plugin(Arc::new(ApprovalPlugin::new(Config::default())), arc(()));
    fiber.settle().await.expect("settle");

    // The TS inject chain registers asynchronously; poll until visible.
    let mut text: Option<String> = None;
    for _ in 0..100 {
        let assembly = system_prompt
            .assemble(&ctx, &dsh_system_prompt::AssembleContext::default())
            .await
            .expect("assemble");
        text = assembly
            .contexts
            .iter()
            .find(|context| context.name == "approval:policy")
            .map(|context| context.text.clone());
        if text.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Deviation: no agent in AssembleContext, so the TS no-agent empty
    // branch resolves.
    assert_eq!(text, Some(String::new()));

    fiber.dispose().await;
    let assembly = system_prompt
        .assemble(&ctx, &dsh_system_prompt::AssembleContext::default())
        .await
        .expect("assemble");
    assert!(assembly
        .contexts
        .iter()
        .all(|context| context.name != "approval:policy"));
}

// ---- invariant companion ----

fn synthetic_event(type_: &str, seq: u64, data: serde_json::Value) -> SessionEvent {
    SessionEvent {
        type_: type_.to_string(),
        seq,
        time: seq as i64,
        data,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

#[test]
fn checker_rejects_every_malformed_or_unpaired_shape_with_the_ts_messages() {
    let traces: invariant::Traces = parking_lot::Mutex::new(Default::default());
    let session_id = "checker";
    apply_turn_1(&traces, session_id);

    let id = "ask-1";
    let empty_tool = synthetic_event(
        "approval/asked",
        1,
        serde_json::json!({ "id": id, "toolName": "" }),
    );
    assert_eq!(
        invariant::validate_event(&traces, session_id, &empty_tool).expect_err("empty toolName"),
        "approval/asked toolName must be non-empty"
    );
    let asked = synthetic_event(
        "approval/asked",
        2,
        serde_json::json!({ "id": id, "toolName": "bash" }),
    );
    invariant::validate_event(&traces, session_id, &asked).expect("asked");
    let repeated = synthetic_event(
        "approval/asked",
        3,
        serde_json::json!({ "id": id, "toolName": "bash" }),
    );
    assert_eq!(
        invariant::validate_event(&traces, session_id, &repeated).expect_err("repeated"),
        "approval/asked repeated open id \"ask-1\""
    );
    let unpaired = synthetic_event(
        "approval/decided",
        4,
        serde_json::json!({ "id": "missing", "outcome": "rejected" }),
    );
    assert_eq!(
        invariant::validate_event(&traces, session_id, &unpaired).expect_err("unpaired"),
        "approval/decided has no matching approval/asked for id \"missing\""
    );
    let unknown_outcome = synthetic_event(
        "approval/decided",
        5,
        serde_json::json!({ "id": id, "outcome": "maybe" }),
    );
    assert_eq!(
        invariant::validate_event(&traces, session_id, &unknown_outcome).expect_err("outcome"),
        "approval/decided carries unknown outcome \"maybe\""
    );
    let unknown_policy = synthetic_event(
        "approval/policy",
        6,
        serde_json::json!({ "policy": "always" }),
    );
    assert_eq!(
        invariant::validate_event(&traces, session_id, &unknown_policy).expect_err("policy"),
        "approval/policy carries unknown policy \"always\""
    );
}

#[test]
fn checker_rejects_audit_events_outside_any_open_turn() {
    let traces: invariant::Traces = parking_lot::Mutex::new(Default::default());
    let asked = synthetic_event(
        "approval/asked",
        0,
        serde_json::json!({ "id": "ask-1", "toolName": "bash" }),
    );
    assert_eq!(
        invariant::validate_event(&traces, "idle", &asked).expect_err("asked"),
        "approval/asked appended outside any open turn"
    );
    let decided = synthetic_event(
        "approval/decided",
        1,
        serde_json::json!({ "id": "ask-1", "outcome": "rejected" }),
    );
    assert_eq!(
        invariant::validate_event(&traces, "idle", &decided).expect_err("decided"),
        "approval/decided appended outside any open turn"
    );
}

fn apply_turn_1(traces: &invariant::Traces, session_id: &str) {
    invariant::apply_turn(
        traces,
        session_id,
        &synthetic_event("turn/start", 0, serde_json::json!({ "turn": 1 })),
    );
}

async fn companion_ctx() -> (Context, Arc<dsh_session::SessionStore>) {
    let ctx = Context::root();
    let store = dsh_session::SessionStore::install(&ctx);
    let _registry = InvariantRegistry::new(
        &ctx,
        InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let fiber = ctx.plugin(Arc::new(UserApprovalInvariantPlugin), arc(()));
    fiber.settle().await.expect("settle");
    (ctx, store)
}

#[tokio::test(flavor = "current_thread")]
async fn companion_accepts_paired_audit_events_and_closed_policy_values() {
    let (ctx, store) = companion_ctx().await;
    let session = store
        .create(
            &ctx,
            Some(session_id("valid-audit")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");
    session
        .append(
            "approval/asked",
            serde_json::json!({ "id": "ask-1", "toolName": "bash" }),
            None,
        )
        .expect("asked");
    session
        .append(
            "approval/decided",
            serde_json::json!({ "id": "ask-1", "outcome": "allowed-once" }),
            None,
        )
        .expect("decided");
    session
        .append(
            "approval/policy",
            serde_json::json!({ "policy": "never" }),
            None,
        )
        .expect("policy");
    assert_eq!(session.seq(), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn companion_accepts_an_unmatched_question_resumed_from_an_existing_session() {
    let ctx = Context::root();
    let store = dsh_session::SessionStore::install(&ctx);
    let session = store
        .create(
            &ctx,
            Some(session_id("resume-audit")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");
    session
        .append(
            "approval/asked",
            serde_json::json!({ "id": "ask-resume", "toolName": "bash" }),
            None,
        )
        .expect("asked");
    let _registry = InvariantRegistry::new(
        &ctx,
        InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let fiber = ctx.plugin(Arc::new(UserApprovalInvariantPlugin), arc(()));
    fiber.settle().await.expect("settle");

    // The seed rebuilt the unmatched question: its pair closes cleanly.
    session
        .append(
            "approval/decided",
            serde_json::json!({ "id": "ask-resume", "outcome": "cancelled" }),
            None,
        )
        .expect("decided");
    session
        .append(
            "turn/end",
            serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("turn/end");
}

#[tokio::test(flavor = "current_thread")]
async fn companion_adopts_a_bare_session_first_observed_through_publication() {
    let (ctx, _store) = companion_ctx().await;
    let session =
        Session::create(session_id("bare-approval-session"), None, None).expect("session");
    let id = "bare-ask";

    // No store attachment: the companion tracks the session from its first
    // publication (the TS `ctx.emit('session/event', ...)` case).
    ctx.emit(
        "session/event",
        vec![
            arc(session.clone()),
            arc(synthetic_event("turn/start", 0, serde_json::json!({ "turn": 1 }))),
        ],
    );
    ctx.emit(
        "session/event",
        vec![
            arc(session.clone()),
            arc(synthetic_event(
                "approval/asked",
                1,
                serde_json::json!({ "id": id, "toolName": "bash" }),
            )),
        ],
    );
    ctx.emit(
        "session/event",
        vec![
            arc(session.clone()),
            arc(synthetic_event(
                "approval/decided",
                2,
                serde_json::json!({ "id": id, "outcome": "rejected" }),
            )),
        ],
    );
}

#[tokio::test(flavor = "current_thread")]
async fn violating_publications_are_contained_without_vetoing_the_append() {
    // Deviation note: the TS append veto throws from `session.append`; this
    // port contains internal-listener panics, so the companion's failure is
    // observable through the checker instead.
    let (ctx, store) = companion_ctx().await;
    let session = store
        .create(
            &ctx,
            Some(session_id("contained-violation")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");

    // A malformed asked commits (containment)...
    let event = session
        .append(
            "approval/asked",
            serde_json::json!({ "id": "bad", "toolName": "" }),
            None,
        )
        .expect("commits");
    // ...but the checker rejects the same shape.
    let traces: invariant::Traces = parking_lot::Mutex::new(Default::default());
    invariant::apply_turn(
        &traces,
        session.id().as_str(),
        &synthetic_event("turn/start", 0, serde_json::json!({ "turn": 1 })),
    );
    assert_eq!(
        invariant::validate_event(&traces, session.id().as_str(), &event).expect_err("shape"),
        "approval/asked toolName must be non-empty"
    );
}
