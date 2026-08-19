//! Package-owned durable retry-event invariants. Rust port of
//! `packages/llm/llm-retry/src/invariant.ts`.
//!
//! # Deviation
//!
//! - The TS companion validates through `internal/dispatch` (pre-hook);
//!   the Rust port listens to `session/event` globally instead, so a
//!   failing append reports at dispatch completion rather than before it.

use std::sync::Arc;

use cordis::{ArcValue, BoxFuture, Context, Disposer, EventOptions, Listener, downcast};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_llm::LlmFailure;
use dsh_session::{Session, SessionEvent};
use dsh_timeout::MAX_TIMER_DELAY_MS;

use crate::history::provider_for_open_step;

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-llm-retry";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "llm-retry-invariant";

/// Services required before the companion can register (TS `inject`).
pub const INJECT: [&str; 1] = ["invariants"];

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> BoxFuture<'static, Disposer> {
    let ctx = ctx.clone();
    Box::pin(async move {
        let invariants = ctx
            .get_typed::<Arc<InvariantRegistry>>("invariants", false)
            .expect("invariants service required by llm-retry-invariant");
        invariants.register(
            &ctx,
            PACKAGE_NAME,
            InvariantInstaller {
                install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
                    let ctx = ctx.clone();
                    Box::pin(async move { install_inner(&ctx, fail).await })
                }),
                inject: Some(cordis::InjectSpec::new(["sessions"])),
            },
        )
    })
}

async fn install_inner(ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>) {
    // Validate every retry record already present in one loaded session.
    if let Some(sessions) = ctx.get_typed::<Arc<dsh_session::SessionStore>>("sessions", false) {
        for session in sessions.list() {
            validate_session(&session, &fail);
        }
    }
    let created_fail = Arc::clone(&fail);
    let created: Arc<Listener> = Arc::new(move |_ctx, args: Vec<ArcValue>| {
        let session = downcast::<Session>(&args[0]).cloned().expect("session arg");
        let fail = Arc::clone(&created_fail);
        Box::pin(async move {
            validate_session(&session, &fail);
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "session/created",
        created,
        EventOptions::default().global(true),
    ));

    let event_fail = Arc::clone(&fail);
    let event: Arc<Listener> = Arc::new(move |_ctx, args: Vec<ArcValue>| {
        let session = downcast::<Session>(&args[0]).cloned().expect("session arg");
        let event = downcast::<SessionEvent>(&args[1])
            .cloned()
            .expect("event arg");
        let fail = Arc::clone(&event_fail);
        Box::pin(async move {
            let history = session.events();
            match event.type_.as_str() {
                "llm/retry" => validate_retry(&history, &event, &fail),
                "llm/retry-started" => validate_started(&history, &event, &fail),
                _ => {}
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "session/event",
        event,
        EventOptions::default().global(true),
    ));
}

/// Validate the complete provider-neutral failure payload (TS
/// `validateFailure`).
fn validate_failure(value: &serde_json::Value, fail: &Arc<dyn Fn(&str) + Send + Sync>) {
    let Some(object) = value.as_object() else {
        fail("llm/retry failure must be an object");
        return;
    };
    let message = object.get("message").and_then(|value| value.as_str());
    if message.is_none_or(|message| message.is_empty()) {
        fail("llm/retry failure.message must be a non-empty string");
    }
    let code = object.get("code").and_then(|value| value.as_str());
    if code.is_none_or(|code| code.is_empty()) {
        fail("llm/retry failure.code must be a non-empty string");
    }
    if let Some(status) = object.get("status").and_then(|value| value.as_u64()) {
        if !(100..=599).contains(&status) {
            fail("llm/retry failure.status must be an integer from 100 through 599 when present");
        }
    }
    if let Some(retry_after) = object
        .get("providerRetryAfterMs")
        .and_then(|value| value.as_u64())
    {
        if retry_after == 0 {
            fail(
                "llm/retry failure.providerRetryAfterMs must be a positive finite number when present",
            );
        }
    }
    if let Some(request_id) = object.get("requestId") {
        if !request_id.is_string() || request_id.as_str().is_some_and(|id| id.is_empty()) {
            fail("llm/retry failure.requestId must be a non-empty string when present");
        }
    }
    // The parsed shape must match the canonical wire type.
    if serde_json::from_value::<LlmFailure>(value.clone()).is_err() {
        fail("llm/retry failure does not match the canonical failure wire shape");
    }
}

/// Validate one retry record against the currently open request step (TS
/// `validateRetry`).
fn validate_retry(
    history: &[SessionEvent],
    event: &SessionEvent,
    fail: &Arc<dyn Fn(&str) + Send + Sync>,
) {
    let data = &event.data;
    let retry_id = data.get("retryId").and_then(|value| value.as_str());
    if retry_id.is_none_or(|retry_id| retry_id.is_empty()) {
        fail("llm/retry retryId must be a non-empty string");
        return;
    }
    let failure = data
        .get("failure")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    validate_failure(&failure, fail);
    let retry = data.get("retry").and_then(|value| value.as_u64());
    if retry.is_none_or(|retry| retry < 1) {
        fail("llm/retry retry must be a positive safe integer");
    }
    let provider = data.get("provider").and_then(|value| value.as_str());
    if provider.is_none_or(|provider| provider.is_empty()) {
        fail("llm/retry provider must be a non-empty string");
    }
    let policy_key = data.get("policyKey").and_then(|value| value.as_str());
    if policy_key.is_none_or(|policy_key| policy_key.is_empty()) {
        fail("llm/retry policyKey must be a non-empty string");
    }
    let mode = data.get("mode").and_then(|value| value.as_str());
    let retry = retry.expect("checked");
    match mode {
        Some("normal") => {
            let max_retries = data.get("maxRetries").and_then(|value| value.as_u64());
            if max_retries.is_none_or(|max_retries| max_retries < 1 || retry > max_retries) {
                fail(&format!(
                    "llm/retry retry {retry} must not exceed a positive safe maxRetries {}",
                    max_retries
                        .map(|value| value.to_string())
                        .unwrap_or_default()
                ));
            }
        }
        Some("always") => {
            if data
                .as_object()
                .is_some_and(|object| object.contains_key("maxRetries"))
            {
                fail("llm/retry always mode must omit maxRetries");
            }
        }
        other => fail(&format!(
            "llm/retry mode must be normal or always, got {}",
            other.unwrap_or("undefined")
        )),
    }
    let delay_ms = data.get("delayMs").and_then(|value| value.as_u64());
    if delay_ms.is_none_or(|delay_ms| delay_ms > MAX_TIMER_DELAY_MS) {
        fail(&format!(
            "llm/retry delayMs must be a finite number within 0..{MAX_TIMER_DELAY_MS}"
        ));
    }

    let turn = data.get("turn").and_then(|value| value.as_u64());
    let step = data.get("step").and_then(|value| value.as_u64());
    let (Some(turn), Some(step)) = (turn, step) else {
        fail("llm/retry must carry integer turn/step");
        return;
    };
    let turn_boundary = history
        .iter()
        .rev()
        .find(|prior| prior.type_ == "turn/start" || prior.type_ == "turn/end");
    if turn_boundary.is_none_or(|boundary| boundary.type_ != "turn/start") {
        fail("llm/retry must be appended inside an open turn");
        return;
    }
    let boundary_turn = turn_boundary
        .expect("checked")
        .data
        .get("turn")
        .and_then(|value| value.as_u64());
    if boundary_turn != Some(turn) {
        fail(&format!(
            "llm/retry names turn {turn}, but the open turn is {}",
            boundary_turn.map(|t| t.to_string()).unwrap_or_default()
        ));
    }
    let step_boundary = history
        .iter()
        .rev()
        .find(|prior| prior.type_ == "step/start" || prior.type_ == "step/end");
    if step_boundary.is_none_or(|boundary| boundary.type_ != "step/start") {
        fail("llm/retry must be appended inside an open step");
        return;
    }
    let boundary_step = step_boundary
        .expect("checked")
        .data
        .get("step")
        .and_then(|value| value.as_u64());
    let boundary_turn = step_boundary
        .expect("checked")
        .data
        .get("turn")
        .and_then(|value| value.as_u64());
    if boundary_step != Some(step) || boundary_turn != Some(turn) {
        fail(&format!(
            "llm/retry names turn {turn}/step {step}, but the open step is {}/{}",
            boundary_turn.map(|t| t.to_string()).unwrap_or_default(),
            boundary_step.map(|s| s.to_string()).unwrap_or_default()
        ));
    }
    let routed_provider = provider_for_open_step(history, turn, step);
    if routed_provider.as_deref() != provider {
        fail(&format!(
            "llm/retry provider {} does not match the failed request provider {}",
            provider.unwrap_or("undefined"),
            routed_provider.as_deref().unwrap_or("undefined")
        ));
    }

    let prior_policy_retry = history.iter().rev().find(|prior| {
        prior.type_ == "llm/retry"
            && prior.data.get("turn").and_then(|value| value.as_u64()) == Some(turn)
            && prior.data.get("step").and_then(|value| value.as_u64()) == Some(step)
            && prior.data.get("provider") == data.get("provider")
            && prior.data.get("policyKey") == data.get("policyKey")
    });
    let expected_retry = prior_policy_retry
        .and_then(|prior| prior.data.get("retry").and_then(|value| value.as_u64()))
        .unwrap_or(0)
        + 1;
    if retry != expected_retry {
        fail(&format!(
            "llm/retry retry {retry} must equal provider policy retry {expected_retry}"
        ));
    }
    if let Some(prior) = prior_policy_retry {
        if prior.data.get("retryId") != data.get("retryId") {
            fail("llm/retry must preserve retryId across one provider-policy chain");
        }
    }
    if prior_policy_retry.is_none()
        && history.iter().any(|prior| {
            (prior.type_ == "llm/retry" || prior.type_ == "llm/retry-started")
                && prior.data.get("retryId") == data.get("retryId")
        })
    {
        fail("llm/retry retryId is already owned by another chain");
    }
}

/// Validate one wait-complete transition against its scheduled attempt (TS
/// `validateStarted`).
fn validate_started(
    history: &[SessionEvent],
    event: &SessionEvent,
    fail: &Arc<dyn Fn(&str) + Send + Sync>,
) {
    let data = &event.data;
    let retry_id = data.get("retryId").and_then(|value| value.as_str());
    if retry_id.is_none_or(|retry_id| retry_id.is_empty()) {
        fail("llm/retry-started retryId must be a non-empty string");
        return;
    }
    let retry = data.get("retry").and_then(|value| value.as_u64());
    let scheduled = history.iter().rev().find(|prior| {
        prior.type_ == "llm/retry"
            && prior.data.get("retryId").and_then(|value| value.as_str()) == retry_id
            && prior.data.get("retry").and_then(|value| value.as_u64()) == retry
    });
    let Some(scheduled) = scheduled else {
        fail("llm/retry-started pairs no prior scheduled attempt");
        return;
    };
    if scheduled.data.get("turn") != data.get("turn")
        || scheduled.data.get("step") != data.get("step")
    {
        fail("llm/retry-started turn/step must match its scheduled attempt");
    }
    if history.iter().any(|prior| {
        prior.type_ == "llm/retry-started"
            && prior.data.get("retryId") == data.get("retryId")
            && prior.data.get("retry") == data.get("retry")
    }) {
        fail("llm/retry-started repeats one scheduled attempt");
    }
}

/// Validate every retry record already present in one loaded session (TS
/// `validateSession`).
fn validate_session(session: &Session, fail: &Arc<dyn Fn(&str) + Send + Sync>) {
    let events = session.events();
    for (index, event) in events.iter().enumerate() {
        let history = &events[..index];
        match event.type_.as_str() {
            "llm/retry" => validate_retry(history, event, fail),
            "llm/retry-started" => validate_started(history, event, fail),
            _ => {}
        }
    }
}
