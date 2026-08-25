//! Log-backed session title service, deterministic fallback, and provider
//! contract. Rust port of `packages/session/session-title/src/index.ts`.
//!
//! # Deviations
//!
//! - The TS `AbortController`/`AbortSignal.any` composition becomes the
//!   seam-local [`SessionTitleSignal`] (String abort reasons, first reason
//!   wins); the service fuses controllers through forwarding tasks.
//! - Deferred work is spawned through [`spawn_detached`]: a tokio task when a
//!   runtime is current (session/event observers run inside the caller's
//!   task) and a dedicated thread otherwise (the `llm/stream` waterfall
//!   listener runs on dsh-llm's dedicated thread).
//! - The `SessionTitleProvider` trait replaces the TS structural provider
//!   validation: non-object providers, invalid `automatic` modes, and
//!   missing `generate` are inexpressible; the id non-emptiness check and
//!   the duplicate-registration rejection remain runtime checks.
//! - Session identity in the work table is the [`dsh_session::Session::identity`]
//!   pointer (the TS `Map<Session, …>` object-identity key); entries are
//!   dropped through `session/disposed`.
//! - `register` publishes the provider registration synchronously and uses
//!   the caller-context effect only for teardown (the same pattern as the
//!   projection registry), because cordis effect setup is asynchronous
//!   while the TS generator effect publishes synchronously.

use std::collections::HashMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, resume_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context as TaskContext, Poll};

use async_trait::async_trait;
use futures::FutureExt;
use parking_lot::Mutex;
use serde_json::Value as JsonValue;
use tokio::sync::oneshot;

use cordis::{
    ArcValue, Context, Disposer, EventOptions, FiberState, InjectSpec, Listener, NextFn, Plugin,
    PluginError, Service, arc, downcast, downcast_arc, make_disposer,
};
use dsh_llm::{GenerateOptions, is_agent_loop_request};
use dsh_session::{Session, SessionEvent, SessionStore, session_id};
use dsh_session_projection::{
    ProjectionDefinition, SessionProjectionRegistry, types::ProjectionValue,
};

use crate::normalize::{fallback_session_title, normalize_session_title};
use crate::types::{
    Config, SessionTitleAutomaticMode, SessionTitleInvalidError, SessionTitleModelProvenance,
    SessionTitleProvider, SessionTitleProviderRequest, SessionTitleProviderResult,
    SessionTitleSignal, SessionTitleSnapshot, SessionTitleSource, SessionTitleUserMessage,
};

/// Cordis plugin name (TS `SessionTitleService` as a plugin).
pub const NAME: &str = "session-title";

/// The `sessions` service gates the whole plugin (TS `static inject`).
pub const INJECT: [&str; 1] = ["sessions"];

/// Spawn one detached future with the narrowest working executor: the
/// current tokio runtime when present, a dedicated thread otherwise (the
/// dsh-llm `llm/stream` waterfall runs outside any tokio runtime).
fn spawn_detached<F>(future: F)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(async move {
                future.await;
            });
        }
        Err(_) => {
            std::thread::spawn(move || {
                futures::executor::block_on(future);
            });
        }
    }
}

/// A retained background task whose output the caller can await (the TS
/// `track`ed promise). Panics inside the tracked future are captured and
/// rethrown at the awaiting call site.
pub struct Tracked<T> {
    rx: oneshot::Receiver<std::thread::Result<T>>,
}

impl<T> Future for Tracked<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<T> {
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(Ok(value))) => Poll::Ready(value),
            Poll::Ready(Ok(Err(payload))) => resume_unwind(payload),
            Poll::Ready(Err(_)) => {
                panic!("session-title tracked task vanished before completion")
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A completion counter with a wakeup channel (the TS `Set<Promise>` drain).
struct Inflight {
    count: AtomicUsize,
    notify: tokio::sync::Notify,
}

impl Inflight {
    fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn increment(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn decrement(&self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    async fn drain(&self) {
        loop {
            if self.count.load(Ordering::SeqCst) == 0 {
                return;
            }
            self.notify.notified().await;
        }
    }
}

/// One exact provider registration generation (TS `ProviderRegistration`).
struct ProviderRegistration {
    provider: Arc<dyn SessionTitleProvider>,
    active: Arc<Inflight>,
    closing: AtomicBool,
}

/// Automatic work waiting for the matching main-request header (TS
/// `PendingAutomaticWork`).
#[derive(Clone)]
struct PendingAutomaticWork {
    registration: Arc<ProviderRegistration>,
    revision: u64,
    through_seq: u64,
}

/// Provider call currently allowed to commit for one session (TS
/// `ActiveProviderWork`).
struct ActiveProviderWork {
    registration: Arc<ProviderRegistration>,
    revision: u64,
    through_seq: u64,
    controller: SessionTitleSignal,
    signal: SessionTitleSignal,
}

/// Mutable concurrency state scoped to one live session (TS
/// `SessionTitleWorkState`).
#[derive(Default)]
struct WorkState {
    revision: u64,
    fallback:
        Option<futures::future::Shared<Tracked<Result<Option<SessionTitleSnapshot>, String>>>>,
    pending: Option<PendingAutomaticWork>,
    active: Option<Arc<ActiveProviderWork>>,
}

/// Rejection of [`SessionTitleService::rename`]: the invalid-title branch
/// carries the narrowable [`SessionTitleInvalidError`], liveness failures
/// stay plain strings (TS mirrors this split through two error classes).
#[derive(Debug)]
pub enum RenameFailure {
    Invalid(SessionTitleInvalidError),
    Error(String),
}

impl std::fmt::Display for RenameFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameFailure::Invalid(error) => write!(f, "{error}"),
            RenameFailure::Error(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RenameFailure {}

impl RenameFailure {
    pub fn is_invalid(&self) -> bool {
        matches!(self, RenameFailure::Invalid(_))
    }
}

/// One validated provider output accepted for the log (TS `validateResult`).
struct AcceptedTitle {
    title: String,
    message_seqs: Vec<u64>,
    model: Option<SessionTitleModelProvenance>,
}

/// Validate one positive integer configuration field (TS `assertPositiveInteger`).
fn assert_positive_integer(name: &str, value: u64) -> Result<(), String> {
    if value == 0 {
        return Err(format!("session-title: {name} must be a positive integer"));
    }
    Ok(())
}

/// The `session/title` event data builder.
pub fn title_event_data(
    title: &str,
    message_seqs: Vec<u64>,
    source: &SessionTitleSource,
) -> JsonValue {
    serde_json::json!({ "title": title, "messageSeqs": message_seqs, "source": source })
}

/// Collect human text-bearing user messages in log order (TS
/// `collectSessionTitleMessages`).
pub fn collect_session_title_messages(
    events: &[SessionEvent],
    through_seq: Option<u64>,
) -> Vec<SessionTitleUserMessage> {
    let mut messages = Vec::new();
    for event in events {
        if through_seq.is_some_and(|seq| event.seq > seq) {
            break;
        }
        if event.type_ != "user/message" {
            continue;
        }
        let data = &event.data;
        if data
            .get("source")
            .and_then(|source| source.get("kind"))
            .and_then(|kind| kind.as_str())
            != Some("user")
        {
            continue;
        }
        let mut parts = Vec::new();
        if let Some(content) = data.get("content").and_then(|content| content.as_array()) {
            for block in content {
                if block.get("type").and_then(|kind| kind.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|text| text.as_str()) {
                        parts.push(text);
                    }
                }
            }
        }
        let text = parts.join("\n");
        if normalize_session_title(&text, u64::MAX).is_empty() {
            continue;
        }
        messages.push(SessionTitleUserMessage {
            seq: event.seq,
            text,
        });
    }
    messages
}

/// Fold the latest logged title without consulting mutable metadata (TS
/// `foldSessionTitle`).
pub fn fold_session_title(events: &[SessionEvent]) -> Option<SessionTitleSnapshot> {
    let event = events
        .iter()
        .rev()
        .find(|event| event.type_ == "session/title")?;
    let title = event.data.get("title")?.as_str()?.to_string();
    let message_seqs = event
        .data
        .get("messageSeqs")?
        .as_array()?
        .iter()
        .map(|seq| seq.as_u64())
        .collect::<Option<Vec<u64>>>()?;
    let source: SessionTitleSource =
        serde_json::from_value(event.data.get("source")?.clone()).ok()?;
    Some(SessionTitleSnapshot {
        title,
        message_seqs,
        source,
        event_seq: event.seq,
        updated_at: event.time,
    })
}

/// The `title` projection unit (TS registration inside the constructor):
/// pure last-wins fold of `session/title` events.
pub fn title_projection_definition() -> ProjectionDefinition {
    let init: Arc<dyn Fn() -> ArcValue + Send + Sync> = Arc::new(|| arc(JsonValue::Null));
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> =
        Arc::new(|state, event| {
            if event.type_ == "session/title" {
                arc(event.data.get("title").cloned().unwrap_or(JsonValue::Null))
            } else {
                state.clone()
            }
        });
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(|state| state.clone());
    let schema: Arc<dyn Fn(&ArcValue) -> Result<ProjectionValue, String> + Send + Sync> =
        Arc::new(|value| {
            let json: &JsonValue = downcast(value).expect("title projection state must be JSON");
            match json {
                JsonValue::Null => Ok(JsonValue::Null),
                JsonValue::String(text) if !text.is_empty() => Ok(JsonValue::String(text.clone())),
                other => Err(format!(
                    "title projection view must be a non-empty string or null, got {other}"
                )),
            }
        });
    ProjectionDefinition {
        key: "title".to_string(),
        schema,
        init,
        apply,
        view,
        state_version: 1,
    }
}

/// Build one compact navigation row from a human-authored prompt.
pub fn user_message_rail_row(event: &SessionEvent) -> Option<JsonValue> {
    if event.type_ != "user/message"
        || event
            .data
            .get("source")
            .and_then(|source| source.get("kind"))
            .and_then(JsonValue::as_str)
            != Some("user")
    {
        return None;
    }
    let mut text = String::new();
    let mut images = 0_u64;
    if let Some(content) = event.data.get("content").and_then(JsonValue::as_array) {
        for block in content {
            match block.get("type").and_then(JsonValue::as_str) {
                Some("text") => {
                    if let Some(value) = block.get("text").and_then(JsonValue::as_str) {
                        text.push_str(value);
                    }
                }
                Some("image") => images += 1,
                _ => {}
            }
        }
    }
    let preview: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect();
    Some(serde_json::json!({
        "key": event.data.get("id").and_then(JsonValue::as_str).unwrap_or(""),
        "seq": event.seq,
        "text": preview,
        "images": images,
    }))
}

/// Linear-time full-log fold used to seed a missing projection checkpoint.
pub fn user_message_rail_rows(events: &[SessionEvent]) -> JsonValue {
    JsonValue::Array(events.iter().filter_map(user_message_rail_row).collect())
}

pub const USER_MESSAGE_RAIL_KEY: &str = "userMessageRail";
pub const USER_MESSAGE_RAIL_STATE_VERSION: u64 = 2;

/// Lightweight user-message index for navigation without loading transcript pages.
pub fn user_message_rail_projection_definition() -> ProjectionDefinition {
    let init: Arc<dyn Fn() -> ArcValue + Send + Sync> =
        Arc::new(|| arc(JsonValue::Array(Vec::new())));
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> =
        Arc::new(|state, event| {
            if event.type_ != "user/message"
                || event
                    .data
                    .get("source")
                    .and_then(|source| source.get("kind"))
                    .and_then(JsonValue::as_str)
                    != Some("user")
            {
                return state.clone();
            }
            let mut rows = downcast::<JsonValue>(state)
                .and_then(JsonValue::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(row) = user_message_rail_row(event) {
                rows.push(row);
            }
            arc(JsonValue::Array(rows))
        });
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(|state| state.clone());
    let schema = Arc::new(|value: &ArcValue| {
        let json = downcast::<JsonValue>(value)
            .ok_or_else(|| "userMessageRail projection must be JSON".to_string())?;
        if !json.is_array() {
            return Err("userMessageRail projection must be an array".to_string());
        }
        Ok(json.clone())
    });
    ProjectionDefinition {
        key: USER_MESSAGE_RAIL_KEY.to_string(),
        schema,
        init,
        apply,
        view,
        state_version: USER_MESSAGE_RAIL_STATE_VERSION,
    }
}

pub const MODEL_SELECTION_KEY: &str = "modelSelection";
pub const MODEL_SELECTION_STATE_VERSION: u64 = 1;

/// Fixed-size persisted model selection. Explicit `model/selection` events
/// permanently take precedence over request headers, matching session.models.
pub fn model_selection_projection_definition() -> ProjectionDefinition {
    let init: Arc<dyn Fn() -> ArcValue + Send + Sync> =
        Arc::new(|| arc(serde_json::json!({ "explicit": false, "selection": null })));
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> = Arc::new(
        |state, event| {
            let mut value = downcast::<JsonValue>(state)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "explicit": false, "selection": null }));
            if event.type_ == "model/selection" {
                if event
                    .data
                    .get("provider")
                    .and_then(JsonValue::as_str)
                    .is_some()
                    && event
                        .data
                        .get("model")
                        .and_then(JsonValue::as_str)
                        .is_some()
                {
                    value["explicit"] = JsonValue::Bool(true);
                    value["selection"] = event.data.clone();
                }
            } else if event.type_ == "request/header"
                && value.get("explicit").and_then(JsonValue::as_bool) != Some(true)
                && let Some(config) = event
                    .data
                    .get("header")
                    .and_then(|header| header.get("config"))
                && config.get("provider").and_then(JsonValue::as_str).is_some()
                && config.get("model").and_then(JsonValue::as_str).is_some()
            {
                value["selection"] = serde_json::json!({
                    "provider": config.get("provider").cloned().unwrap_or(JsonValue::Null),
                    "model": config.get("model").cloned().unwrap_or(JsonValue::Null),
                    "reasoningEffort": config.get("reasoningEffort").cloned().unwrap_or(JsonValue::Null),
                });
            }
            arc(value)
        },
    );
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(|state| {
        arc(downcast::<JsonValue>(state)
            .and_then(|value| value.get("selection"))
            .cloned()
            .unwrap_or(JsonValue::Null))
    });
    let schema = Arc::new(|value: &ArcValue| {
        let json = downcast::<JsonValue>(value)
            .ok_or_else(|| "modelSelection projection must be JSON".to_string())?;
        if json.is_null()
            || (json.get("provider").is_some_and(JsonValue::is_string)
                && json.get("model").is_some_and(JsonValue::is_string))
        {
            Ok(json.clone())
        } else {
            Err("modelSelection projection has an invalid shape".to_string())
        }
    });
    ProjectionDefinition {
        key: MODEL_SELECTION_KEY.to_string(),
        schema,
        init,
        apply,
        view,
        state_version: MODEL_SELECTION_STATE_VERSION,
    }
}

pub const SESSION_LIST_METADATA_KEY: &str = "sessionListMetadata";
pub const SESSION_LIST_METADATA_STATE_VERSION: u64 = 1;

/// Fixed-size session-list state. The fold never retains event payloads.
pub fn session_list_metadata_projection_definition() -> ProjectionDefinition {
    let init: Arc<dyn Fn() -> ArcValue + Send + Sync> =
        Arc::new(|| arc(serde_json::json!({ "blank": true, "updatedAt": null })));
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> =
        Arc::new(|state, event| match event.type_.as_str() {
            "turn/start" => {
                let mut value = downcast::<JsonValue>(state)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({ "blank": true, "updatedAt": null }));
                value["blank"] = JsonValue::Bool(false);
                arc(value)
            }
            "user/message" => {
                let mut value = downcast::<JsonValue>(state)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({ "blank": true, "updatedAt": null }));
                value["updatedAt"] = JsonValue::from(event.time);
                arc(value)
            }
            _ => state.clone(),
        });
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(|state| state.clone());
    let schema = Arc::new(|value: &ArcValue| {
        let json = downcast::<JsonValue>(value)
            .ok_or_else(|| "sessionListMetadata projection must be JSON".to_string())?;
        let valid = json.get("blank").is_some_and(JsonValue::is_boolean)
            && json
                .get("updatedAt")
                .is_some_and(|value| value.is_null() || value.is_i64() || value.is_u64());
        if !valid {
            return Err("sessionListMetadata projection has an invalid shape".to_string());
        }
        Ok(json.clone())
    });
    ProjectionDefinition {
        key: SESSION_LIST_METADATA_KEY.to_string(),
        schema,
        init,
        apply,
        view,
        state_version: SESSION_LIST_METADATA_STATE_VERSION,
    }
}

#[cfg(test)]
mod session_list_metadata_tests {
    use super::*;
    use cordis::downcast;

    fn event(seq: u64, time: i64, type_: &str) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time,
            data: serde_json::json!({}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    #[test]
    fn model_selection_projection_prefers_explicit_selection_over_request_headers() {
        let definition = model_selection_projection_definition();
        let mut state = (definition.init)();
        let mut header = event(0, 10, "request/header");
        header.data = serde_json::json!({
            "header": { "config": { "provider": "header", "model": "h1", "reasoningEffort": "low" } }
        });
        state = (definition.apply)(&state, &header);
        let mut selected = event(1, 20, "model/selection");
        selected.data = serde_json::json!({
            "provider": "explicit", "model": "e1", "reasoningEffort": "high"
        });
        state = (definition.apply)(&state, &selected);
        let mut later_header = event(2, 30, "request/header");
        later_header.data = serde_json::json!({
            "header": { "config": { "provider": "header", "model": "h2" } }
        });
        state = (definition.apply)(&state, &later_header);
        let view = (definition.view)(&state);
        let value = downcast::<JsonValue>(&view).expect("model selection must be JSON");
        assert_eq!(
            value,
            &serde_json::json!({
                "provider": "explicit", "model": "e1", "reasoningEffort": "high"
            })
        );
    }

    #[test]
    fn folds_blank_and_latest_user_time_in_constant_state() {
        let definition = session_list_metadata_projection_definition();
        let mut state = (definition.init)();
        for row in [
            event(0, 10, "assistant/chunk"),
            event(1, 20, "turn/start"),
            event(2, 30, "user/message"),
            event(3, 40, "tool/result"),
            event(4, 50, "user/message"),
        ] {
            state = (definition.apply)(&state, &row);
        }
        let view = (definition.view)(&state);
        let value = downcast::<JsonValue>(&view).expect("metadata projection must be JSON");
        assert_eq!(
            value,
            &serde_json::json!({ "blank": false, "updatedAt": 50 })
        );
    }
}

/// Log-backed title fold plus asynchronous fallback generation (TS
/// `SessionTitleService`).
pub struct SessionTitleService {
    ctx: Context,
    config: Config,
    owner_fiber: Arc<cordis::FiberCore>,
    lifetime: SessionTitleSignal,
    registration: Mutex<Option<Arc<ProviderRegistration>>>,
    work: Mutex<HashMap<usize, Arc<Mutex<WorkState>>>>,
    in_flight: Arc<Inflight>,
    projection_disposers: Arc<Mutex<Vec<Disposer>>>,
}

impl Service for SessionTitleService {
    fn service_name(&self) -> &'static str {
        "sessionTitle"
    }
}

impl SessionTitleService {
    /// Create the service, register it as `ctx.sessionTitle`, and wire the
    /// lifecycle, projection, session, and llm-stream listeners (TS
    /// constructor). Configuration rejection becomes `Err`.
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        Self::install_with_registry(ctx, config, None)
    }

    pub fn install_with_registry(
        ctx: &Context,
        config: Config,
        explicit_registry: Option<Arc<SessionProjectionRegistry>>,
    ) -> Result<Arc<Self>, String> {
        assert_positive_integer("fallbackMaxWords", config.fallback_max_words)?;
        assert_positive_integer("fallbackMaxBytes", config.fallback_max_bytes)?;
        assert_positive_integer("maxTitleBytes", config.max_title_bytes)?;
        if config.fallback_max_bytes > config.max_title_bytes {
            return Err(
                "session-title: fallbackMaxBytes must not exceed maxTitleBytes".to_string(),
            );
        }

        let service = Arc::new(Self {
            ctx: ctx.clone(),
            config,
            owner_fiber: ctx.fiber.clone(),
            lifetime: SessionTitleSignal::new(),
            registration: Mutex::new(None),
            work: Mutex::new(HashMap::new()),
            in_flight: Arc::new(Inflight::new()),
            projection_disposers: Arc::new(Mutex::new(Vec::new())),
        });
        ctx.register_service(service.clone());

        // Lifecycle: abort, clear pending work, and drain on fiber unload.
        let lifecycle_service = service.clone();
        let _ = ctx.effect(
            "sessionTitle lifecycle",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let service = lifecycle_service.clone();
                    Box::pin(async move {
                        service.dispose_service().await;
                    })
                }))
            }),
        );

        // The Host composes the registry before this service. Register
        // synchronously in that common path so the first list/history request
        // cannot race an inject fiber; retain injection for alternate orders.
        if let Some(registry) = explicit_registry.or_else(|| {
            ctx.get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
                .map(|slot| slot.as_ref().clone())
        }) {
            let title_disposer = registry
                .register(ctx, title_projection_definition())
                .map_err(|error| format!("session-title projection: {error}"))?;
            let rail_disposer = registry
                .register(ctx, user_message_rail_projection_definition())
                .map_err(|error| format!("user-message-rail projection: {error}"))?;
            let metadata_disposer = registry
                .register(ctx, session_list_metadata_projection_definition())
                .map_err(|error| format!("session-list-metadata projection: {error}"))?;
            let model_disposer = registry
                .register(ctx, model_selection_projection_definition())
                .map_err(|error| format!("model-selection projection: {error}"))?;
            service.projection_disposers.lock().extend([
                title_disposer,
                rail_disposer,
                metadata_disposer,
                model_disposer,
            ]);
        } else {
            let projection_disposers = service.projection_disposers.clone();
            ctx.inject(
                InjectSpec::new(["sessionProjections"]),
                Arc::new(move |projection_ctx: &Context, _config: ArcValue| {
                    let projection_ctx = projection_ctx.clone();
                    let projection_disposers = projection_disposers.clone();
                    Box::pin(async move {
                        let registry: Arc<Arc<SessionProjectionRegistry>> = projection_ctx
                            .get_typed::<Arc<SessionProjectionRegistry>>(
                                "sessionProjections",
                                false,
                            )
                            .ok_or_else(|| {
                                PluginError::new(arc(
                                    "sessionProjections service is not configured".to_string(),
                                ))
                            })?;
                        let title_disposer = registry
                            .register(&projection_ctx, title_projection_definition())
                            .map_err(|error| PluginError::new(arc(error)))?;
                        let rail_disposer = registry
                            .register(&projection_ctx, user_message_rail_projection_definition())
                            .map_err(|error| PluginError::new(arc(error)))?;
                        let metadata_disposer = registry
                            .register(
                                &projection_ctx,
                                session_list_metadata_projection_definition(),
                            )
                            .map_err(|error| PluginError::new(arc(error)))?;
                        let model_disposer = registry
                            .register(&projection_ctx, model_selection_projection_definition())
                            .map_err(|error| PluginError::new(arc(error)))?;
                        projection_disposers.lock().extend([
                            title_disposer,
                            rail_disposer,
                            metadata_disposer,
                            model_disposer,
                        ]);
                        Ok(())
                    })
                }),
            );
        }

        // session/event: user messages schedule fallback + provider work;
        // request/header releases pending work with its exact route.
        let event_service = service.clone();
        let event_listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let event = downcast::<SessionEvent>(&args[1])
                .expect("event arg")
                .clone();
            let service = event_service.clone();
            Box::pin(async move {
                match event.type_.as_str() {
                    "user/message" => service.on_user_message(&session, &event),
                    "request/header" => service.on_request_header(&session, &event),
                    _ => {}
                }
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "session/event",
            event_listener,
            EventOptions::default(),
        ));

        // llm/stream (global, prepend): start pending work when the marked
        // loop request reuses the logged route unchanged.
        let stream_service = service.clone();
        let stream_listener: Arc<Listener> =
            Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
                let options = args
                    .first()
                    .and_then(|value| downcast_arc::<Arc<Mutex<GenerateOptions>>>(value))
                    .map(|cell| cell.as_ref().clone());
                let next = args.get(1).and_then(|value| downcast_arc::<NextFn>(value));
                let service = stream_service.clone();
                Box::pin(async move {
                    if let Some(options) = options {
                        let snapshot = options.lock().clone();
                        service.on_main_request(&snapshot);
                    }
                    match next {
                        Some(next) => Some(next.call().await),
                        None => None,
                    }
                })
            });
        let _ = futures::executor::block_on(ctx.on(
            "llm/stream",
            stream_listener,
            EventOptions::default().global(true).prepend(true),
        ));

        // session/disposed: abort active work and forget the session state.
        let disposed_service = service.clone();
        let disposed_listener: Arc<Listener> =
            Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
                let session = downcast::<Session>(&args[0]).expect("session arg").clone();
                let service = disposed_service.clone();
                Box::pin(async move {
                    let state = service.work.lock().remove(&session.identity());
                    if let Some(state) = state {
                        let guard = state.lock();
                        if let Some(active) = &guard.active {
                            active
                                .controller
                                .abort("session disposed during title generation");
                        }
                    }
                    None
                })
            });
        let _ = futures::executor::block_on(ctx.on(
            "session/disposed",
            disposed_listener,
            EventOptions::default(),
        ));

        Ok(service)
    }

    /// The `sessions` store the service consumes.
    fn sessions_store(&self) -> Option<Arc<SessionStore>> {
        self.ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|store| store.as_ref().clone())
    }

    /// Read the latest folded title from one live or replayed session (TS
    /// `get`).
    pub fn get(&self, session: &Session) -> Option<SessionTitleSnapshot> {
        fold_session_title(&session.events())
    }

    /// Accept an explicit user title (TS `rename`). Appends a
    /// `session/title` event with the `user` source, which pins the title.
    pub fn rename(
        &self,
        session: &Session,
        title: &str,
    ) -> Result<SessionTitleSnapshot, RenameFailure> {
        self.assert_service_active().map_err(RenameFailure::Error)?;
        let store = self.sessions_store().ok_or_else(|| {
            RenameFailure::Error("sessions service is not configured".to_string())
        })?;
        if store
            .get(session.id())
            .is_none_or(|live| !live.ptr_eq(session))
        {
            return Err(RenameFailure::Error(format!(
                "session \"{}\" is not live in this store",
                session.id()
            )));
        }
        let normalized = normalize_session_title(title, self.config.max_title_bytes);
        if normalized.is_empty() {
            return Err(RenameFailure::Invalid(SessionTitleInvalidError::new(
                "session title must contain visible characters",
            )));
        }
        let state = self.state_for(session);
        self.supersede(&state, "user rename superseded automatic title generation");
        session
            .append(
                "session/title",
                title_event_data(&normalized, Vec::new(), &SessionTitleSource::User),
                None,
            )
            .map_err(|error| {
                RenameFailure::Error(format!("session title append failed: {error}"))
            })?;
        self.get(session)
            .ok_or_else(|| RenameFailure::Error("renamed title failed to fold".to_string()))
    }

    /// Explicitly retry the registered provider, or materialize the built-in
    /// fallback when no provider is registered (TS `refresh`).
    pub async fn refresh(
        self: &Arc<Self>,
        session: &Session,
        signal: Option<&SessionTitleSignal>,
    ) -> Result<Option<SessionTitleSnapshot>, String> {
        if let Some(signal) = signal {
            if let Some(reason) = signal.abort_reason() {
                return Err(reason);
            }
        }
        self.assert_service_active()?;
        let store = self
            .sessions_store()
            .ok_or("sessions service is not configured")?;
        if store
            .get(session.id())
            .is_none_or(|live| !live.ptr_eq(session))
        {
            return Err(format!(
                "session \"{}\" is not live in this store",
                session.id()
            ));
        }
        let registration = self.registration.lock().clone();
        let messages = collect_session_title_messages(&session.events(), None);
        let latest = messages.last().cloned();
        let unusable = registration
            .as_ref()
            .is_none_or(|registration| registration.closing.load(Ordering::SeqCst));
        if unusable || latest.is_none() {
            // Explicit refresh is the unpin even without a provider.
            let current = self.get(session);
            let first = messages.first().cloned();
            if current
                .as_ref()
                .is_some_and(|current| current.source.kind() == "user")
                && first.is_some()
            {
                self.append_fallback(session, &first.unwrap())?;
                if let Some(signal) = signal {
                    if let Some(reason) = signal.abort_reason() {
                        return Err(reason);
                    }
                }
                return Ok(self.get(session));
            }
            let fallback = self.ensure_fallback(session).await;
            if let Some(signal) = signal {
                if let Some(reason) = signal.abort_reason() {
                    return Err(reason);
                }
            }
            return fallback;
        }
        let registration = registration.unwrap();
        let state = self.state_for(session);
        let revision = self.supersede(&state, "explicit title refresh superseded older generation");
        let pending = PendingAutomaticWork {
            registration,
            revision,
            through_seq: latest.unwrap().seq,
        };
        let work = self.activate(pending, &state, signal);
        let route = session
            .request_header()
            .map(|header| SessionTitleModelProvenance {
                provider: header.config.provider,
                model: header.config.model,
            });
        self.start_provider(session, work, route).await
    }

    /// Register the sole optional title provider (TS `register`). Disposal
    /// aborts its pending and active work before another provider may
    /// register.
    pub fn register(
        self: &Arc<Self>,
        caller: &Context,
        provider: Arc<dyn SessionTitleProvider>,
    ) -> Result<Disposer, String> {
        if provider.id().as_str().is_empty() {
            return Err("session-title provider id must be a non-empty string".to_string());
        }
        let registration = Arc::new(ProviderRegistration {
            provider: provider.clone(),
            active: Arc::new(Inflight::new()),
            closing: AtomicBool::new(false),
        });
        {
            let mut slot = self.registration.lock();
            if let Some(existing) = &*slot {
                return Err(format!(
                    "session-title provider \"{}\" is already registered",
                    existing.provider.id()
                ));
            }
            *slot = Some(registration.clone());
        }
        let service = self.clone();
        let dispose = caller.effect(
            "sessionTitle.register()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let service = service.clone();
                    let registration = registration.clone();
                    Box::pin(async move {
                        service.dispose_registration(&registration).await;
                    })
                }))
            }),
        );
        Ok(dispose)
    }

    /// Schedule fallback creation and any provider cadence for one eligible
    /// event (TS `onUserMessage`).
    fn on_user_message(self: &Arc<Self>, session: &Session, event: &SessionEvent) {
        if !self.service_active() {
            return;
        }
        let data = &event.data;
        if data
            .get("source")
            .and_then(|source| source.get("kind"))
            .and_then(|kind| kind.as_str())
            != Some("user")
        {
            return;
        }
        if collect_session_title_messages(&[event.clone()], None).is_empty() {
            return;
        }
        // A user rename pins the title: no automatic revision may override it.
        if self
            .get(session)
            .is_some_and(|current| current.source.kind() == "user")
        {
            return;
        }
        let registration = self.registration.lock().clone();
        if let Some(registration) = registration {
            if !registration.closing.load(Ordering::SeqCst) {
                let messages = collect_session_title_messages(&session.events(), Some(event.seq));
                let should_schedule = registration.provider.automatic()
                    == SessionTitleAutomaticMode::AllPrompts
                    || (session.header().parent_session.is_none()
                        && messages.len() == 1
                        && self.get(session).is_none());
                if should_schedule {
                    let state = self.state_for(session);
                    let revision =
                        self.supersede(&state, "newer user message superseded title generation");
                    state.lock().pending = Some(PendingAutomaticWork {
                        registration,
                        revision,
                        through_seq: event.seq,
                    });
                }
            }
        }
        let service = self.clone();
        let session = session.clone();
        self.defer(async move {
            match service.ensure_fallback(&session).await {
                Ok(_) => {}
                Err(error) => {
                    if !service.service_active() {
                        return;
                    }
                    service
                        .ctx
                        .named_logger(Some("session-title"))
                        .warn(vec![arc(format!(
                            "session \"{}\": fallback title update failed: {error}",
                            session.id()
                        ))]);
                }
            }
        });
    }

    /// Start pending automatic work only after its exact main-request route
    /// is logged (TS `onRequestHeader`).
    fn on_request_header(self: &Arc<Self>, session: &Session, event: &SessionEvent) {
        if !self.service_active() {
            return;
        }
        let Some(state) = self.work.lock().get(&session.identity()).cloned() else {
            return;
        };
        let Some(pending) = state.lock().pending.clone() else {
            return;
        };
        if pending.through_seq >= event.seq {
            return;
        }
        let Some(provider) = event
            .data
            .get("header")
            .and_then(|header| header.get("config"))
            .and_then(|config| config.get("provider"))
            .and_then(|provider| provider.as_str())
        else {
            return;
        };
        let Some(model) = event
            .data
            .get("header")
            .and_then(|header| header.get("config"))
            .and_then(|config| config.get("model"))
            .and_then(|model| model.as_str())
        else {
            return;
        };
        let route = SessionTitleModelProvenance {
            provider: provider.to_string(),
            model: model.to_string(),
        };
        self.start_pending(session, &state, pending, route);
    }

    /// Start unchanged-route work from the marked loop request after its
    /// header fold is current (TS `onMainRequest`).
    fn on_main_request(self: &Arc<Self>, options: &GenerateOptions) {
        if !self.service_active() {
            return;
        }
        let Some(session_id_str) = options.session_id.as_deref() else {
            return;
        };
        if !is_agent_loop_request(options) {
            return;
        }
        let Some(store) = self.sessions_store() else {
            return;
        };
        let Some(session) = store.get(&session_id(session_id_str.to_string())) else {
            return;
        };
        let Some(state) = self.work.lock().get(&session.identity()).cloned() else {
            return;
        };
        let Some(pending) = state.lock().pending.clone() else {
            return;
        };
        let events = session.events();
        let boundary = events
            .iter()
            .rev()
            .find(|event| event.type_ == "step/start" || event.type_ == "step/end");
        let Some(boundary) = boundary else {
            return;
        };
        if boundary.type_ != "step/start" || boundary.seq <= pending.through_seq {
            return;
        }
        let Some(route) = session.request_header().map(|header| header.config) else {
            return;
        };
        if route.provider != options.provider || route.model != options.model {
            return;
        }
        let provenance = SessionTitleModelProvenance {
            provider: options.provider.clone(),
            model: options.model.clone(),
        };
        self.start_pending(&session, &state, pending, provenance);
    }

    /// Consume one pending revision and schedule its non-blocking provider
    /// call (TS `startPending`).
    fn start_pending(
        self: &Arc<Self>,
        session: &Session,
        state: &Arc<Mutex<WorkState>>,
        pending: PendingAutomaticWork,
        route: SessionTitleModelProvenance,
    ) {
        state.lock().pending = None;
        let service = self.clone();
        let session = session.clone();
        let state = state.clone();
        let run_service = service.clone();
        service.defer(async move {
            let registration_current = run_service
                .registration
                .lock()
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &pending.registration));
            if !registration_current
                || pending.registration.closing.load(Ordering::SeqCst)
                || !run_service
                    .work
                    .lock()
                    .get(&session.identity())
                    .is_some_and(|current| Arc::ptr_eq(current, &state))
                || state.lock().revision != pending.revision
            {
                return;
            }
            let work = run_service.activate(pending.clone(), &state, None);
            match run_service
                .start_provider(&session, work.clone(), Some(route))
                .await
            {
                Ok(_) => {}
                Err(error) => {
                    if work.signal.is_aborted() || !run_service.service_active() {
                        return;
                    }
                    run_service
                        .ctx
                        .named_logger(Some("session-title"))
                        .warn(vec![arc(format!(
                            "session \"{}\": automatic title generation failed: {error}",
                            session.id()
                        ))]);
                }
            }
        });
    }

    /// Start one tracked provider call after publishing its active revision
    /// (TS `startProvider`).
    fn start_provider(
        self: &Arc<Self>,
        session: &Session,
        work: Arc<ActiveProviderWork>,
        route: Option<SessionTitleModelProvenance>,
    ) -> Tracked<Result<Option<SessionTitleSnapshot>, String>> {
        let service = Arc::clone(self);
        let session = session.clone();
        let work_for_run = work.clone();
        let run = async move { service.run_provider(&session, &work_for_run, route).await };
        self.track(run, Some(&work.registration))
    }

    /// Execute and accept one current provider revision (TS `runProvider`).
    async fn run_provider(
        self: &Arc<Self>,
        session: &Session,
        work: &Arc<ActiveProviderWork>,
        route: Option<SessionTitleModelProvenance>,
    ) -> Result<Option<SessionTitleSnapshot>, String> {
        let outcome = (async {
            self.assert_current(session, work)?;
            self.ensure_fallback(session).await?;
            self.assert_current(session, work)?;
            let messages =
                collect_session_title_messages(&session.events(), Some(work.through_seq));
            let result = work
                .registration
                .provider
                .generate(SessionTitleProviderRequest {
                    session: session.clone(),
                    messages: messages.clone(),
                    route: route.clone(),
                    signal: work.signal.clone(),
                })
                .await
                .map_err(|error| error.message)?;
            self.assert_current(session, work)?;
            let accepted = self.validate_result(result, &messages)?;
            let data = title_event_data(
                &accepted.title,
                accepted.message_seqs.clone(),
                &SessionTitleSource::Provider {
                    provider: work.registration.provider.id().clone(),
                    model: accepted.model.clone(),
                },
            );
            session
                .append("session/title", data, None)
                .map_err(|error| format!("session title append failed: {error}"))?;
            Ok(self.get(session))
        })
        .await;
        if let Some(state) = self.work.lock().get(&session.identity()).cloned() {
            let mut guard = state.lock();
            if guard
                .active
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, work))
            {
                guard.active = None;
            }
        }
        outcome
    }

    /// Validate and normalize provider output against the supplied message
    /// snapshot (TS `validateResult`).
    fn validate_result(
        &self,
        result: SessionTitleProviderResult,
        messages: &[SessionTitleUserMessage],
    ) -> Result<AcceptedTitle, String> {
        let title = normalize_session_title(&result.title, self.config.max_title_bytes);
        if title.is_empty() {
            return Err("session-title provider returned an empty title".to_string());
        }
        if result.message_seqs.is_empty() {
            return Err(
                "session-title provider must identify at least one source message seq".to_string(),
            );
        }
        let order: HashMap<u64, usize> = messages
            .iter()
            .enumerate()
            .map(|(index, message)| (message.seq, index))
            .collect();
        let mut previous: isize = -1;
        for seq in &result.message_seqs {
            match order.get(seq) {
                Some(&index) if (index as isize) > previous => previous = index as isize,
                _ => {
                    return Err(
                        "session-title provider messageSeqs must be unique, ordered seqs from the request"
                            .to_string(),
                    )
                }
            }
        }
        let model = match &result.model {
            None => None,
            Some(model) => {
                if model.provider.is_empty() || model.model.is_empty() {
                    return Err(
                        "session-title provider result model must contain non-empty provider and model strings"
                            .to_string(),
                    );
                }
                Some(model.clone())
            }
        };
        Ok(AcceptedTitle {
            title,
            message_seqs: result.message_seqs,
            model,
        })
    }

    /// Fail a completion whose provider, revision, session, or signal is
    /// stale (TS `assertCurrent`).
    fn assert_current(
        &self,
        session: &Session,
        work: &Arc<ActiveProviderWork>,
    ) -> Result<(), String> {
        self.assert_service_active()?;
        if let Some(reason) = work.signal.abort_reason() {
            return Err(reason);
        }
        let state = self.work.lock().get(&session.identity()).cloned();
        let active_matches = state
            .as_ref()
            .and_then(|state| state.lock().active.clone())
            .is_some_and(|active| Arc::ptr_eq(&active, work));
        let registration_matches = self
            .registration
            .lock()
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &work.registration));
        let revision_matches = state
            .as_ref()
            .is_some_and(|state| state.lock().revision == work.revision);
        let live = self.sessions_store().is_some_and(|store| {
            store
                .get(session.id())
                .is_some_and(|live| live.ptr_eq(session))
        });
        if !registration_matches || !active_matches || !revision_matches || !live {
            return Err("session title generation state changed without cancellation".to_string());
        }
        Ok(())
    }

    /// Create and publish an active provider call from one fixed revision
    /// (TS `activate`).
    fn activate(
        &self,
        pending: PendingAutomaticWork,
        state: &Arc<Mutex<WorkState>>,
        upstream: Option<&SessionTitleSignal>,
    ) -> Arc<ActiveProviderWork> {
        let controller = SessionTitleSignal::new();
        let signal = self.compose_signal(&controller, upstream);
        let work = Arc::new(ActiveProviderWork {
            registration: pending.registration,
            revision: pending.revision,
            through_seq: pending.through_seq,
            controller,
            signal,
        });
        state.lock().active = Some(work.clone());
        work
    }

    /// Fuse the controller with the service lifetime and the optional
    /// caller signal (TS `AbortSignal.any([controller.signal,
    /// lifetime.signal, upstream])`). The synchronous aborted predicate
    /// scans the fused sources; forwarding tasks additionally mirror each
    /// source abort onto the fused signal so `cancelled()` waiters wake.
    fn compose_signal(
        &self,
        controller: &SessionTitleSignal,
        upstream: Option<&SessionTitleSignal>,
    ) -> SessionTitleSignal {
        let mut sources: Vec<SessionTitleSignal> = vec![self.lifetime.clone()];
        if let Some(upstream) = upstream {
            sources.push(upstream.clone());
        }
        let fused = controller.fused_with(sources.clone());
        let mut forward_sources = vec![controller.clone()];
        forward_sources.extend(sources);
        for source in forward_sources {
            let fused_for_task = fused.clone();
            spawn_detached(async move {
                source.cancelled().await;
                let reason = source
                    .abort_reason()
                    .unwrap_or_else(|| "session title work aborted".to_string());
                fused_for_task.abort(reason);
            });
        }
        fused
    }

    /// Abort older active work and reserve the next session-local revision
    /// (TS `supersede`).
    fn supersede(&self, state: &Arc<Mutex<WorkState>>, reason: &str) -> u64 {
        let mut guard = state.lock();
        if let Some(active) = &guard.active {
            active.controller.abort(reason.to_string());
        }
        guard.pending = None;
        guard.revision += 1;
        guard.revision
    }

    /// Return mutable work state for one session (TS `stateFor`).
    fn state_for(&self, session: &Session) -> Arc<Mutex<WorkState>> {
        self.work
            .lock()
            .entry(session.identity())
            .or_insert_with(|| Arc::new(Mutex::new(WorkState::default())))
            .clone()
    }

    /// Queue detached service work and retain it through service disposal
    /// (TS `defer`).
    fn defer(self: &Arc<Self>, task: impl Future<Output = ()> + Send + 'static) {
        let service = self.clone();
        let run = async move {
            if !service.service_active() {
                return;
            }
            task.await;
        };
        self.track(run, None);
    }

    /// Retain one future until settlement for service and optional provider
    /// teardown (TS `track`).
    fn track<T>(
        &self,
        run: impl Future<Output = T> + Send + 'static,
        registration: Option<&Arc<ProviderRegistration>>,
    ) -> Tracked<T>
    where
        T: Send + 'static,
    {
        self.in_flight.increment();
        let registration_active = registration.map(|registration| registration.active.clone());
        if let Some(active) = &registration_active {
            active.increment();
        }
        let (tx, rx) = oneshot::channel();
        let inflight = self.in_flight.clone();
        spawn_detached(async move {
            let result = AssertUnwindSafe(run).catch_unwind().await;
            inflight.decrement();
            if let Some(active) = &registration_active {
                active.decrement();
            }
            let _ = tx.send(result);
        });
        Tracked { rx }
    }

    /// Abort pending and active work of one provider registration and drain
    /// its calls (TS `register` disposer).
    async fn dispose_registration(&self, registration: &Arc<ProviderRegistration>) {
        registration.closing.store(true, Ordering::SeqCst);
        let states: Vec<Arc<Mutex<WorkState>>> = self.work.lock().values().cloned().collect();
        for state in states {
            let mut guard = state.lock();
            if guard
                .pending
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.registration, registration))
            {
                guard.pending = None;
            }
            if guard
                .active
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(&active.registration, registration))
            {
                if let Some(active) = &guard.active {
                    active.controller.abort(format!(
                        "session-title provider \"{}\" was disposed",
                        registration.provider.id()
                    ));
                }
            }
        }
        registration.active.drain().await;
        let mut slot = self.registration.lock();
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, registration))
        {
            *slot = None;
        }
    }

    /// Service teardown: abort everything and drain retained work (TS
    /// lifecycle effect cleanup).
    async fn dispose_service(&self) {
        self.lifetime.abort("session-title service disposed");
        if let Some(registration) = self.registration.lock().clone() {
            registration.closing.store(true, Ordering::SeqCst);
        }
        *self.registration.lock() = None;
        let states: Vec<Arc<Mutex<WorkState>>> = self.work.lock().values().cloned().collect();
        for state in states {
            let mut guard = state.lock();
            guard.pending = None;
            if let Some(active) = &guard.active {
                active.controller.abort("session-title service disposed");
            }
        }
        self.in_flight.drain().await;
        self.work.lock().clear();
    }

    /// Whether the owning plugin fiber can still start or commit title work
    /// (TS `serviceActive`).
    fn service_active(&self) -> bool {
        !self.lifetime.is_aborted()
            && self.owner_fiber.uid_value().is_some()
            && self.owner_fiber.state() == FiberState::Active
    }

    /// Reject work once the owning plugin fiber has begun unloading (TS
    /// `assertServiceActive`).
    fn assert_service_active(&self) -> Result<(), String> {
        if !self.service_active() {
            return Err("session-title service disposed".to_string());
        }
        Ok(())
    }

    /// Derive and append the deterministic fallback title over whatever
    /// stands (TS `appendFallback`).
    fn append_fallback(
        &self,
        session: &Session,
        first: &SessionTitleUserMessage,
    ) -> Result<(), String> {
        let title = fallback_session_title(
            &first.text,
            self.config.fallback_max_words,
            self.config.fallback_max_bytes,
        );
        if title.is_empty() {
            return Ok(());
        }
        session
            .append(
                "session/title",
                title_event_data(&title, vec![first.seq], &SessionTitleSource::Fallback),
                None,
            )
            .map_err(|error| format!("session title append failed: {error}"))?;
        Ok(())
    }

    /// Create the first deterministic fallback if the session still lacks a
    /// title (TS `ensureFallback`).
    async fn ensure_fallback(
        self: &Arc<Self>,
        session: &Session,
    ) -> Result<Option<SessionTitleSnapshot>, String> {
        self.assert_service_active()?;
        if let Some(current) = self.get(session) {
            return Ok(Some(current));
        }
        let messages = collect_session_title_messages(&session.events(), None);
        let Some(first) = messages.first().cloned() else {
            return Ok(None);
        };
        let title = fallback_session_title(
            &first.text,
            self.config.fallback_max_words,
            self.config.fallback_max_bytes,
        );
        if title.is_empty() {
            return Ok(None);
        }
        let state = self.state_for(session);
        type SharedFallback =
            futures::future::Shared<Tracked<Result<Option<SessionTitleSnapshot>, String>>>;
        let shared: SharedFallback = {
            let mut guard = state.lock();
            match &guard.fallback {
                Some(existing) => existing.clone(),
                None => {
                    let service = self.clone();
                    let session_for_run = session.clone();
                    let first_for_run = first.clone();
                    let title_for_run = title.clone();
                    let run = async move {
                        service.assert_service_active()?;
                        let store = service
                            .sessions_store()
                            .ok_or("sessions service is not configured")?;
                        if store
                            .get(session_for_run.id())
                            .is_none_or(|live| !live.ptr_eq(&session_for_run))
                        {
                            return Err(format!(
                                "session \"{}\" is not live in this store",
                                session_for_run.id()
                            ));
                        }
                        if let Some(accepted) = service.get(&session_for_run) {
                            return Ok(Some(accepted));
                        }
                        session_for_run
                            .append(
                                "session/title",
                                title_event_data(
                                    &title_for_run,
                                    vec![first_for_run.seq],
                                    &SessionTitleSource::Fallback,
                                ),
                                None,
                            )
                            .map_err(|error| format!("session title append failed: {error}"))?;
                        Ok(service.get(&session_for_run))
                    };
                    let tracked = self.track(run, None);
                    let shared = tracked.shared();
                    guard.fallback = Some(shared.clone());
                    shared
                }
            }
        };
        let outcome = shared.await;
        state.lock().fallback = None;
        outcome
    }
}

/// The Cordis plugin form of the service (`name = "session-title"`,
/// `inject = ["sessions"]`).
pub struct SessionTitlePlugin;

#[async_trait]
impl Plugin for SessionTitlePlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = downcast::<Config>(&config).cloned().ok_or_else(|| {
            PluginError::new(arc("session-title: configuration is required".to_string()))
        })?;
        SessionTitleService::install(ctx, config)
            .map(|_| ())
            .map_err(|error| PluginError::new(arc(error)))
    }
}
