use super::support::{
    Harness, NamedToolThenTextAdapter, harness, message, quick_tool, register_adapter,
};
use dsh_agent::{Agent, AgentFactory, AgentRegistry, InboxTarget};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, RequestAuthentication,
    RequestAuthenticationIdentity, StreamChunk,
};
use futures::StreamExt;
use parking_lot::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::Notify;

fn account(scope: &str, generation: &str) -> RequestAuthentication {
    RequestAuthentication::new(RequestAuthenticationIdentity::Account {
        auth_provider: "same-provider".into(),
        account_scope: scope.into(),
        login_generation: generation.into(),
    })
}
fn completed() -> Vec<StreamChunk> {
    vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".into(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: "Complete".into(),
            },
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
    ]
}
#[derive(Default)]
struct Gate {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    calls: AtomicUsize,
}
impl LlmAdapter for Gate {
    fn stream(&self, _: &GenerateOptions) -> ChunkStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        let release = self.release.clone();
        Box::pin(
            futures::stream::once(async move {
                release.notified().await;
                completed()
            })
            .flat_map(futures::stream::iter),
        )
    }
}
struct Routed {
    selected: Arc<Mutex<RequestAuthentication>>,
    inner: Arc<dyn LlmAdapter>,
    snapshot_gate: Option<Arc<Gate>>,
}
struct Bound {
    authentication: RequestAuthentication,
    inner: Arc<dyn LlmAdapter>,
}
impl LlmAdapter for Bound {
    fn request_authentication(&self) -> RequestAuthentication {
        self.authentication.clone()
    }
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        self.inner.stream(options)
    }
}
#[async_trait::async_trait]
impl LlmAdapter for Routed {
    async fn snapshot_for_call(
        &self,
        _: &str,
        _: &str,
        _: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> Result<Option<Arc<dyn LlmAdapter>>, dsh_llm::LlmError> {
        let authentication = self.selected.lock().clone();
        if let Some(gate) = &self.snapshot_gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        Ok(Some(Arc::new(Bound {
            authentication,
            inner: self.inner.clone(),
        })))
    }
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        self.inner.stream(options)
    }
}
async fn setup(
    auth: RequestAuthentication,
    inner: Arc<dyn LlmAdapter>,
    snapshot_gate: Option<Arc<Gate>>,
) -> (Harness, Arc<Mutex<RequestAuthentication>>) {
    let h = harness().await;
    dsh_agent_loop::AgentLoop::install(&h.ctx, Default::default()).unwrap();
    let selected = Arc::new(Mutex::new(auth));
    register_adapter(
        &h,
        Arc::new(Routed {
            selected: selected.clone(),
            inner,
            snapshot_gate,
        }),
    );
    (h, selected)
}
async fn notified(notify: &Notify) {
    tokio::time::timeout(Duration::from_secs(2), notify.notified())
        .await
        .unwrap();
}
async fn idle(agent: &dyn Agent) {
    tokio::time::timeout(Duration::from_secs(2), agent.when_idle())
        .await
        .unwrap();
}
fn signed_out(agent: &dyn Agent) {
    let events = agent.session().events();
    let ending = events
        .iter()
        .rev()
        .find(|event| event.type_ == "turn/end")
        .unwrap();
    assert_eq!(
        ending.data["reason"]["reason"]["reason"],
        dsh_agent::ACCOUNT_SIGNED_OUT_REASON
    );
    assert!(agent.authentication_binding().is_none());
}

#[tokio::test]
async fn logout_only_cancels_the_captured_account_turn_and_keeps_pending_input() {
    let a = account("a", "login-a");
    let gate = Arc::new(Gate::default());
    let (h, _) = setup(a.clone(), gate.clone(), None).await;
    h.agent.followup(message("first"));
    notified(&gate.entered).await;
    let binding = h.agent.authentication_binding().unwrap();
    for identity in [
        account("b", "login-b").identity().clone(),
        account("a", "new-login-a").identity().clone(),
        RequestAuthenticationIdentity::ApiKey,
    ] {
        let wrong = dsh_agent::AgentAuthenticationBinding {
            turn: binding.turn,
            identity,
        };
        assert!(!h.agent.cancel_authentication(&wrong).unwrap());
    }
    h.agent
        .send(message("retained queue"), InboxTarget::NextTurn, false);
    a.revoke();
    assert!(h.agent.cancel_authentication(&binding).unwrap());
    idle(h.agent.as_ref()).await;
    signed_out(h.agent.as_ref());
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.agent.inbox().next_turn().len(), 1);
    assert!(!h.agent.cancel_authentication(&binding).unwrap());
    let mut background = message("automatic goal continuation");
    background.source = dsh_llm::MessageSource::Goal {
        goal_id: "goal-fixture".into(),
        revision: 1,
        round: 2,
    };
    h.agent.followup(background);
    idle(h.agent.as_ref()).await;
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        1,
        "background followup cannot resume after logout"
    );
    assert_eq!(h.agent.inbox().next_turn().len(), 2);
}

#[tokio::test]
async fn a_late_first_prepared_call_observes_revocation_after_the_logout_scan() {
    let a = account("a", "login-a");
    let gate = Arc::new(Gate::default());
    let preparation = Arc::new(Gate::default());
    let (h, _) = setup(a.clone(), gate.clone(), Some(preparation.clone())).await;
    h.agent.followup(message("preparing"));
    notified(&preparation.entered).await;
    assert!(h.agent.authentication_binding().is_none());
    a.revoke();
    preparation.release.notify_one();
    idle(h.agent.as_ref()).await;
    signed_out(h.agent.as_ref());
    assert_eq!(gate.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stale_turn_confirmation_does_not_cancel_a_new_api_key_turn() {
    let gate = Arc::new(Gate::default());
    let a = account("a", "login-a");
    let (h, selected) = setup(a.clone(), gate.clone(), None).await;
    h.agent.followup(message("account turn"));
    notified(&gate.entered).await;
    let old = h.agent.authentication_binding().unwrap();
    let old_observer = h.agent.authentication_observer().unwrap();
    gate.release.notify_one();
    idle(h.agent.as_ref()).await;
    a.revoke();
    let before = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let before_hook = before.clone();
    let release_hook = release.clone();
    let listener = h
        .ctx
        .on(
            "agent/request",
            Arc::new(move |_, args| {
                let before = before_hook.clone();
                let release = release_hook.clone();
                let next = cordis::downcast_arc::<cordis::NextFn>(&args[1])
                    .expect("request waterfall continuation");
                Box::pin(async move {
                    before.notify_one();
                    release.notified().await;
                    let proposal = next.call().await;
                    assert!(cordis::downcast::<dsh_llm::LlmCallConfig>(&proposal).is_some());
                    Some(proposal)
                })
            }),
            Default::default(),
        )
        .await;
    *selected.lock() = RequestAuthentication::new(RequestAuthenticationIdentity::ApiKey);
    h.agent.followup(message("new API key turn"));
    notified(&before).await;
    assert!(
        h.agent.authentication_binding().is_none(),
        "previous request/context is not current authentication"
    );
    assert!(!h.agent.cancel_authentication(&old).unwrap());
    release.notify_one();
    notified(&gate.entered).await;
    assert_eq!(
        h.agent.authentication_binding().unwrap().identity,
        RequestAuthenticationIdentity::ApiKey
    );
    assert!(!h.agent.cancel_authentication(&old).unwrap());
    assert!(old_observer(account("a", "login-a")).is_err());
    assert_eq!(h.agent.status(), dsh_agent::AgentStatus::Running);
    gate.release.notify_one();
    idle(h.agent.as_ref()).await;
    listener().await;
}

async fn finishing_turn_logout_blocks_background_handoff(after_idle: bool) {
    let a = account("a", "login-a");
    let gate = Arc::new(Gate::default());
    let (h, selected) = setup(a.clone(), gate.clone(), None).await;
    let finishing = Arc::new(Notify::new());
    let release_finish = Arc::new(Notify::new());
    let entered = finishing.clone();
    let release = release_finish.clone();
    let first = Arc::new(AtomicBool::new(true));
    let hook = h
        .ctx
        .on(
            "agent/turn-finished",
            Arc::new(move |_, _| {
                let first = first.clone();
                let entered = entered.clone();
                let release = release.clone();
                Box::pin(async move {
                    if first.swap(false, Ordering::SeqCst) {
                        entered.notify_one();
                        release.notified().await;
                    }
                    None
                })
            }),
            Default::default(),
        )
        .await;
    h.agent.followup(message("A's work chain"));
    notified(&gate.entered).await;
    assert_eq!(
        &h.agent.authentication_binding().unwrap().identity,
        a.identity()
    );
    gate.release.notify_one();
    notified(&finishing).await;
    assert_eq!(h.agent.status(), dsh_agent::AgentStatus::Running);
    assert_eq!(
        h.agent
            .session()
            .events()
            .iter()
            .rev()
            .find(|event| event.type_ == "turn/end")
            .unwrap()
            .data["reason"]["kind"],
        "completed"
    );

    // Match the Host's revoke-then-query-and-cancel sequence. The selected B
    // models the automatic activation of the remaining saved account.
    let impact_binding = h.agent.authentication_binding();
    a.revoke();
    if let Some(binding) = &impact_binding {
        h.agent.cancel_authentication(binding).unwrap();
    }
    *selected.lock() = account("b", "login-b");
    let mut background = message("continue the existing goal");
    background.source = dsh_llm::MessageSource::Goal {
        goal_id: "already-running-goal".into(),
        revision: 1,
        round: 2,
    };
    // Let an erroneously dispatched B finish, so the assertion reports the
    // actual extra dispatch rather than failing on a hanging model timeout.
    gate.release.notify_one();
    if !after_idle {
        h.agent.followup(background.clone());
    }
    release_finish.notify_one();
    idle(h.agent.as_ref()).await;
    if after_idle {
        // GoalRoundDriver dispatches after its first Idle notification.
        h.agent.followup(background);
        idle(h.agent.as_ref()).await;
    }
    hook().await;
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        1,
        "logout during turn-finished must stop the same background work chain; after_idle={after_idle}, impact_binding={impact_binding:?}"
    );
    assert_eq!(
        h.agent.inbox().next_turn().len(),
        1,
        "keep the undispatched continuation queued"
    );
}

#[tokio::test]
async fn logout_during_turn_finished_does_not_dispatch_queued_background_work_with_b() {
    finishing_turn_logout_blocks_background_handoff(false).await;
}

#[tokio::test]
async fn logout_during_turn_finished_does_not_dispatch_goal_handoff_after_idle_with_b() {
    finishing_turn_logout_blocks_background_handoff(true).await;
}

#[tokio::test]
async fn handoff_pause_reentrant_append_failure_is_reported_and_background_work_stays_queued() {
    let a = account("a", "login-a");
    let gate = Arc::new(Gate::default());
    let (h, selected) = setup(a.clone(), gate.clone(), None).await;
    let finishing = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let entered = finishing.clone();
    let finish_release = release.clone();
    let finished = h
        .ctx
        .on(
            "agent/turn-finished",
            Arc::new(move |_, _| {
                let entered = entered.clone();
                let release = finish_release.clone();
                Box::pin(async move {
                    entered.notify_one();
                    release.notified().await;
                    None
                })
            }),
            Default::default(),
        )
        .await;
    h.agent.followup(message("first"));
    notified(&gate.entered).await;
    gate.release.notify_one();
    notified(&finishing).await;
    let binding = h.agent.authentication_binding().unwrap();
    let inspected = Arc::new(AtomicBool::new(false));
    let seen = inspected.clone();
    let weak = Arc::downgrade(&h.agent);
    let outcome = Arc::new(Mutex::new(None));
    let recorded = outcome.clone();
    let expected = binding.clone();
    let fault = h
        .ctx
        .on(
            "internal/dispatch",
            Arc::new(move |_, args| {
                let weak = weak.clone();
                let seen = seen.clone();
                let recorded = recorded.clone();
                let expected = expected.clone();
                Box::pin(async move {
                    let is_event = cordis::downcast_arc::<String>(&args[1])
                        .is_some_and(|name| name.as_str() == "session/event");
                    if is_event
                        && let Some(payload) =
                            cordis::downcast_arc::<Vec<cordis::ArcValue>>(&args[2])
                        && let Some(event) = payload
                            .get(1)
                            .and_then(cordis::downcast_arc::<dsh_session::SessionEvent>)
                        && event.type_ == "fixture/account-pause-trigger"
                    {
                        let agent = weak.upgrade().unwrap();
                        let _ = agent.status();
                        let _ = agent.authentication_binding();
                        let _ = agent.session().events();
                        seen.store(true, Ordering::SeqCst);
                        // This is a real append rejection: the outer Session
                        // publication still owns the same-thread append guard.
                        *recorded.lock() = Some(agent.cancel_authentication(&expected));
                    }
                    None
                })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;
    a.revoke();
    *selected.lock() = account("b", "login-b");
    h.agent
        .session()
        .append("fixture/account-pause-trigger", serde_json::json!({}), None)
        .unwrap();
    assert!(
        outcome
            .lock()
            .take()
            .expect("precommit cancellation was invoked")
            .unwrap_err()
            .contains("cannot reenter")
    );
    assert!(
        inspected.load(Ordering::SeqCst),
        "precommit must be able to read Agent and Session without held Phase locks"
    );
    let mut background = message("retained background work");
    background.source = dsh_llm::MessageSource::Goal {
        goal_id: "running".into(),
        revision: 1,
        round: 2,
    };
    h.agent.followup(background);
    release.notify_one();
    idle(h.agent.as_ref()).await;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.agent.inbox().next_turn().len(), 1);
    assert_eq!(h.agent.status(), dsh_agent::AgentStatus::Idle);
    assert!(
        !h.agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "agent/account-continuation")
    );
    fault().await;
    finished().await;
}

#[tokio::test]
async fn failed_manual_resume_control_preserves_both_existing_and_new_input() {
    let h = harness().await;
    let session = h.agent.session();
    session
        .append("turn/start", serde_json::json!({"turn":1}), None)
        .unwrap();
    session
        .append(
            "turn/end",
            serde_json::json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    session
        .append(
            "agent/account-continuation",
            serde_json::json!({
                "owner":session.id().as_str(),"turn":1,"revision":9_007_199_254_740_991_u64,
                "paused":true,"reason":"account-signed-out"
            }),
            None,
        )
        .unwrap();
    let restored_session = dsh_session::Session::from_restore(
        session.id().clone(),
        session.events().to_vec(),
        session.header(),
        session.inherited_event_count(),
    )
    .unwrap();
    let restored = dsh_agent_loop::ReactLoopAgent::new(
        &h.ctx,
        session.id().clone(),
        h.agent.options().clone(),
        restored_session,
    )
    .unwrap();
    restored.send(message("retained queue"), InboxTarget::NextTurn, false);
    let failed =
        restored.send_with_context_checked(message("explicit resume"), InboxTarget::NextTurn, None);
    let error = failed.unwrap_err();
    assert!(
        error.contains("输入已保留在队列") && error.contains("revision exhausted"),
        "{error}"
    );
    assert_eq!(restored.inbox().next_turn().len(), 2);
    assert_eq!(restored.status(), dsh_agent::AgentStatus::Idle);
    assert_eq!(
        restored
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "agent/account-continuation")
            .count(),
        1
    );
}

#[tokio::test]
async fn disposing_with_an_earlier_clear_inbox_decision_cannot_erase_a_new_logout_pause() {
    let a = account("a", "login-a");
    let gate = Arc::new(Gate::default());
    let (h, _) = setup(a.clone(), gate.clone(), None).await;
    h.agent.followup(message("running"));
    notified(&gate.entered).await;
    h.agent
        .send(message("keep after logout"), InboxTarget::NextTurn, false);
    let stale_options = dsh_agent::CancelOptions { keep_inbox: false };
    let binding = h.agent.authentication_binding().unwrap();
    a.revoke();
    assert!(h.agent.cancel_authentication(&binding).unwrap());
    h.agent
        .cancel(dsh_agent::AgentCancelCause::Disposed, Some(&stale_options));
    idle(h.agent.as_ref()).await;
    assert_eq!(h.agent.inbox().next_turn().len(), 1);
    signed_out(h.agent.as_ref());
}

#[tokio::test]
async fn a_late_flush_failure_cannot_repause_a_new_manual_api_key_turn() {
    let a = account("a", "login-a");
    let gate = Arc::new(Gate::default());
    let (h, selected) = setup(a.clone(), gate.clone(), None).await;
    let finishing = Arc::new(Notify::new());
    let release_finish = Arc::new(Notify::new());
    let entered = finishing.clone();
    let release = release_finish.clone();
    let first = Arc::new(AtomicBool::new(true));
    let finish_hook = h
        .ctx
        .on(
            "agent/turn-finished",
            Arc::new(move |_, _| {
                let entered = entered.clone();
                let release = release.clone();
                let first = first.clone();
                Box::pin(async move {
                    if first.swap(false, Ordering::SeqCst) {
                        entered.notify_one();
                        release.notified().await;
                    }
                    None
                })
            }),
            Default::default(),
        )
        .await;
    h.agent.followup(message("A work"));
    notified(&gate.entered).await;
    gate.release.notify_one();
    notified(&finishing).await;
    let flush_entered = Arc::new(Notify::new());
    let release_flush = Arc::new(Notify::new());
    let entered = flush_entered.clone();
    let release = release_flush.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let flush_hook = h
        .ctx
        .on(
            "session/flush",
            Arc::new(move |_, _| {
                let entered = entered.clone();
                let release = release.clone();
                let calls = calls.clone();
                Box::pin(async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        entered.notify_one();
                        release.notified().await;
                        panic!("old pause flush failed");
                    }
                    None
                })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;
    let binding = h.agent.authentication_binding().unwrap();
    a.revoke();
    assert!(h.agent.cancel_authentication(&binding).unwrap());
    let old_flush = tokio::spawn(h.agent.flush_authentication_control());
    notified(&flush_entered).await;
    *selected.lock() = RequestAuthentication::new(RequestAuthenticationIdentity::ApiKey);
    h.agent
        .send_with_context_checked(
            message("explicit new API key task"),
            InboxTarget::NextTurn,
            None,
        )
        .unwrap();
    h.agent.flush_authentication_control().await.unwrap();
    assert!(observed.load(Ordering::SeqCst) >= 2);
    release_finish.notify_one();
    notified(&gate.entered).await;
    assert_eq!(
        h.agent.authentication_binding().unwrap().identity,
        RequestAuthenticationIdentity::ApiKey
    );
    release_flush.notify_one();
    assert!(old_flush.await.unwrap().is_err());
    gate.release.notify_one();
    idle(h.agent.as_ref()).await;
    let mut next = message("continue new task");
    next.source = dsh_llm::MessageSource::Goal {
        goal_id: "new-task".into(),
        revision: 1,
        round: 2,
    };
    gate.release.notify_one();
    h.agent.followup(next);
    idle(h.agent.as_ref()).await;
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        3,
        "an old flush error cannot pause the new task's automatic continuation"
    );
    assert!(!h.agent.inbox().has_pending());
    flush_hook().await;
    finish_hook().await;
}

#[tokio::test]
async fn turn_start_reservation_and_publication_exclude_concurrent_account_cancellation() {
    let a = account("a", "login-a");
    let gate = Arc::new(Gate::default());
    let (h, selected) = setup(a.clone(), gate.clone(), None).await;
    let protected = Arc::new(AtomicBool::new(false));
    let checked = protected.clone();
    let cancellation = Arc::new(Mutex::new(None));
    let owned = cancellation.clone();
    let weak = Arc::downgrade(&h.agent);
    let revoked = a.clone();
    let hook = h
        .ctx
        .on(
            "internal/dispatch",
            Arc::new(move |_, args| {
                let weak = weak.clone();
                let checked = checked.clone();
                let owned = owned.clone();
                let revoked = revoked.clone();
                Box::pin(async move {
                    if cordis::downcast_arc::<String>(&args[1])
                        .is_some_and(|name| name.as_str() == "session/event")
                        && let Some(payload) =
                            cordis::downcast_arc::<Vec<cordis::ArcValue>>(&args[2])
                        && let Some(event) = payload
                            .get(1)
                            .and_then(cordis::downcast_arc::<dsh_session::SessionEvent>)
                        && event.type_ == "turn/start"
                        && event.data["turn"] == 2
                    {
                        let agent = weak.upgrade().unwrap();
                        let binding = agent.authentication_binding().unwrap();
                        checked.store(
                            agent
                                .try_generation_control(agent.cancellation_generation().unwrap())
                                .is_err(),
                            Ordering::SeqCst,
                        );
                        let attempted = Arc::new(AtomicBool::new(false));
                        let began = attempted.clone();
                        *owned.lock() = Some(std::thread::spawn(move || {
                            revoked.revoke();
                            began.store(true, Ordering::SeqCst);
                            agent.cancel_authentication(&binding)
                        }));
                        let deadline = std::time::Instant::now() + Duration::from_secs(2);
                        while !attempted.load(Ordering::SeqCst)
                            && std::time::Instant::now() < deadline
                        {
                            std::thread::yield_now();
                        }
                    }
                    None
                })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;
    h.agent.followup(message("first"));
    notified(&gate.entered).await;
    let mut next = message("automatic next round");
    next.source = dsh_llm::MessageSource::Goal {
        goal_id: "running".into(),
        revision: 1,
        round: 2,
    };
    h.agent.followup(next);
    *selected.lock() = account("b", "login-b");
    gate.release.notify_one();
    idle(h.agent.as_ref()).await;
    assert!(
        protected.load(Ordering::SeqCst),
        "the phase reservation must remain inside the same control boundary as turn/start publication"
    );
    assert!(cancellation.lock().take().unwrap().join().unwrap().is_ok());
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    let events = h.agent.session().events();
    let start = events
        .iter()
        .position(|event| event.type_ == "turn/start" && event.data["turn"] == 2)
        .unwrap();
    let pause = events
        .iter()
        .position(|event| event.type_ == "agent/account-continuation" && event.data["turn"] == 2)
        .unwrap();
    assert!(
        start < pause,
        "a control cannot precede its durable turn/start"
    );
    let restored = dsh_session::Session::from_restore(
        h.agent.id().clone(),
        events.to_vec(),
        h.agent.session().header(),
        h.agent.session().inherited_event_count(),
    )
    .unwrap();
    dsh_agent_loop::ReactLoopAgent::new(
        &h.ctx,
        h.agent.id().clone(),
        h.agent.options().clone(),
        restored,
    )
    .unwrap();
    hook().await;
}

#[tokio::test]
async fn tool_phase_keeps_a_binding_and_rejects_automatic_b_after_a_is_revoked() {
    let a = account("a", "login-a");
    let inner = Arc::new(NamedToolThenTextAdapter {
        name: "quick".into(),
        calls: AtomicUsize::new(0),
    });
    let (h, selected) = setup(a.clone(), inner.clone(), None).await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut tool = quick_tool(Arc::new(AtomicBool::new(false)));
    let tool_entered = entered.clone();
    let tool_release = release.clone();
    tool.execute = Arc::new(move |_, _| {
        let entered = tool_entered.clone();
        let release = tool_release.clone();
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            Ok(serde_json::json!("done"))
        })
    });
    h.tools.register(&h.ctx, tool).unwrap();
    h.agent.followup(message("tool"));
    notified(&entered).await;
    assert_eq!(
        &h.agent.authentication_binding().unwrap().identity,
        a.identity()
    );
    a.revoke();
    *selected.lock() = account("b", "login-b");
    release.notify_one();
    idle(h.agent.as_ref()).await;
    signed_out(h.agent.as_ref());
    assert_eq!(
        inner.calls.load(Ordering::SeqCst),
        1,
        "no second request may silently use B"
    );
}

#[tokio::test]
async fn auxiliary_llm_stream_binds_the_turn_before_any_conversation_request() {
    let a = account("a", "login-a");
    let gate = Arc::new(Gate::default());
    let (h, _) = setup(a.clone(), gate.clone(), None).await;
    let llm = h
        .ctx
        .get_typed::<Arc<dsh_llm::LlmRuntime>>("llm", false)
        .unwrap()
        .as_ref()
        .clone();
    let hook = h
        .ctx
        .on(
            "agent/pre-step",
            Arc::new(move |_, args| {
                let llm = llm.clone();
                let next = cordis::downcast_arc::<cordis::NextFn>(&args[1])
                    .expect("pre-step waterfall continuation");
                Box::pin(async move {
                    let options = GenerateOptions {
                        provider: "test".into(),
                        model: "model".into(),
                        purpose: Some("compaction".into()),
                        agent_loop_request: false,
                        reasoning_effort: None,
                        messages: vec![],
                        system: None,
                        tools: None,
                        temperature: None,
                        max_tokens: None,
                        stop: None,
                        signal: None,
                        session_id: None,
                        telemetry: None,
                    };
                    let _ = llm.stream(options).collect::<Vec<_>>().await;
                    Some(next.call().await)
                })
            }),
            Default::default(),
        )
        .await;
    h.agent.followup(message("requires compaction"));
    notified(&gate.entered).await;
    let bound = h
        .agent
        .authentication_binding()
        .expect("auxiliary stream must bind its initiating turn");
    assert_eq!(&bound.identity, a.identity());
    a.revoke();
    assert!(h.agent.cancel_authentication(&bound).unwrap());
    idle(h.agent.as_ref()).await;
    signed_out(h.agent.as_ref());
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    hook().await;
}

#[tokio::test]
async fn retry_wait_retains_the_account_and_cannot_retry_with_another_login_after_revocation() {
    struct Failing(AtomicUsize);
    impl LlmAdapter for Failing {
        fn stream(&self, _: &GenerateOptions) -> ChunkStream {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(futures::stream::iter([StreamChunk::Finish {
                reason: FinishReason::Error {
                    failure: dsh_llm::LlmFailure {
                        message: "synthetic retry".into(),
                        code: "TRANSPORT".into(),
                        status: None,
                        provider_retry_after_ms: None,
                        request_id: None,
                        offload_images: None,
                    },
                },
                replay_state: None,
            }]))
        }
    }
    let a = account("a", "login-a");
    let inner = Arc::new(Failing(AtomicUsize::new(0)));
    let (h, selected) = setup(a.clone(), inner.clone(), None).await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let notify = entered.clone();
    let proceed = release.clone();
    let hook = h
        .ctx
        .on(
            "agent/request-error",
            Arc::new(move |_, _| {
                let notify = notify.clone();
                let proceed = proceed.clone();
                Box::pin(async move {
                    notify.notify_one();
                    proceed.notified().await;
                    Some(cordis::arc(Some(dsh_agent::RequestErrorAction::Retry)))
                })
            }),
            Default::default(),
        )
        .await;
    h.agent.followup(message("retry"));
    notified(&entered).await;
    assert_eq!(
        &h.agent.authentication_binding().unwrap().identity,
        a.identity()
    );
    a.revoke();
    *selected.lock() = account("b", "login-b");
    release.notify_one();
    idle(h.agent.as_ref()).await;
    signed_out(h.agent.as_ref());
    assert_eq!(inner.0.load(Ordering::SeqCst), 1);
    hook().await;
}

#[tokio::test]
async fn signed_out_inbox_survives_dispose_and_real_jsonl_cold_resume_without_waking() {
    cold_resume_after_logout(false).await;
}

#[tokio::test]
async fn finished_chain_pause_survives_cold_resume_and_old_controls_cannot_override_manual_resume()
{
    cold_resume_after_logout(true).await;
}

async fn cold_resume_after_logout(finishing: bool) {
    let root = std::env::temp_dir().join(format!("oauth-inbox-{}", uuid::Uuid::new_v4()));
    let a = account("a", "login-a");
    let gate = Arc::new(Gate::default());
    let selected = Arc::new(Mutex::new(a.clone()));
    let create_runtime = || {
        let ctx = cordis::Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        let llm = dsh_llm::LlmRuntime::install(&ctx);
        llm.register_adapter(
            &ctx,
            vec!["test".into()],
            Arc::new(Routed {
                selected: selected.clone(),
                inner: gate.clone(),
                snapshot_gate: None,
            }),
        )
        .unwrap();
        dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
        let sessions = dsh_session::SessionStore::install(&ctx);
        AgentRegistry::install(&ctx);
        dsh_session_persistence_jsonl::JsonlSessionPersistence::install(
            &ctx,
            dsh_session_persistence_jsonl::JsonlConfig {
                root: root.to_string_lossy().into(),
                ..Default::default()
            },
        )
        .unwrap();
        let agent_loop = dsh_agent_loop::AgentLoop::install(&ctx, Default::default()).unwrap();
        (ctx, sessions, agent_loop)
    };
    let options = dsh_agent::AgentOptions {
        provider: Some("test".into()),
        model: Some("model".into()),
        ..Default::default()
    };
    let id = dsh_session::session_id("signed-out-retained-inbox");
    let (ctx, sessions, agent_loop) = create_runtime();
    let finish_entered = Arc::new(Notify::new());
    let finish_release = Arc::new(Notify::new());
    if finishing {
        let entered = finish_entered.clone();
        let release = finish_release.clone();
        ctx.on(
            "agent/turn-finished",
            Arc::new(move |_, _| {
                let entered = entered.clone();
                let release = release.clone();
                Box::pin(async move {
                    entered.notify_one();
                    release.notified().await;
                    None
                })
            }),
            Default::default(),
        )
        .await;
    }
    let handle = agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(id.clone()),
                agent_options: Some(options.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    handle.agent.followup(message("first"));
    notified(&gate.entered).await;
    handle
        .agent
        .send(message("retained"), InboxTarget::NextTurn, false);
    if finishing {
        gate.release.notify_one();
        notified(&finish_entered).await;
    }
    let binding = handle.agent.authentication_binding().unwrap();
    a.revoke();
    assert!(handle.agent.cancel_authentication(&binding).unwrap());
    if finishing {
        finish_release.notify_one();
    }
    idle(handle.agent.as_ref()).await;
    let control = handle
        .agent
        .session()
        .events()
        .iter()
        .rev()
        .find(|event| event.type_ == "agent/account-continuation")
        .unwrap()
        .clone();
    assert_eq!(control.ignorable, Some(true));
    assert_eq!(control.data["owner"], id.as_str());
    assert!(
        control.data.get("accountScope").is_none() && control.data.get("loginGeneration").is_none()
    );
    if finishing {
        assert_eq!(
            handle
                .agent
                .session()
                .events()
                .iter()
                .rev()
                .find(|event| event.type_ == "turn/end")
                .unwrap()
                .data["reason"]["kind"],
            "completed"
        );
    }
    assert!(sessions.flush(handle.agent.session()).await.unwrap());
    handle.dispose.await;
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
    *selected.lock() = account("b", "login-b");
    let (ctx2, _, loop2) = create_runtime();
    let restored = loop2
        .resume(
            &ctx2,
            dsh_agent::ResumeAgentOptions {
                resume_session_id: Some(id.clone()),
                agent_options: Some(options.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    idle(restored.agent.as_ref()).await;
    assert_eq!(restored.agent.status(), dsh_agent::AgentStatus::Idle);
    assert_eq!(restored.agent.inbox().next_turn().len(), 1);
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    restored
        .agent
        .steer_queued(&restored.agent.inbox().next_turn()[0].id)
        .unwrap();
    notified(&gate.entered).await;
    assert_eq!(
        &restored.agent.authentication_binding().unwrap().identity,
        selected.lock().identity()
    );
    // A control from the old completed turn arrives after a newer explicit
    // user wake and turn/start. Replay must not let it pause this new turn.
    restored
        .agent
        .session()
        .append("agent/account-continuation", control.data, None)
        .unwrap();
    gate.release.notify_one();
    idle(restored.agent.as_ref()).await;
    restored.dispose.await;
    for dispose in ctx2.fiber.disposables.clear() {
        dispose().await;
    }
    let (ctx3, _, loop3) = create_runtime();
    let again = loop3
        .resume(
            &ctx3,
            dsh_agent::ResumeAgentOptions {
                resume_session_id: Some(id),
                agent_options: Some(options),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    *selected.lock() = RequestAuthentication::new(RequestAuthenticationIdentity::ApiKey);
    let mut next = message("new authorized work after manual resume");
    next.source = dsh_llm::MessageSource::Goal {
        goal_id: "new-work".into(),
        revision: 1,
        round: 1,
    };
    gate.release.notify_one();
    again.agent.followup(next);
    idle(again.agent.as_ref()).await;
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        3,
        "a late old control must not restore a pause after explicit resume"
    );
    again.dispose.await;
    for dispose in ctx3.fiber.disposables.clear() {
        dispose().await;
    }
    std::fs::remove_dir_all(root).unwrap();
}
