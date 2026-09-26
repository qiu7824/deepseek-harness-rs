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

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-llm-retry";

struct RetryFact {
    type_: &'static str,
    data: serde_json::Value,
}

/// Only facts used to check retry correlation. Historical prompts, schemas,
/// failure prose and arbitrary event payloads never enter this projection.
#[derive(Default)]
struct RetryEvidence {
    facts: Vec<RetryFact>,
    provider: Option<String>,
}

impl RetryEvidence {
    fn observe(&mut self, event: &SessionEvent) {
        let kind = match event.type_.as_str() {
            "request/header" => {
                self.provider = event
                    .data
                    .pointer("/header/config/provider")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                return;
            }
            "turn/start" => "turn/start",
            "turn/end" => "turn/end",
            "step/start" => "step/start",
            "step/end" => "step/end",
            "llm/retry" => "llm/retry",
            "llm/retry-started" => "llm/retry-started",
            _ => return,
        };
        let mut data = serde_json::Map::new();
        for name in ["turn", "step", "retryId", "retry", "provider", "policyKey"] {
            if let Some(value) = event.data.get(name) {
                data.insert(name.into(), value.clone());
            }
        }
        self.facts.push(RetryFact {
            type_: kind,
            data: serde_json::Value::Object(data),
        });
    }

    fn provider_for_open_step(&self, turn: u64, step: u64) -> Option<&str> {
        let start = self.facts.iter().rposition(|event| {
            event.type_ == "step/start"
                && event.data["turn"].as_u64() == Some(turn)
                && event.data["step"].as_u64() == Some(step)
        })?;
        if self.facts[start + 1..]
            .iter()
            .any(|event| matches!(event.type_, "step/end" | "turn/end"))
        {
            return None;
        }
        self.provider.as_deref()
    }
}

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
        let event = downcast::<SessionEvent>(&args[1]).expect("event arg");
        if !matches!(event.type_.as_str(), "llm/retry" | "llm/retry-started") {
            return Box::pin(async { None });
        }
        let session = downcast::<Session>(&args[0]).cloned().expect("session arg");
        let event = event.clone();
        let fail = Arc::clone(&event_fail);
        Box::pin(async move {
            let mut history = RetryEvidence::default();
            if let Err(error) = session.visit_events(0, Some(event.seq.get()), |prior| {
                history.observe(prior);
                Ok(true)
            }) {
                fail(&format!("could not read retry protocol history: {error}"));
                return None;
            }
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
    history: &RetryEvidence,
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
        .facts
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
        .facts
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
    let routed_provider = history.provider_for_open_step(turn, step);
    if routed_provider.as_deref() != provider {
        fail(&format!(
            "llm/retry provider {} does not match the failed request provider {}",
            provider.unwrap_or("undefined"),
            routed_provider.as_deref().unwrap_or("undefined")
        ));
    }

    let prior_policy_retry = history.facts.iter().rev().find(|prior| {
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
        && history.facts.iter().any(|prior| {
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
    history: &RetryEvidence,
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
    let scheduled = history.facts.iter().rev().find(|prior| {
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
    if history.facts.iter().any(|prior| {
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
    let mut history = RetryEvidence::default();
    if let Err(error) = session.visit_events(0, None, |event| {
        match event.type_.as_str() {
            "llm/retry" => validate_retry(&history, event, fail),
            "llm/retry-started" => validate_started(&history, event, fail),
            _ => {}
        }
        history.observe(event);
        Ok(true)
    }) {
        fail(&format!("could not read retry protocol history: {error}"));
    }
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    use serde_json::json;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    #[derive(Clone, Copy)]
    struct AllocationCount {
        active: bool,
        live: usize,
        peak: usize,
    }
    thread_local! {
        static ALLOCATION_COUNT: Cell<AllocationCount> = const {
            Cell::new(AllocationCount { active:false, live:0, peak:0 })
        };
    }
    struct CountAlloc;
    fn allocation_change(added: usize, removed: usize) {
        let _ = ALLOCATION_COUNT.try_with(|count| {
            let mut value = count.get();
            if value.active {
                value.live = value.live.saturating_sub(removed).saturating_add(added);
                value.peak = value.peak.max(value.live);
                count.set(value);
            }
        });
    }
    unsafe impl GlobalAlloc for CountAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                allocation_change(layout.size(), 0);
            }
            ptr
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc_zeroed(layout) };
            if !ptr.is_null() {
                allocation_change(layout.size(), 0);
            }
            ptr
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            allocation_change(0, layout.size());
            unsafe {
                System.dealloc(ptr, layout);
            }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let next = unsafe { System.realloc(ptr, layout, size) };
            if !next.is_null() {
                allocation_change(size, layout.size());
            }
            next
        }
    }
    #[global_allocator]
    static ALLOCATOR: CountAlloc = CountAlloc;

    fn protocol_event(kind: &str, data: serde_json::Value) -> SessionEvent {
        SessionEvent {
            type_: kind.into(),
            seq: dsh_session::SessionSeq::new(0).unwrap(),
            time: 0,
            data,
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    fn retry_event(provider: &str, policy: &str, id: &str, retry: u64) -> SessionEvent {
        protocol_event(
            "llm/retry",
            json!({"turn":1,"step":1,"retryId":id,"retry":retry,"provider":provider,
                "policyKey":policy,"mode":"normal","maxRetries":8,"delayMs":1,
                "failure":{"message":"temporary failure","code":"TRANSIENT"}}),
        )
    }

    fn checked_errors(history: &RetryEvidence, event: &SessionEvent) -> Vec<String> {
        let errors = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let observed = errors.clone();
        let fail: Arc<dyn Fn(&str) + Send + Sync> =
            Arc::new(move |error| observed.lock().push(error.into()));
        match event.type_.as_str() {
            "llm/retry" => validate_retry(history, event, &fail),
            "llm/retry-started" => validate_started(history, event, &fail),
            _ => unreachable!(),
        }
        let reported = errors.lock().clone();
        reported
    }

    fn open_step_evidence() -> RetryEvidence {
        let mut history = RetryEvidence::default();
        for event in [
            protocol_event("turn/start", json!({"turn":1})),
            protocol_event("step/start", json!({"turn":1,"step":1})),
            protocol_event(
                "request/header",
                json!({"header":{"config":{"provider":"account-a"}}}),
            ),
        ] {
            history.observe(&event);
        }
        history
    }

    #[test]
    fn projected_provider_matches_full_history_at_each_protocol_boundary() {
        let mut history = Vec::new();
        let mut evidence = RetryEvidence::default();
        for event in [
            protocol_event(
                "request/header",
                json!({"header":{"config":{"provider":"a"}}}),
            ),
            protocol_event("turn/start", json!({"turn":1})),
            protocol_event("step/start", json!({"turn":1,"step":1})),
            protocol_event(
                "request/header",
                json!({"header":{"config":{"provider":"b"}}}),
            ),
            protocol_event("request/header", json!({"header":{"config":{}}})),
            protocol_event(
                "request/header",
                json!({"header":{"config":{"provider":"c"}}}),
            ),
            protocol_event("step/end", json!({"turn":1,"step":1})),
            protocol_event("step/start", json!({"turn":1,"step":2})),
            protocol_event("turn/end", json!({"turn":1})),
            protocol_event("turn/start", json!({"turn":2})),
            protocol_event("step/start", json!({"turn":2,"step":1})),
        ] {
            evidence.observe(&event);
            history.push(event);
            for (turn, step) in [(1, 1), (1, 2), (2, 1), (2, 2)] {
                assert_eq!(
                    evidence.provider_for_open_step(turn, step),
                    crate::history::provider_for_open_step(&history, turn, step).as_deref(),
                    "boundary {} for {turn}/{step}",
                    history.len()
                );
            }
        }
    }

    #[test]
    fn projected_retry_keeps_provider_policy_chain_ownership_and_sequence() {
        let mut history = open_step_evidence();
        let first = retry_event("account-a", "policy-a", "retry-a", 1);
        assert!(checked_errors(&history, &first).is_empty());
        history.observe(&first);
        assert!(
            checked_errors(
                &history,
                &retry_event("account-a", "policy-a", "retry-a", 2)
            )
            .is_empty()
        );
        assert!(
            checked_errors(
                &history,
                &retry_event("account-a", "policy-a", "changed-id", 2)
            )
            .iter()
            .any(|error| error.contains("preserve retryId"))
        );
        assert!(
            checked_errors(
                &history,
                &retry_event("account-a", "policy-a", "retry-a", 3)
            )
            .iter()
            .any(|error| error.contains("must equal provider policy retry 2"))
        );
        assert!(
            checked_errors(
                &history,
                &retry_event("account-a", "policy-b", "retry-a", 1)
            )
            .iter()
            .any(|error| error.contains("already owned by another chain"))
        );
        history.observe(&protocol_event(
            "request/header",
            json!({"header":{"config":{"provider":"account-b"}}}),
        ));
        assert!(
            checked_errors(
                &history,
                &retry_event("account-b", "policy-a", "retry-b", 1)
            )
            .is_empty()
        );
        assert!(
            checked_errors(
                &history,
                &retry_event("account-a", "policy-a", "retry-a", 2)
            )
            .iter()
            .any(|error| error.contains("does not match the failed request provider"))
        );
        history.observe(&protocol_event("step/end", json!({"turn":1,"step":1})));
        assert!(
            checked_errors(
                &history,
                &retry_event("account-b", "policy-a", "retry-b", 1)
            )
            .iter()
            .any(|error| error.contains("inside an open step"))
        );
        history.observe(&protocol_event("turn/end", json!({"turn":1})));
        assert!(
            checked_errors(
                &history,
                &retry_event("account-b", "policy-a", "retry-b", 1)
            )
            .iter()
            .any(|error| error.contains("inside an open turn"))
        );
    }

    #[test]
    fn projected_started_keeps_attempt_pairing_and_duplicate_detection() {
        let mut history = open_step_evidence();
        let started = protocol_event(
            "llm/retry-started",
            json!({"turn":1,"step":1,"retryId":"retry-a","retry":1}),
        );
        assert!(
            checked_errors(&history, &started)
                .iter()
                .any(|error| error.contains("pairs no prior scheduled attempt"))
        );
        history.observe(&retry_event("account-a", "policy-a", "retry-a", 1));
        assert!(checked_errors(&history, &started).is_empty());
        let mut mismatched = started.clone();
        mismatched.data["step"] = json!(2);
        assert!(
            checked_errors(&history, &mismatched)
                .iter()
                .any(|error| error.contains("turn/step must match"))
        );
        history.observe(&started);
        assert!(
            checked_errors(&history, &started)
                .iter()
                .any(|error| error.contains("repeats one scheduled attempt"))
        );
    }

    #[test]
    fn live_validation_prefix_excludes_current_and_later_events() {
        let session =
            Session::create(dsh_session::session_id("retry-prefix"), None, None, None).unwrap();
        for event in [
            protocol_event("turn/start", json!({"turn":1})),
            protocol_event("step/start", json!({"turn":1,"step":1})),
            protocol_event(
                "request/header",
                json!({"header":{"config":{"provider":"account-a"}}}),
            ),
        ] {
            session.append(&event.type_, event.data, None).unwrap();
        }
        let retry = retry_event("account-a", "policy-a", "retry-a", 1);
        let appended = session.append(&retry.type_, retry.data, None).unwrap();
        session
            .append("step/end", json!({"turn":1,"step":1}), None)
            .unwrap();
        session.append("turn/end", json!({"turn":1}), None).unwrap();
        let mut history = RetryEvidence::default();
        session
            .visit_events(0, Some(appended.seq.get()), |event| {
                history.observe(event);
                Ok(true)
            })
            .unwrap();
        assert!(checked_errors(&history, &appended).is_empty());
    }

    #[test]
    fn many_large_headers_keep_only_current_provider_with_bounded_extra_peak() {
        let mut event = SessionEvent {
            type_: "request/header".into(),
            seq: dsh_session::SessionSeq::new(0).unwrap(),
            time: 0,
            data: json!({"header":{"config":{"provider":"account-a","model":"model"},"tools":[{"name":"probe","description":"x".repeat(1024*1024),"parameters":{"type":"object"}}]},"reason":"change"}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        };
        let mut evidence = RetryEvidence::default();
        ALLOCATION_COUNT.with(|count| {
            count.set(AllocationCount {
                active: true,
                live: 0,
                peak: 0,
            })
        });
        for index in 0..512 {
            event.seq = dsh_session::SessionSeq::new(index).unwrap();
            evidence.observe(&event);
        }
        let peak = ALLOCATION_COUNT.with(|count| {
            let mut value = count.get();
            value.active = false;
            count.set(value);
            value.peak
        });
        assert!(
            peak < 64 * 1024,
            "header evidence duplicated historical payloads: peak={peak}"
        );
        assert!(
            evidence.facts.is_empty(),
            "headers are not retained as history records"
        );
        assert_eq!(evidence.provider.as_deref(), Some("account-a"));
        event.data["header"]["config"]["provider"] = json!("account-b");
        evidence.observe(&event);
        assert_eq!(evidence.provider.as_deref(), Some("account-b"));
        println!("retry header evidence extra peak bytes: {peak}");
    }

    #[test]
    fn archived_retry_protocol_keeps_route_chain_and_started_pairing() {
        let session =
            Session::create(dsh_session::session_id("retry-archive"), None, None, None).unwrap();
        for (kind, data) in [
            ("turn/start", json!({"turn":1})),
            ("step/start", json!({"turn":1,"step":1})),
            (
                "request/header",
                json!({"header":{"config":{"provider":"account-a","model":"model"}},"reason":"initial"}),
            ),
            (
                "request/phase",
                json!({"turn":1,"step":1,"phase":"streaming","detail":"unrelated payload".repeat(4096)}),
            ),
            (
                "llm/retry",
                json!({"turn":1,"step":1,"retryId":"retry-a","retry":1,"provider":"account-a","policyKey":"normal-policy","mode":"normal","maxRetries":2,"delayMs":1,"failure":{"message":"temporary failure","code":"TRANSIENT"}}),
            ),
            (
                "llm/retry-started",
                json!({"turn":1,"step":1,"retryId":"retry-a","retry":1}),
            ),
        ] {
            session.append(kind, data, None).unwrap();
        }
        let mut builder =
            dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        session
            .visit_events(0, None, |event| {
                builder.push(event)?;
                Ok(true)
            })
            .unwrap();
        let archived = Session::from_event_archive(
            session.id().clone(),
            builder.finish().unwrap(),
            session.header(),
            session.inherited_event_count(),
            vec![],
        )
        .unwrap();
        let errors = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let observed = errors.clone();
        let fail: Arc<dyn Fn(&str) + Send + Sync> =
            Arc::new(move |error| observed.lock().push(error.into()));
        validate_session(&archived, &fail);
        let reported = errors.lock().clone();
        assert!(reported.is_empty(), "{reported:?}");
        archived
            .append(
                "llm/retry-started",
                json!({"turn":1,"step":1,"retryId":"retry-a","retry":1}),
                None,
            )
            .unwrap();
        validate_session(&archived, &fail);
        assert!(
            errors
                .lock()
                .iter()
                .any(|error| error.contains("repeats one scheduled attempt"))
        );
    }
}
