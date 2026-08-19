//! Rust port of the core `packages/llm/llm-retry/tests/retry.spec.ts`
//! behaviors, driven at the waterfall extension point without the (unported)
//! LlmRuntime/AgentLoop stack: a mock Agent owns a real detached Session.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentOptions, AgentStatus, Inbox, RequestErrorAction};
use dsh_llm::{LlmFailure, ResolvedRetryPolicy, resolve_retry_policy};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionStore, UserMessage, session_id};

struct TestAgent {
    session: Session,
    inbox: dsh_agent::Inbox,
}

impl TestAgent {
    fn new(session: Session) -> Self {
        let inbox = dsh_agent::Inbox::new(&session, Default::default()).expect("inbox");
        Self { session, inbox }
    }
}

impl Agent for TestAgent {
    fn id(&self) -> &dsh_session::SessionId {
        self.session.id()
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
        unreachable!("not used by the retry executor")
    }

    fn scope_key(&self) -> &ScopeKey {
        static KEY: std::sync::OnceLock<ScopeKey> = std::sync::OnceLock::new();
        KEY.get_or_init(ScopeKey::new)
    }

    fn cancel(
        &self,
        _cause: dsh_agent::AgentCancelCause,
        _options: Option<&dsh_agent::CancelOptions>,
    ) {
    }

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: UserMessage, _target: dsh_agent::InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: UserMessage) {}

    fn steer(&self, _message: UserMessage) {}

    fn inject(&self, _message: UserMessage) {}
}

fn failure(code: &str) -> LlmFailure {
    LlmFailure {
        message: "busy".to_string(),
        code: code.to_string(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

async fn harness() -> (Context, Arc<SessionStore>, Arc<TestAgent>) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let session = store
        .prepare(
            Some(session_id("retry-test")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .unwrap();
    (ctx, store, Arc::new(TestAgent::new(session)))
}

fn open_step(agent: &TestAgent) {
    let session = agent.session();
    session
        .append("turn/start", serde_json::json!({"turn": 1}), None)
        .unwrap();
    session
        .append(
            "request/header",
            serde_json::json!({
                "header": {"config": {"provider": "mock", "model": "mock"}}
            }),
            None,
        )
        .unwrap();
    session
        .append(
            "step/start",
            serde_json::json!({"turn": 1, "step": 1}),
            None,
        )
        .unwrap();
}

/// Drive one `agent/request-error` waterfall round and return the decision.
async fn drive(
    ctx: Context,
    agent: Arc<TestAgent>,
    policy: Option<ResolvedRetryPolicy>,
    failure: LlmFailure,
) -> Option<RequestErrorAction> {
    let payload = Arc::new(dsh_llm_retry::RequestErrorPayload {
        agent,
        turn: 1,
        step: 1,
        provider: "mock".to_string(),
        failure,
        retry_policy: policy,
        signal: dsh_llm_retry::CancellationSignal::new(),
    });
    let decision = ctx
        .waterfall(
            "agent/request-error",
            vec![cordis::arc(payload)],
            Box::pin(async { cordis::arc(()) }),
        )
        .await;
    cordis::downcast::<RequestErrorAction>(&decision).copied()
}

fn retry_events(session: &Session) -> Vec<String> {
    session
        .events()
        .iter()
        .filter(|event| event.type_ == "llm/retry" || event.type_ == "llm/retry-started")
        .map(|event| {
            format!(
                "{}#{}",
                event.type_,
                event
                    .data
                    .get("retry")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0)
            )
        })
        .collect()
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn records_the_scheduled_delay_before_retrying() {
    let (ctx, _store, agent) = harness().await;
    let _dispose = dsh_llm_retry::apply(
        &ctx,
        &serde_json::json!({}),
        dsh_llm_retry::RetryInternals {
            random: Arc::new(|| 0.5),
        },
    )
    .unwrap();
    open_step(&agent);
    let policy = resolve_retry_policy(
        Some(&serde_json::json!({
            "mode": "normal",
            "retryableCodes": ["SERVER", "RATE_LIMIT"],
            "backoff": {"initialDelayMs": 500, "maxDelayMs": 10000, "jitterRatio": 0},
        })),
        "p",
    )
    .unwrap();

    // The first round schedules the wait (the delay future pends).
    let decision = tokio::spawn(drive(
        ctx.clone(),
        agent.clone(),
        Some(policy.clone()),
        failure("RATE_LIMIT"),
    ));
    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    // The llm/retry event is durable BEFORE the wait completes.
    let events = agent.session().events();
    assert!(events.iter().any(|event| event.type_ == "llm/retry"));
    let retry_event = events
        .iter()
        .find(|event| event.type_ == "llm/retry")
        .unwrap();
    assert_eq!(
        retry_event
            .data
            .get("retry")
            .and_then(|value| value.as_u64()),
        Some(1)
    );
    assert_eq!(
        retry_event
            .data
            .get("mode")
            .and_then(|value| value.as_str()),
        Some("normal")
    );
    assert_eq!(
        retry_event
            .data
            .get("maxRetries")
            .and_then(|value| value.as_u64()),
        Some(2)
    );
    assert_eq!(
        retry_event
            .data
            .get("delayMs")
            .and_then(|value| value.as_u64()),
        Some(500)
    );
    assert_eq!(
        retry_event
            .data
            .get("policyKey")
            .and_then(|value| value.as_str()),
        Some("[\"normal\",2,[\"RATE_LIMIT\",\"SERVER\"],500,10000,0]")
    );
    assert_eq!(
        retry_event.data.get("failure"),
        Some(&serde_json::to_value(failure("RATE_LIMIT")).unwrap())
    );

    // The wait completes and the transition is recorded before the decision.
    tokio::time::advance(std::time::Duration::from_millis(500)).await;
    let decision = decision.await.unwrap();
    assert_eq!(decision, Some(RequestErrorAction::Retry));
    assert_eq!(
        retry_events(&agent.session()),
        vec!["llm/retry#1".to_string(), "llm/retry-started#1".to_string()]
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn passes_through_non_retryable_codes_and_unconfigured_providers() {
    let (ctx, _store, agent) = harness().await;
    let _dispose = dsh_llm_retry::apply(&ctx, &serde_json::json!({}), Default::default()).unwrap();
    open_step(&agent);

    // No policy: the downstream decision passes through untouched.
    let decision = drive(ctx.clone(), agent.clone(), None, failure("SERVER")).await;
    assert_eq!(decision, None, "no policy → downstream (dummy) decision");
    assert!(
        !agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "llm/retry")
    );

    // A non-retryable code under a normal policy passes through.
    let policy = resolve_retry_policy(
        Some(&serde_json::json!({"mode": "normal", "retryableCodes": ["RATE_LIMIT"]})),
        "p",
    )
    .unwrap();
    let decision = drive(
        ctx.clone(),
        agent.clone(),
        Some(policy),
        failure("INVALID_CREDENTIAL"),
    )
    .await;
    assert_eq!(decision, None);
    assert!(
        !agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "llm/retry")
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn honors_max_retries_and_keeps_retry_id_across_the_chain() {
    let (ctx, _store, agent) = harness().await;
    let _dispose = dsh_llm_retry::apply(
        &ctx,
        &serde_json::json!({}),
        dsh_llm_retry::RetryInternals {
            random: Arc::new(|| 0.5),
        },
    )
    .unwrap();
    open_step(&agent);
    let policy = resolve_retry_policy(
        Some(&serde_json::json!({
            "mode": "normal",
            "maxRetries": 1,
            "backoff": {"initialDelayMs": 1, "maxDelayMs": 10, "jitterRatio": 0},
        })),
        "p",
    )
    .unwrap();

    let first = drive(
        ctx.clone(),
        agent.clone(),
        Some(policy.clone()),
        failure("SERVER"),
    )
    .await;
    assert_eq!(first, Some(RequestErrorAction::Retry));
    let retry_id = agent
        .session()
        .events()
        .iter()
        .find(|event| event.type_ == "llm/retry")
        .and_then(|event| event.data.get("retryId").cloned())
        .expect("retryId");

    // A second failure with the same route reuses the retryId but exceeds
    // maxRetries → pass through.
    let second = drive(ctx.clone(), agent.clone(), Some(policy), failure("SERVER")).await;
    assert_eq!(second, None, "maxRetries exhausted → downstream");
    let events = agent.session().events();
    let retries: Vec<&dsh_session::SessionEvent> = events
        .iter()
        .filter(|event| event.type_ == "llm/retry")
        .collect();
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0].data.get("retryId"), Some(&retry_id));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn honors_provider_retry_after_ms_within_the_cap() {
    let (ctx, _store, agent) = harness().await;
    let _dispose = dsh_llm_retry::apply(
        &ctx,
        &serde_json::json!({}),
        dsh_llm_retry::RetryInternals {
            random: Arc::new(|| 0.5),
        },
    )
    .unwrap();
    open_step(&agent);
    let policy = resolve_retry_policy(
        Some(&serde_json::json!({"mode": "normal", "backoff": {"initialDelayMs": 500, "maxDelayMs": 10000, "jitterRatio": 0}})),
        "p",
    )
    .unwrap();
    let mut failure = failure("SERVER");
    failure.provider_retry_after_ms = Some(300);
    let decision = tokio::spawn(drive(ctx.clone(), agent.clone(), Some(policy), failure));
    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    let retry_event = agent
        .session()
        .events()
        .iter()
        .find(|event| event.type_ == "llm/retry")
        .cloned()
        .expect("llm/retry");
    assert_eq!(
        retry_event
            .data
            .get("delayMs")
            .and_then(|value| value.as_u64()),
        Some(300)
    );
    tokio::time::advance(std::time::Duration::from_millis(300)).await;
    assert_eq!(decision.await.unwrap(), Some(RequestErrorAction::Retry));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn lifetime_cancellation_stops_a_pending_wait() {
    let (ctx, _store, agent) = harness().await;
    let dispose = dsh_llm_retry::apply(
        &ctx,
        &serde_json::json!({}),
        dsh_llm_retry::RetryInternals {
            random: Arc::new(|| 0.5),
        },
    )
    .unwrap();
    open_step(&agent);
    let policy = resolve_retry_policy(
        Some(&serde_json::json!({"mode": "normal", "backoff": {"initialDelayMs": 10000, "maxDelayMs": 10000, "jitterRatio": 0}})),
        "p",
    )
    .unwrap();
    let decision = tokio::spawn(drive(
        ctx.clone(),
        agent.clone(),
        Some(policy),
        failure("SERVER"),
    ));
    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    assert!(
        agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "llm/retry")
    );
    // Dispose the plugin: the pending wait aborts and no transition lands.
    dispose().await;
    let outcome = decision.await.unwrap();
    assert!(
        outcome.is_none(),
        "cancelled wait must not deliver a decision"
    );
    assert!(
        !agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "llm/retry-started")
    );
}

#[test]
fn executor_config_validation() {
    assert!(dsh_llm_retry::validate_executor_config(&serde_json::json!({})).is_ok());
    let error = dsh_llm_retry::validate_executor_config(&serde_json::json!({"retryPolicy": {}}))
        .unwrap_err();
    assert!(
        error.contains("retryPolicy belongs under each provider"),
        "{error}"
    );
    let error =
        dsh_llm_retry::validate_executor_config(&serde_json::json!({"other": 1})).unwrap_err();
    assert!(error.contains("unknown key"), "{error}");
}
