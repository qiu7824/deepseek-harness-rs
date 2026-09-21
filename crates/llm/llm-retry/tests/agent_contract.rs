use cordis::{BoxFuture, Context, arc, downcast_arc};
use dsh_agent::{
    Agent, AgentOptions, AgentStatus, Inbox, InboxNotifications, InboxTarget, RequestErrorAction,
};
use dsh_session::{Session, SessionId, UserMessage};
use std::sync::Arc;

struct RequestAgent {
    session: Session,
    inbox: Inbox,
    ctx: Context,
    options: AgentOptions,
    key: dsh_scope::ScopeKey,
}

#[tokio::test]
async fn exhausted_quota_never_enters_transport_retry_even_with_always_policy() {
    let ctx = Context::root();
    let session =
        Session::create(dsh_session::session_id("quota-no-retry"), None, None, None).unwrap();
    let agent: Arc<dyn Agent> = Arc::new(RequestAgent {
        inbox: Inbox::new(&session, InboxNotifications::default()).unwrap(),
        session,
        ctx: ctx.clone(),
        options: AgentOptions::default(),
        key: dsh_scope::ScopeKey::new(),
    });
    let dispose = dsh_llm_retry::apply(
        &ctx,
        &serde_json::json!({}),
        dsh_llm_retry::RetryInternals::default(),
    )
    .unwrap();
    let backoff = dsh_llm::ResolvedRetryBackoff {
        initial_delay_ms: 1,
        max_delay_ms: 1,
        jitter_ratio: 0.0,
    };
    let dispatcher = dsh_agent::AgentEventDispatch::new(&ctx, agent.clone());
    for policy in [
        dsh_llm::ResolvedRetryPolicy::Always { backoff },
        dsh_llm::ResolvedRetryPolicy::Normal {
            max_retries: 2,
            retryable_codes: vec![dsh_llm::QUOTA_EXCEEDED_CODE.into()],
            backoff,
        },
    ] {
        let result = dispatcher
            .waterfall(
                "agent/request-error",
                |agent| {
                    arc(dsh_agent::AgentRequestErrorPayload {
                        agent: agent.clone(),
                        turn: 1,
                        step: 1,
                        provider: "devin".into(),
                        failure: dsh_llm::LlmFailure {
                            offload_images: None,
                            message: "Your weekly usage quota has been exhausted".into(),
                            code: dsh_llm::QUOTA_EXCEEDED_CODE.into(),
                            status: Some(400),
                            provider_retry_after_ms: None,
                            request_id: None,
                        },
                        retry_policy: Some(policy.clone()),
                        signal: dsh_agent::CancellationSignal::new(),
                    })
                },
                Box::pin(async { arc(None::<RequestErrorAction>) }),
            )
            .await;
        assert_eq!(
            *downcast_arc::<Option<RequestErrorAction>>(&result).unwrap(),
            None
        );
    }
    assert!(
        !agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_.starts_with("llm/retry"))
    );
    dispose().await;
}
impl Agent for RequestAgent {
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
        AgentStatus::Running
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        &self.key
    }
    fn cancel(&self, _: dsh_agent::AgentCancelCause, _: Option<&dsh_agent::CancelOptions>) {}
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        _: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}

#[tokio::test]
async fn retry_uses_the_real_agent_waterfall_payload_and_optional_decision_contract() {
    let ctx = Context::root();
    let session =
        Session::create(dsh_session::session_id("retry-contract"), None, None, None).unwrap();
    let agent: Arc<dyn Agent> = Arc::new(RequestAgent {
        inbox: Inbox::new(&session, InboxNotifications::default()).unwrap(),
        session,
        ctx: ctx.clone(),
        options: AgentOptions::default(),
        key: dsh_scope::ScopeKey::new(),
    });
    let dispose = dsh_llm_retry::apply(
        &ctx,
        &serde_json::json!({}),
        dsh_llm_retry::RetryInternals::default(),
    )
    .unwrap();
    let dispatcher = dsh_agent::AgentEventDispatch::new(&ctx, agent.clone());
    for attempt in 0..2 {
        let result = dispatcher
            .waterfall(
                "agent/request-error",
                |agent| {
                    arc(dsh_agent::AgentRequestErrorPayload {
                        agent: agent.clone(),
                        turn: 1,
                        step: 1,
                        provider: "fixture".into(),
                        failure: dsh_llm::LlmFailure {
                            offload_images: None,
                            message: "interrupted response body".into(),
                            code: "TRANSPORT".into(),
                            status: None,
                            provider_retry_after_ms: None,
                            request_id: None,
                        },
                        retry_policy: Some(dsh_llm::ResolvedRetryPolicy::Normal {
                            max_retries: 1,
                            retryable_codes: vec!["TRANSPORT".into()],
                            backoff: dsh_llm::ResolvedRetryBackoff {
                                initial_delay_ms: 1,
                                max_delay_ms: 1,
                                jitter_ratio: 0.0,
                            },
                        }),
                        signal: dsh_agent::CancellationSignal::new(),
                    })
                },
                Box::pin(async { arc(None::<RequestErrorAction>) }),
            )
            .await;
        let decision = downcast_arc::<Option<RequestErrorAction>>(&result)
            .expect("same type the Agent loop consumes");
        assert_eq!(
            *decision,
            if attempt == 0 {
                Some(RequestErrorAction::Retry)
            } else {
                None
            }
        );
    }
    assert_eq!(
        agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "llm/retry-started")
            .count(),
        1
    );
    dispose().await;
}
