//! Provider-routed model-request retry policy on the agent loop's request
//! recovery extension point. Rust port of
//! `packages/llm/llm-retry/src/index.ts`.
//!
//! # Deviations
//!
//! - `AbortSignal` has no Rust equivalent; a [`CancellationSignal`]
//!   (`AtomicBool` + `Notify`) carries both the request and the plugin
//!   lifetime cancellation, and `AbortSignal.any` becomes a select over both
//!   `cancelled()` futures.
//! - The deterministic test hook is `RetryInternals { random }`.
//! - `apply` returns a `Disposer` (TS returns `void` and registers the
//!   teardown via `ctx.effect`); in-flight recovery is counted and the
//!   disposer waits for it to drain.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cordis::{
    ArcValue, Context, Disposer, EventOptions, Listener, NextFn, arc, downcast_arc, make_disposer,
};
use dsh_agent::{Agent, RequestErrorAction};
use dsh_llm::{LlmFailure, ResolvedRetryPolicy};

use crate::brand::RetryId;
use crate::types::{LlmRetryEventData, LlmRetryStartedEventData};

/// The shared request-cancellation signal (defined in dsh-agent so the loop
/// can publish it; re-exported for the retry plugin's API surface).
pub use dsh_agent::CancellationSignal;

/// The `agent/request-error` waterfall payload (defined in dsh-agent;
/// re-exported under the name the TS plugin publishes it as).
pub use dsh_agent::AgentRequestErrorPayload as RequestErrorPayload;

/// Cordis plugin name (TS `name`).
pub const NAME: &str = "llm-retry";

/// The plugin requires the agent registry's extension point (TS `inject`).
pub const INJECT: [&str; 1] = ["agents"];

/// This policy executor has no config; providers own `retryPolicy`.
#[derive(Debug, Clone, Default)]
pub struct Config {}

/// Non-serializable hooks used to make timing policy deterministic in tests.
#[derive(Clone)]
pub struct RetryInternals {
    /// Random sample in the inclusive zero-to-one range used for jitter.
    pub random: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl Default for RetryInternals {
    fn default() -> Self {
        Self {
            random: Arc::new(|| {
                // A cheap xorshift in the absence of a rand crate.
                static STATE: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
                let mut state = STATE.load(Ordering::Relaxed);
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                STATE.store(state, Ordering::Relaxed);
                (state >> 11) as f64 / (1u64 << 53) as f64
            }),
        }
    }
}

/// The empty executor config shape (TS `validateConfig`).
pub fn validate_executor_config(config: &serde_json::Value) -> Result<(), String> {
    let Some(object) = config.as_object() else {
        return Ok(());
    };
    for key in object.keys() {
        if key == "retryPolicy" {
            return Err("llm-retry: retryPolicy belongs under each provider configuration".to_string());
        }
        return Err(format!("llm-retry: unknown key \"{key}\""));
    }
    Ok(())
}

fn local_delay(policy: &ResolvedRetryPolicy, retry: u64, random: &dyn Fn() -> f64) -> u64 {
    let backoff = policy.backoff();
    let exponent = std::cmp::min(retry - 1, 1024);
    let exponential = (backoff.initial_delay_ms as f64 * 2f64.powi(exponent as i32))
        .min(backoff.max_delay_ms as f64);
    let jitter = 1.0 - backoff.jitter_ratio + 2.0 * backoff.jitter_ratio * random();
    (exponential * jitter).min(backoff.max_delay_ms as f64) as u64
}

/// JSON number token matching `JSON.stringify` (integral floats serialize
/// without a decimal point — load-bearing for `policyKey` stability).
fn number_token(value: f64) -> serde_json::Value {
    if value.fract() == 0.0 && value.abs() < 9.007_199_254_740_992e15 {
        serde_json::json!(value as i64)
    } else {
        serde_json::json!(value)
    }
}

fn retry_policy_key(policy: &ResolvedRetryPolicy) -> String {
    match policy {
        ResolvedRetryPolicy::Always { backoff } => serde_json::to_string(&serde_json::json!([
            "always",
            backoff.initial_delay_ms,
            backoff.max_delay_ms,
            number_token(backoff.jitter_ratio),
        ]))
        .expect("policy key"),
        ResolvedRetryPolicy::Normal { max_retries, retryable_codes, backoff } => {
            let mut codes = retryable_codes.clone();
            codes.sort();
            serde_json::to_string(&serde_json::json!([
                "normal",
                max_retries,
                codes,
                backoff.initial_delay_ms,
                backoff.max_delay_ms,
                number_token(backoff.jitter_ratio),
            ]))
            .expect("policy key")
        }
    }
}

/// Shared executor state for one installed plugin instance.
struct RetryState {
    random: Arc<dyn Fn() -> f64 + Send + Sync>,
    lifetime: Arc<CancellationSignal>,
    /// In-flight recovery count; the disposer waits for zero.
    active: AtomicU64,
    drained: tokio::sync::Notify,
}

/// Install provider-routed normal or unbounded request recovery (TS
/// `apply`). Returns the disposer (abort + drain).
pub fn apply(
    ctx: &Context,
    config: &serde_json::Value,
    internals: RetryInternals,
) -> Result<Disposer, String> {
    validate_executor_config(config)?;
    let state = Arc::new(RetryState {
        random: internals.random,
        lifetime: CancellationSignal::new(),
        active: AtomicU64::new(0),
        drained: tokio::sync::Notify::new(),
    });

    // agent/request-error waterfall listener: args[0] = payload,
    // args[1] = NextFn (appended by the waterfall dispatcher).
    let listener_state = Arc::clone(&state);
    let listener: Arc<Listener> = Arc::new(move |_listener_ctx, args: Vec<ArcValue>| {
        let state = Arc::clone(&listener_state);
        Box::pin(async move {
            // A waterfall may have captured this callback before its
            // registration was removed; lifetime cancellation prevents the
            // stale callback from entering a downstream policy.
            if state.lifetime.aborted() {
                return None;
            }
            let payload = downcast_arc::<Arc<RequestErrorPayload>>(&args[0])
                .expect("agent/request-error payload");
            let next = downcast_arc::<NextFn>(&args[1]).expect("agent/request-error next");
            state.active.fetch_add(1, Ordering::SeqCst);
            let decision = recover(&state, payload.as_ref(), next).await;
            if state.active.fetch_sub(1, Ordering::SeqCst) == 1 {
                state.drained.notify_waiters();
            }
            decision
        })
    });

    let dispose_listener = futures::executor::block_on(ctx.on(
        "agent/request-error",
        listener,
        EventOptions::default(),
    ));

    let state_for_dispose = Arc::clone(&state);
    Ok(make_disposer(move || {
        let state = Arc::clone(&state_for_dispose);
        let dispose_listener = dispose_listener.clone();
        Box::pin(async move {
            dispose_listener().await;
            state.lifetime.abort();
            loop {
                if state.active.load(Ordering::SeqCst) == 0 {
                    return;
                }
                state.drained.notified().await;
            }
        })
    }))
}

/// One downstream recovery attempt, contained (TS `settleDownstream`).
enum DownstreamOutcome {
    Decision(Option<RequestErrorAction>),
}

async fn settle_downstream(next: Arc<NextFn>) -> DownstreamOutcome {
    let value = next.call().await;
    match downcast_arc::<RequestErrorAction>(&value) {
        Some(decision) => DownstreamOutcome::Decision(Some(*decision)),
        None => DownstreamOutcome::Decision(None),
    }
}

#[allow(clippy::too_many_arguments)]
async fn recover(
    state: &RetryState,
    payload: &RequestErrorPayload,
    next: Arc<NextFn>,
) -> Option<ArcValue> {
    let RequestErrorPayload {
        agent,
        turn,
        step,
        provider,
        failure,
        retry_policy,
        signal,
    } = payload;
    let Some(policy) = retry_policy else {
        return Some(next.call().await);
    };
    if policy.mode() == "always" {
        if signal.aborted() || state.lifetime.aborted() {
            return None;
        }
        // The loop and plugin lifetime stay open until delegated recovery
        // settles; an abort then wins before the decision or fallback can
        // mutate later state.
        let downstream = settle_downstream(next.clone()).await;
        if signal.aborted() || state.lifetime.aborted() {
            return None;
        }
        if let DownstreamOutcome::Decision(Some(RequestErrorAction::Retry)) = downstream {
            return Some(arc(RequestErrorAction::Retry));
        }
    } else if !policy
        .retryable_codes()
        .expect("normal policy")
        .contains(&failure.code)
    {
        return Some(next.call().await);
    }

    let policy_key = retry_policy_key(policy);
    let events = agent.session().events();
    let prior_policy_retry = events.iter().rev().find(|event| {
        event.type_ == "llm/retry"
            && event.data.get("turn").and_then(|value| value.as_u64()) == Some(*turn)
            && event.data.get("step").and_then(|value| value.as_u64()) == Some(*step)
            && event.data.get("provider").and_then(|value| value.as_str()) == Some(provider.as_str())
            && event.data.get("policyKey").and_then(|value| value.as_str()) == Some(policy_key.as_str())
    });
    let previous_retry = prior_policy_retry
        .and_then(|event| event.data.get("retry").and_then(|value| value.as_u64()))
        .unwrap_or(0);
    if policy.mode() == "normal" && previous_retry >= policy.max_retries().expect("normal policy") {
        return Some(next.call().await);
    }
    let retry = previous_retry + 1;
    let retry_id: RetryId = match prior_policy_retry {
        Some(event) => crate::brand::retry_id(
            event
                .data
                .get("retryId")
                .and_then(|value| value.as_str())
                .expect("llm/retry retryId")
                .to_string(),
        ),
        None => crate::brand::retry_id(uuid::Uuid::new_v4().simple().to_string()),
    };
    let delay_ms = match failure.provider_retry_after_ms {
        Some(retry_after) if retry_after > 0 => {
            if retry_after > policy.backoff().max_delay_ms {
                if policy.mode() == "normal" {
                    return Some(next.call().await);
                }
                local_delay(policy, retry, state.random.as_ref())
            } else {
                retry_after
            }
        }
        _ => local_delay(policy, retry, state.random.as_ref()),
    };

    backoff(
        agent.clone(),
        *turn,
        *step,
        failure.clone(),
        provider.clone(),
        policy.clone(),
        policy_key,
        retry,
        retry_id,
        delay_ms,
        signal.clone(),
        state,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn backoff(
    agent: Arc<dyn Agent>,
    turn: u64,
    step: u64,
    failure: LlmFailure,
    provider: String,
    policy: ResolvedRetryPolicy,
    policy_key: String,
    retry: u64,
    retry_id: RetryId,
    delay_ms: u64,
    signal: Arc<CancellationSignal>,
    state: &RetryState,
) -> Option<ArcValue> {
    if signal.aborted() || state.lifetime.aborted() {
        return None;
    }
    let event_data = match &policy {
        ResolvedRetryPolicy::Normal { max_retries, .. } => LlmRetryEventData::Normal {
            retry_id: retry_id.clone(),
            turn,
            step,
            provider: provider.clone(),
            policy_key: policy_key.clone(),
            retry,
            max_retries: *max_retries,
            delay_ms,
            failure: failure.clone(),
        },
        ResolvedRetryPolicy::Always { .. } => LlmRetryEventData::Always {
            retry_id: retry_id.clone(),
            turn,
            step,
            provider: provider.clone(),
            policy_key: policy_key.clone(),
            retry,
            delay_ms,
            failure: failure.clone(),
        },
    };
    let data = serde_json::to_value(&event_data).expect("llm/retry data is JSON");
    if agent.session().append("llm/retry", data, None).is_err() {
        return None;
    }
    // Fused signal: abort on either the request signal or the lifetime.
    let waited = tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => true,
        _ = signal.cancelled() => false,
        _ = state.lifetime.cancelled() => false,
    };
    if !waited {
        return None;
    }
    let started = LlmRetryStartedEventData {
        retry_id,
        turn,
        step,
        retry,
    };
    let data = serde_json::to_value(&started).expect("llm/retry-started data is JSON");
    if agent.session().append("llm/retry-started", data, None).is_err() {
        return None;
    }
    Some(arc(RequestErrorAction::Retry))
}
