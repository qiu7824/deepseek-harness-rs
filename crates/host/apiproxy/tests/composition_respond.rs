//! Composition-layer approval/question response loops over the real proxy
//! service. These are the answerable server-request paths behind
//! `POST /api/respond`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cordis::Context;
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
    InboxTarget,
};
use dsh_host_apiproxy::{
    AbortSignal, ApiProxyCarrier, ApiProxyDefaults, ApiProxyService, ClientResponse, FrameRequest,
    RpcReceipt, RpcReceiptReason, rpc_id,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, UserMessage, session_id};
use dsh_user_approval::{
    ApprovalOutcome, ApprovalRequest, ApprovalService, Config as ApprovalConfig,
};
use dsh_user_questions::{
    AskUserQuestionItem, AskUserQuestionOption, AskUserQuestionRequest, UserQuestionService,
};
use futures::StreamExt;

struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl StubAgent {
    fn new(ctx: &Context, id: &str) -> Arc<dyn Agent> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
            ctx: ctx.clone(),
            scope_key: ScopeKey::new(),
        })
    }
}

impl Agent for StubAgent {
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
        &self.inbox
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
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

    fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: UserMessage) {}

    fn steer(&self, _message: UserMessage) {}

    fn inject(&self, _message: UserMessage) {}
}

async fn register_agent(registry: &AgentRegistry, agent: &Arc<dyn Agent>) {
    registry.register(&registry.ctx, agent.clone());
    let id = agent.id().clone();
    for _ in 0..10_000 {
        if registry.get(&id).is_some() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("agent never became live");
}

struct Harness {
    _ctx: Context,
    questions: Arc<UserQuestionService>,
    approval: Arc<ApprovalService>,
    proxy: Arc<ApiProxyService>,
    agent: Arc<dyn Agent>,
}

impl Harness {
    async fn new(id: &str) -> Self {
        let ctx = Context::root();
        let agents = AgentRegistry::install(&ctx);
        let questions = UserQuestionService::install(&ctx);
        let approval = ApprovalService::install(&ctx, ApprovalConfig::default());
        let proxy = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let agent = StubAgent::new(&ctx, id);
        register_agent(&agents, &agent).await;
        Self {
            _ctx: ctx,
            questions,
            approval,
            proxy,
            agent,
        }
    }

    fn mux(&self) -> std::pin::Pin<Box<dyn futures::Stream<Item = FrameRequest> + Send>> {
        self.proxy.events_mux(
            FrameRequest {
                rpc_id: rpc_id("respond-mux"),
                payload: serde_json::json!({}),
            },
            AbortSignal::new(),
        )
    }
}

async fn next_frame(
    stream: &mut std::pin::Pin<Box<dyn futures::Stream<Item = FrameRequest> + Send>>,
) -> FrameRequest {
    tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("interaction frame timed out")
        .expect("interaction stream ended")
}

fn response(value: serde_json::Value) -> ClientResponse {
    serde_json::from_value(value).expect("client response")
}

fn question_request(agent: Arc<dyn Agent>) -> AskUserQuestionRequest {
    AskUserQuestionRequest {
        questions: vec![AskUserQuestionItem {
            id: "target".to_string(),
            question: "Choose a target".to_string(),
            detail: None,
            header: None,
            options: Some(vec![
                AskUserQuestionOption {
                    label: "Code".to_string(),
                    description: None,
                },
                AskUserQuestionOption {
                    label: "Docs".to_string(),
                    description: None,
                },
            ]),
            multi_select: Some(false),
            intent: None,
        }],
        agent: Some(agent),
        signal: None,
    }
}

fn question_answer(
    rpc_id: &dsh_host_apiproxy::RpcId,
    session_id: &str,
    selected: serde_json::Value,
    custom: Option<&str>,
) -> ClientResponse {
    let mut answer = serde_json::json!({ "id": "target", "selected": selected });
    if let Some(custom) = custom {
        answer["custom"] = serde_json::json!(custom);
    }
    response(serde_json::json!({
        "type": "client-response",
        "rpcId": rpc_id,
        "result": {
            "ok": true,
            "value": {
                "sessionId": session_id,
                "answer": { "answers": [answer] }
            }
        }
    }))
}

fn approval_answer(
    rpc_id: &dsh_host_apiproxy::RpcId,
    session_id: &str,
    approval_id: serde_json::Value,
    outcome: &str,
) -> ClientResponse {
    response(serde_json::json!({
        "type": "client-response",
        "rpcId": rpc_id,
        "result": {
            "ok": true,
            "value": {
                "sessionId": session_id,
                "approvalId": approval_id,
                "outcome": outcome
            }
        }
    }))
}

#[tokio::test(flavor = "current_thread")]
async fn question_request_responds_once_and_broadcasts_resolution() {
    let harness = Harness::new("question-owner").await;
    let mut mux = harness.mux();
    let questions = Arc::clone(&harness.questions);
    let agent = Arc::clone(&harness.agent);
    let asked = tokio::spawn(async move {
        questions
            .ask(&AskUserQuestionRequest {
                questions: vec![AskUserQuestionItem {
                    id: "target".to_string(),
                    question: "Choose a target".to_string(),
                    detail: None,
                    header: None,
                    options: Some(vec![AskUserQuestionOption {
                        label: "Code".to_string(),
                        description: None,
                    }]),
                    multi_select: Some(false),
                    intent: None,
                }],
                agent: Some(agent),
                signal: None,
            })
            .await
    });

    let requested = next_frame(&mut mux).await;
    assert_eq!(requested.payload["type"], "question/requested");
    let receipt = harness
        .proxy
        .respond(response(serde_json::json!({
            "type": "client-response",
            "rpcId": requested.rpc_id,
            "result": {
                "ok": true,
                "value": {
                    "sessionId": "question-owner",
                    "answer": { "answers": [{ "id": "target", "selected": ["Code"] }] }
                }
            }
        })))
        .await;
    assert_eq!(
        receipt,
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True
        }
    );
    let answer = asked
        .await
        .expect("question task")
        .expect("question answer");
    assert_eq!(answer.answers[0].selected, vec!["Code"]);

    let resolved = next_frame(&mut mux).await;
    assert_eq!(resolved.payload["type"], "question/resolved");
    assert_eq!(resolved.payload["outcome"], "answered");
    assert_eq!(
        harness
            .proxy
            .respond(response(serde_json::json!({
                "type": "client-response",
                "rpcId": requested.rpc_id,
                "result": {
                    "ok": true,
                    "value": {
                        "sessionId": "question-owner",
                        "answer": { "answers": [{ "id": "target", "selected": ["Code"] }] }
                    }
                }
            })))
            .await,
        RpcReceipt::Rejected {
            accepted: dsh_host_apiproxy::False,
            reason: RpcReceiptReason::NotPending,
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_a_mux_removes_its_session_event_listener() {
    let harness = Harness::new("mux-lifetime").await;
    let listener_count = || {
        harness
            ._ctx
            .events
            .collect(
                cordis::DispatchMode::Emit,
                Some(&harness._ctx),
                "session/event",
                &[],
            )
            .len()
    };
    let baseline = listener_count();
    let mux = harness.mux();
    for _ in 0..100 {
        if listener_count() == baseline + 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(listener_count(), baseline + 1);

    drop(mux);
    for _ in 0..100 {
        if listener_count() == baseline {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        listener_count(),
        baseline,
        "a disconnected mux must not retain a global session-event listener"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn aborting_an_idle_mux_ends_the_stream_and_releases_its_listener() {
    let harness = Harness::new("mux-abort").await;
    let listener_count = || {
        harness
            ._ctx
            .events
            .collect(
                cordis::DispatchMode::Emit,
                Some(&harness._ctx),
                "session/event",
                &[],
            )
            .len()
    };
    let baseline = listener_count();
    let signal = AbortSignal::new();
    let mut mux = harness.proxy.events_mux(
        FrameRequest {
            rpc_id: rpc_id("abort-idle-mux"),
            payload: serde_json::json!({}),
        },
        signal.clone(),
    );
    assert_eq!(listener_count(), baseline + 1);

    let pending = tokio::spawn(async move { mux.next().await });
    tokio::task::yield_now().await;
    assert!(
        !pending.is_finished(),
        "the idle mux must be waiting for a frame before abort"
    );
    signal.abort();
    let ended = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .expect("an aborted idle mux must wake promptly")
        .expect("mux task");
    assert!(ended.is_none());
    for _ in 0..100 {
        if listener_count() == baseline {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(listener_count(), baseline);
}

#[tokio::test(flavor = "current_thread")]
async fn question_request_replays_with_the_same_rpc_id_after_mux_reconnect() {
    let harness = Harness::new("question-replay").await;
    let mut first_mux = harness.mux();
    let questions = Arc::clone(&harness.questions);
    let agent = Arc::clone(&harness.agent);
    let asked = tokio::spawn(async move { questions.ask(&question_request(agent)).await });

    let first = next_frame(&mut first_mux).await;
    assert_eq!(first.payload["type"], "question/requested");
    drop(first_mux);

    let mut second_mux = harness.mux();
    let replayed = next_frame(&mut second_mux).await;
    assert_eq!(replayed.payload["type"], "question/requested");
    assert_eq!(replayed.rpc_id, first.rpc_id);
    assert_eq!(
        harness
            .proxy
            .respond(question_answer(
                &replayed.rpc_id,
                "question-replay",
                serde_json::json!(["Code"]),
                None,
            ))
            .await,
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True,
        }
    );
    assert!(asked.await.expect("question task").is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_question_answers_do_not_consume_the_pending_request() {
    let harness = Harness::new("question-validation").await;
    let mut mux = harness.mux();
    let questions = Arc::clone(&harness.questions);
    let agent = Arc::clone(&harness.agent);
    let asked = tokio::spawn(async move { questions.ask(&question_request(agent)).await });
    let requested = next_frame(&mut mux).await;
    let bad = RpcReceipt::Rejected {
        accepted: dsh_host_apiproxy::False,
        reason: RpcReceiptReason::BadResponse,
    };

    assert_eq!(
        harness
            .proxy
            .respond(question_answer(
                &requested.rpc_id,
                "question-validation",
                serde_json::json!(["Unknown"]),
                None,
            ))
            .await,
        bad
    );
    assert_eq!(
        harness
            .proxy
            .respond(question_answer(
                &requested.rpc_id,
                "question-validation",
                serde_json::json!(["Code", "Docs"]),
                None,
            ))
            .await,
        bad
    );
    assert_eq!(
        harness
            .proxy
            .respond(question_answer(
                &requested.rpc_id,
                "question-validation",
                serde_json::json!(["Code"]),
                Some("custom conflicts with a single selection"),
            ))
            .await,
        bad
    );

    assert_eq!(
        harness
            .proxy
            .respond(question_answer(
                &requested.rpc_id,
                "question-validation",
                serde_json::json!([]),
                Some("Release notes"),
            ))
            .await,
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True,
        }
    );
    let answer = asked.await.expect("question task").expect("valid answer");
    assert_eq!(answer.answers[0].custom.as_deref(), Some("Release notes"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_claimed_question_response_wins_over_an_abort_already_being_observed() {
    let harness = Harness::new("question-claim-race").await;
    let mut mux = harness.mux();
    let armed = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let signal_armed = Arc::clone(&armed);
    let signal_entered = Arc::clone(&entered);
    let signal_release = Arc::clone(&release);
    let questions = Arc::clone(&harness.questions);
    let agent = Arc::clone(&harness.agent);
    let asked = tokio::spawn(async move {
        let mut request = question_request(agent);
        request.signal = Some(Arc::new(move || {
            if !signal_armed.load(Ordering::SeqCst) {
                return false;
            }
            signal_entered.wait();
            signal_release.wait();
            true
        }));
        questions.ask(&request).await
    });
    let requested = next_frame(&mut mux).await;

    let proxy = Arc::clone(&harness.proxy);
    let rpc_id = requested.rpc_id.clone();
    let responder = std::thread::spawn(move || {
        entered.wait();
        let receipt = futures::executor::block_on(proxy.respond(question_answer(
            &rpc_id,
            "question-claim-race",
            serde_json::json!(["Code"]),
            None,
        )));
        release.wait();
        receipt
    });
    armed.store(true, Ordering::SeqCst);

    let answer = asked
        .await
        .expect("question task")
        .expect("an accepted response must beat the abort observer it already claimed past");
    assert_eq!(answer.answers[0].selected, vec!["Code"]);
    assert_eq!(
        responder.join().expect("responder thread"),
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True,
        }
    );
    let resolved = next_frame(&mut mux).await;
    assert_eq!(resolved.payload["outcome"], "answered");
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_question_removes_it_and_rejects_a_late_response() {
    let harness = Harness::new("question-cancel").await;
    let mut mux = harness.mux();
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&cancelled);
    let questions = Arc::clone(&harness.questions);
    let agent = Arc::clone(&harness.agent);
    let asked = tokio::spawn(async move {
        let mut request = question_request(agent);
        request.signal = Some(Arc::new(move || signal.load(Ordering::SeqCst)));
        questions.ask(&request).await
    });
    let requested = next_frame(&mut mux).await;

    cancelled.store(true, Ordering::SeqCst);
    let resolved = next_frame(&mut mux).await;
    assert_eq!(resolved.payload["type"], "question/resolved");
    assert_eq!(resolved.payload["outcome"], "cancelled");
    let error = asked
        .await
        .expect("question task")
        .expect_err("cancelled request must fail");
    assert_eq!(error.code, "ASK_ABORTED");
    assert_eq!(
        harness
            .proxy
            .respond(question_answer(
                &requested.rpc_id,
                "question-cancel",
                serde_json::json!(["Code"]),
                None,
            ))
            .await,
        RpcReceipt::Rejected {
            accepted: dsh_host_apiproxy::False,
            reason: RpcReceiptReason::NotPending,
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn approval_request_responds_once_and_broadcasts_resolution() {
    let harness = Harness::new("approval-owner").await;
    harness
        .agent
        .session()
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("open turn");
    let mut mux = harness.mux();
    let approval = Arc::clone(&harness.approval);
    let agent = Arc::clone(&harness.agent);
    let asked = tokio::spawn(async move {
        approval
            .request(&ApprovalRequest {
                agent,
                tool_name: "bash".to_string(),
                call_id: Some("call-1".to_string()),
                reason: Some("sandbox escalation".to_string()),
                signal: None,
            })
            .await
    });

    let requested = next_frame(&mut mux).await;
    assert_eq!(requested.payload["type"], "approval/requested");
    assert_eq!(requested.payload["callId"], "call-1");
    let approval_id = requested.payload["approvalId"].clone();
    let receipt = harness
        .proxy
        .respond(response(serde_json::json!({
            "type": "client-response",
            "rpcId": requested.rpc_id,
            "result": {
                "ok": true,
                "value": {
                    "sessionId": "approval-owner",
                    "approvalId": approval_id,
                    "outcome": "allowed-once"
                }
            }
        })))
        .await;
    assert_eq!(
        receipt,
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True
        }
    );
    assert_eq!(
        asked
            .await
            .expect("approval task")
            .expect("approval outcome"),
        ApprovalOutcome::AllowedOnce
    );

    let resolved = next_frame(&mut mux).await;
    assert_eq!(resolved.payload["type"], "approval/resolved");
    assert_eq!(resolved.payload["outcome"], "allowed-once");
}

#[tokio::test(flavor = "current_thread")]
async fn mismatched_approval_answers_do_not_consume_the_pending_request() {
    let harness = Harness::new("approval-validation").await;
    harness
        .agent
        .session()
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("open turn");
    let mut mux = harness.mux();
    let approval = Arc::clone(&harness.approval);
    let agent = Arc::clone(&harness.agent);
    let asked = tokio::spawn(async move {
        approval
            .request(&ApprovalRequest {
                agent,
                tool_name: "bash".to_string(),
                call_id: Some("validation-call".to_string()),
                reason: None,
                signal: None,
            })
            .await
    });
    let requested = next_frame(&mut mux).await;
    let approval_id = requested.payload["approvalId"].clone();
    let bad = RpcReceipt::Rejected {
        accepted: dsh_host_apiproxy::False,
        reason: RpcReceiptReason::BadResponse,
    };

    assert_eq!(
        harness
            .proxy
            .respond(approval_answer(
                &requested.rpc_id,
                "foreign-session",
                approval_id.clone(),
                "rejected",
            ))
            .await,
        bad
    );
    assert_eq!(
        harness
            .proxy
            .respond(approval_answer(
                &requested.rpc_id,
                "approval-validation",
                serde_json::json!("foreign-approval"),
                "rejected",
            ))
            .await,
        bad
    );
    assert_eq!(
        harness
            .proxy
            .respond(approval_answer(
                &requested.rpc_id,
                "approval-validation",
                approval_id,
                "rejected",
            ))
            .await,
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True,
        }
    );
    assert_eq!(
        asked
            .await
            .expect("approval task")
            .expect("approval outcome"),
        ApprovalOutcome::Rejected
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_claimed_approval_response_wins_over_an_abort_already_being_observed() {
    let harness = Harness::new("approval-claim-race").await;
    harness
        .agent
        .session()
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("open turn");
    let mut mux = harness.mux();
    let armed = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let signal_armed = Arc::clone(&armed);
    let signal_entered = Arc::clone(&entered);
    let signal_release = Arc::clone(&release);
    let approval = Arc::clone(&harness.approval);
    let agent = Arc::clone(&harness.agent);
    let asked = tokio::spawn(async move {
        approval
            .request(&ApprovalRequest {
                agent,
                tool_name: "bash".to_string(),
                call_id: Some("claim-race-call".to_string()),
                reason: None,
                signal: Some(Arc::new(move || {
                    if !signal_armed.load(Ordering::SeqCst) {
                        return false;
                    }
                    signal_entered.wait();
                    signal_release.wait();
                    true
                })),
            })
            .await
    });
    let requested = next_frame(&mut mux).await;
    let approval_id = requested.payload["approvalId"].clone();

    let proxy = Arc::clone(&harness.proxy);
    let rpc_id = requested.rpc_id.clone();
    let responder = std::thread::spawn(move || {
        entered.wait();
        let receipt = futures::executor::block_on(proxy.respond(approval_answer(
            &rpc_id,
            "approval-claim-race",
            approval_id,
            "allowed-once",
        )));
        release.wait();
        receipt
    });
    armed.store(true, Ordering::SeqCst);

    assert_eq!(
        asked
            .await
            .expect("approval task")
            .expect("approval outcome"),
        ApprovalOutcome::AllowedOnce,
        "an accepted approval response must beat the abort observer it already claimed past"
    );
    assert_eq!(
        responder.join().expect("responder thread"),
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True,
        }
    );
    let resolved = next_frame(&mut mux).await;
    assert_eq!(resolved.payload["outcome"], "allowed-once");
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_an_approval_removes_it_and_rejects_a_late_response() {
    let harness = Harness::new("approval-cancel").await;
    harness
        .agent
        .session()
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("open turn");
    let mut mux = harness.mux();
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&cancelled);
    let approval = Arc::clone(&harness.approval);
    let agent = Arc::clone(&harness.agent);
    let asked = tokio::spawn(async move {
        approval
            .request(&ApprovalRequest {
                agent,
                tool_name: "bash".to_string(),
                call_id: Some("cancel-call".to_string()),
                reason: None,
                signal: Some(Arc::new(move || signal.load(Ordering::SeqCst))),
            })
            .await
    });
    let requested = next_frame(&mut mux).await;
    let approval_id = requested.payload["approvalId"].clone();

    cancelled.store(true, Ordering::SeqCst);
    let resolved = next_frame(&mut mux).await;
    assert_eq!(resolved.payload["type"], "approval/resolved");
    assert_eq!(resolved.payload["outcome"], "cancelled");
    assert_eq!(
        asked
            .await
            .expect("approval task")
            .expect("approval outcome"),
        ApprovalOutcome::Cancelled
    );
    assert_eq!(
        harness
            .proxy
            .respond(approval_answer(
                &requested.rpc_id,
                "approval-cancel",
                approval_id,
                "allowed-once",
            ))
            .await,
        RpcReceipt::Rejected {
            accepted: dsh_host_apiproxy::False,
            reason: RpcReceiptReason::NotPending,
        }
    );

    let mut replay = harness.mux();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), replay.next())
            .await
            .is_err(),
        "a cancelled approval must not replay on a new mux"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn parallel_approvals_claim_distinct_audit_ids_by_call_id() {
    let harness = Harness::new("approval-parallel").await;
    harness
        .agent
        .session()
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("open turn");
    let mut mux = harness.mux();
    let approval_a = Arc::clone(&harness.approval);
    let agent_a = Arc::clone(&harness.agent);
    let ask_a = tokio::spawn(async move {
        approval_a
            .request(&ApprovalRequest {
                agent: agent_a,
                tool_name: "bash".to_string(),
                call_id: Some("call-a".to_string()),
                reason: None,
                signal: None,
            })
            .await
    });
    let approval_b = Arc::clone(&harness.approval);
    let agent_b = Arc::clone(&harness.agent);
    let ask_b = tokio::spawn(async move {
        approval_b
            .request(&ApprovalRequest {
                agent: agent_b,
                tool_name: "write".to_string(),
                call_id: Some("call-b".to_string()),
                reason: None,
                signal: None,
            })
            .await
    });

    let first = next_frame(&mut mux).await;
    let second = next_frame(&mut mux).await;
    let (frame_a, frame_b) = if first.payload["callId"] == "call-a" {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(frame_a.payload["callId"], "call-a");
    assert_eq!(frame_b.payload["callId"], "call-b");
    assert_ne!(frame_a.payload["approvalId"], frame_b.payload["approvalId"]);

    assert_eq!(
        harness
            .proxy
            .respond(approval_answer(
                &frame_b.rpc_id,
                "approval-parallel",
                frame_b.payload["approvalId"].clone(),
                "rejected",
            ))
            .await,
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True,
        }
    );
    assert_eq!(
        harness
            .proxy
            .respond(approval_answer(
                &frame_a.rpc_id,
                "approval-parallel",
                frame_a.payload["approvalId"].clone(),
                "allowed-once",
            ))
            .await,
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True,
        }
    );
    assert_eq!(
        ask_a.await.expect("approval A task").expect("approval A"),
        ApprovalOutcome::AllowedOnce
    );
    assert_eq!(
        ask_b.await.expect("approval B task").expect("approval B"),
        ApprovalOutcome::Rejected
    );
}
