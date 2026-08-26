//! Event-sourced session service: append-only session log, in-memory store,
#![allow(clippy::type_complexity)]
// Publication and lifecycle callback tuples intentionally preserve the public event contract.
//! and the derived LLM message history. Rust port of
//! `packages/core/session/src/index.ts`.
//!
//! # Deviations
//!
//! - `Session` is a cloneable `Arc` handle; the store keeps strong refs.
//! - `SessionStore::create`/`announce` are `async`: listener veto panics
//!   propagate as `Err` (the TS synchronous throw boundary).
//! - `Session::append` stays synchronous; observers run fire-and-forget on
//!   the ambient tokio runtime (or inline when none exists), matching the
//!   port's emit semantics.
//! - `deepFreeze`/`structuredClone` collapse to the identity function:
//!   Rust values are owned.
//! - The listener snapshot resolves while the session state lock is held;
//!   `internal/dispatch` listeners must therefore not re-enter the
//!   dispatching session's state (documented).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Weak};

use cordis::{
    ArcValue, Context, DispatchMode, Disposer, InjectSpec, Listener, Service, arc, make_disposer,
};
use dsh_llm::Message;
use dsh_scope::{ScopeCarrier, scope_of, scope_target};
use dsh_typert_protocol::{TypertLookup, TypertService};
use futures::FutureExt;
use parking_lot::Mutex;
use serde_json::{Map, Value as JsonValue};

use crate::json::snapshot_json_value;
use crate::surface::{SurfaceManager, derive_event_message};
use crate::types::{
    CreateSessionOptions, EpochHeader, RequestContext, SessionEvent, SessionHeader, SessionId,
    SurfaceIntent, end_seed_data, session_id, snapshot_session_header, validate_session_header,
};

/// Store attachment keyed by session identity (TS `attachments` WeakMap).
static ATTACHMENTS: LazyLock<Mutex<HashMap<usize, Weak<SessionEntry>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ---- Seed / restore validation ----

/// Validate the fixed event envelope after one-pass JSON materialization
/// (TS `assertSessionEventEnvelope`).
fn assert_session_event_envelope(value: &JsonValue, index: usize) -> Result<(), String> {
    let invalid = || format!("seed event at index {index} has an invalid event envelope");
    let Some(record) = value.as_object() else {
        return Err(invalid());
    };
    if record.get("type").and_then(|value| value.as_str()) == Some("request/header-delta") {
        return Err(format!(
            "seed event at index {index} uses unsupported legacy request/header-delta format"
        ));
    }
    for key in record.keys() {
        match key.as_str() {
            "type" | "seq" | "time" | "data" | "surfaceOp" | "sourceEventSeqs" | "ignorable" => {}
            _ => return Err(invalid()),
        }
    }
    let type_ok = record.get("type").is_some_and(|value| value.is_string());
    let seq_ok = record
        .get("seq")
        .is_some_and(|value| value.as_u64().is_some());
    let time_ok = record
        .get("time")
        .is_some_and(|value| value.as_i64().is_some());
    let data_present = record.contains_key("data");
    let ignorable_ok = match record.get("ignorable") {
        None => true,
        Some(value) => value.as_bool() == Some(true),
    };
    if !type_ok || !seq_ok || !time_ok || !data_present || !ignorable_ok {
        return Err(invalid());
    }
    if let Some("request/header" | "user/message" | "assistant/message" | "tool/result") =
        record.get("type").and_then(|value| value.as_str())
    {
        assert_current_llm_shape(record, index)?;
    }
    Ok(())
}

/// Whether an unknown value carries the current provider/model pair.
fn has_provider_model(value: Option<&JsonValue>) -> bool {
    let Some(pair) = value.and_then(|value| value.as_object()) else {
        return false;
    };
    pair.get("provider")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.is_empty())
        && pair
            .get("model")
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.is_empty())
}

/// Validate adapter-default markers imported from a durable request header.
fn assert_adapter_defaults(
    value: Option<&JsonValue>,
    config: &Map<String, JsonValue>,
    index: usize,
) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    let invalid = || format!("seed request/header at index {index} has invalid adapterDefaults");
    let Some(defaults) = value.as_object() else {
        return Err(invalid());
    };
    let unknown_key = defaults
        .keys()
        .any(|key| key != "reasoningEffort" && key != "maxTokens");
    let non_true_marker = defaults
        .values()
        .any(|marker| marker.as_bool() != Some(true));
    let dangling_effort = defaults
        .get("reasoningEffort")
        .and_then(|marker| marker.as_bool())
        == Some(true)
        && !config.contains_key("reasoningEffort");
    let dangling_max = defaults
        .get("maxTokens")
        .and_then(|marker| marker.as_bool())
        == Some(true)
        && !config.contains_key("maxTokens");
    if unknown_key || non_true_marker || dangling_effort || dangling_max {
        return Err(invalid());
    }
    Ok(())
}

/// Reject obsolete request headers and malformed messages at the seed/load
/// boundary (TS `assertCurrentLlmShape`).
fn assert_current_llm_shape(record: &Map<String, JsonValue>, index: usize) -> Result<(), String> {
    let type_ = record
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let data = record.get("data").and_then(|value| value.as_object());
    if type_ == "request/header" {
        let header = data
            .and_then(|data| data.get("header"))
            .and_then(|header| header.as_object());
        let config = header.and_then(|header| header.get("config"));
        if !has_provider_model(config) {
            return Err(format!(
                "seed request/header at index {index} lacks provider/model"
            ));
        }
        let config_record = config.and_then(|config| config.as_object()).unwrap();
        if let Some(effort) = config_record.get("reasoningEffort")
            && effort.as_str().is_none_or(|effort| effort.is_empty())
        {
            return Err(format!(
                "seed request/header at index {index} has an invalid reasoningEffort"
            ));
        }
        assert_adapter_defaults(
            header.and_then(|header| header.get("adapterDefaults")),
            config_record,
            index,
        )?;
    }
    if type_ != "user/message" && type_ != "assistant/message" && type_ != "tool/result" {
        return Ok(());
    }
    assert_message_event_shape(record, &format!("seed {type_} at index {index}"))
}

/// Validate only the event-specific invariants needed to safely replay a
/// message (TS `assertMessageEventShape`).
fn assert_message_event_shape(event: &Map<String, JsonValue>, subject: &str) -> Result<(), String> {
    let type_ = event
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if type_ != "user/message" && type_ != "assistant/message" && type_ != "tool/result" {
        return Ok(());
    }
    let data_value = event.get("data");
    let data = data_value.and_then(|value| value.as_object());
    let message = match type_ {
        "user/message" => data_value,
        _ => data.and_then(|data| data.get("message")),
    };
    let Some(message_record) = message.and_then(|message| message.as_object()) else {
        return Err(format!("{subject} lacks an identified message"));
    };
    let id = message_record.get("id").and_then(|value| value.as_str());
    if id.is_none_or(|id| id.is_empty()) {
        return Err(format!("{subject} lacks an identified message"));
    }
    let expected_role = if type_ == "assistant/message" {
        "assistant"
    } else {
        "user"
    };
    if message_record.get("role").and_then(|value| value.as_str()) != Some(expected_role) {
        return Err(format!(
            "{subject} message must have role \"{expected_role}\""
        ));
    }
    let source = message_record
        .get("source")
        .and_then(|value| value.as_object());
    let source_kind = source
        .and_then(|source| source.get("kind"))
        .and_then(|value| value.as_str());
    if source_kind.is_none_or(|kind| kind.is_empty()) {
        return Err(format!("{subject} message has invalid source"));
    }
    if message_record
        .get("content")
        .and_then(|value| value.as_array())
        .is_none()
    {
        return Err(format!("{subject} message has invalid content"));
    }
    let source = source.unwrap();
    if type_ == "assistant/message" {
        if source_kind != Some("model")
            || !has_provider_model(Some(&JsonValue::Object(source.clone())))
        {
            return Err(format!("{subject} message must have model source"));
        }
        return Ok(());
    }
    if type_ != "tool/result" {
        return Ok(());
    }
    let call_id = source.get("callId").and_then(|value| value.as_str());
    if source_kind != Some("tool") || call_id.is_none_or(|call_id| call_id.is_empty()) {
        return Err(format!("{subject} message must have tool source"));
    }
    let content = message_record
        .get("content")
        .and_then(|value| value.as_array())
        .expect("content array checked above");
    let block = content.first();
    let block_ok = content.len() == 1
        && block
            .and_then(|block| block.get("type"))
            .and_then(|value| value.as_str())
            == Some("tool-result")
        && block
            .and_then(|block| block.get("content"))
            .and_then(|value| value.as_array())
            .is_some();
    if !block_ok {
        return Err(format!(
            "{subject} message must contain one tool-result block"
        ));
    }
    let tool_call_id = block.and_then(|block| block.get("toolCallId"));
    if tool_call_id != source.get("callId") {
        return Err(format!("{subject} message has mismatched tool call ids"));
    }
    Ok(())
}

/// Reject request-header vocabulary removed with the legacy delta codec.
fn assert_supported_request_header(
    type_: &str,
    data: &JsonValue,
    location: &str,
) -> Result<(), String> {
    if type_ == "request/header-delta" {
        return Err(format!(
            "{location} uses unsupported legacy request/header-delta format"
        ));
    }
    if type_ == "request/header"
        && data.get("reason").and_then(|value| value.as_str()) == Some("fallback")
    {
        return Err(format!(
            "{location} uses unsupported legacy request/header reason \"fallback\""
        ));
    }
    Ok(())
}

// ---- Session ----

/// Mutable log state of one session.
#[derive(Default)]
pub(crate) struct SessionState {
    log: Vec<SessionEvent>,
    surface: SurfaceManager,
    header_fold: Option<EpochHeader>,
    header_fold_seq: usize,
    context_fold: Option<RequestContext>,
    context_fold_seq: usize,
    derived: Arc<Vec<Message>>,
    derived_nodes: usize,
    derived_generation: u64,
}

/// An event-sourced session: an append-only log of [`SessionEvent`]s.
/// Cloneable handle over a shared, lock-guarded state (TS `Session` class).
#[derive(Clone)]
pub struct Session {
    pub(crate) inner: Arc<SessionInner>,
}

pub(crate) struct SessionInner {
    pub id: SessionId,
    pub header: SessionHeader,
    pub first_live_seq: usize,
    pub state: Mutex<SessionState>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.inner.id)
            .finish()
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Session {
    /// Create a detached session by validating and snapshotting borrowed
    /// seed events and storage metadata (TS `Session.create`).
    pub fn create(
        id: SessionId,
        seed: Option<Vec<SessionEvent>>,
        header: Option<&SessionHeader>,
    ) -> Result<Session, String> {
        Self::construct(id, seed, header, false)
    }

    /// Restore a detached session by taking ownership of fresh persistence
    /// values (TS `Session.fromRestore`).
    pub fn from_restore(
        id: SessionId,
        seed: Vec<SessionEvent>,
        header: &SessionHeader,
    ) -> Result<Session, String> {
        Self::construct(id, Some(seed), Some(header), true)
    }

    fn construct(
        id: SessionId,
        seed: Option<Vec<SessionEvent>>,
        header: Option<&SessionHeader>,
        restore: bool,
    ) -> Result<Session, String> {
        let had_seed = seed.is_some();
        let mut state = SessionState::default();
        if let Some(seed) = seed {
            for (index, source) in seed.iter().enumerate() {
                let value = serde_json::to_value(source).map_err(|_| {
                    format!("seed event at index {index} is not losslessly JSON-serializable")
                })?;
                let snapshot_value = if restore {
                    value.clone()
                } else {
                    snapshot_json_value(&value).ok_or_else(|| {
                        format!("seed event at index {index} is not losslessly JSON-serializable")
                    })?
                };
                let snapshot: SessionEvent = serde_json::from_value(snapshot_value.clone())
                    .map_err(|_| {
                        format!("seed event at index {index} has an invalid event envelope")
                    })?;
                assert_session_event_envelope(&snapshot_value, index)?;
                assert_supported_request_header(
                    &snapshot.type_,
                    &snapshot.data,
                    &format!("seed event at index {index}"),
                )?;
                if snapshot.seq != index as u64 {
                    return Err(format!(
                        "seed event at index {index} has seq {} (expected {index}); seed must be contiguous from 0",
                        snapshot.seq
                    ));
                }
                state
                    .surface
                    .validate_next(&state.log, &snapshot)
                    .map_err(|error| format!("invalid seed event at index {index}: {error}"))?;
                state.log.push(snapshot);
            }
        }
        let first_live_seq = state.log.len();
        let header = match header {
            Some(header) => {
                let value = serde_json::to_value(header).map_err(|_| {
                    "session header is not losslessly JSON-serializable".to_string()
                })?;
                validate_session_header(&id, &value)?
            }
            None => snapshot_session_header(&id, None)?,
        };
        // Appended here so the marker is already in `events` when a backend
        // captures the creation seed; re-marking is skipped.
        if had_seed
            && state.log.last().map(|event| event.type_.as_str()) != Some("session/end-seed")
        {
            let event = SessionEvent {
                type_: "session/end-seed".to_string(),
                seq: state.log.len() as u64,
                time: now_ms(),
                data: end_seed_data(),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            };
            state
                .surface
                .validate_next(&state.log, &event)
                .expect("the end-seed marker carries no surface metadata");
            state.log.push(event);
        }
        Ok(Session {
            inner: Arc::new(SessionInner {
                id,
                header,
                first_live_seq,
                state: Mutex::new(state),
            }),
        })
    }

    /// The session identity, derived from its durable header's single copy.
    pub fn id(&self) -> &SessionId {
        &self.inner.id
    }

    /// Identity comparison over cloneable handles (TS `===` on the Session
    /// object): true only for the exact same live session.
    pub fn ptr_eq(&self, other: &Session) -> bool {
        std::sync::Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// A process-unique opaque identity for map keys (TS object identity).
    pub fn identity(&self) -> usize {
        std::sync::Arc::as_ptr(&self.inner) as *const () as usize
    }

    /// Detached, deep-frozen creation metadata.
    pub fn header(&self) -> &SessionHeader {
        &self.inner.header
    }

    /// The first seq appended IN THIS PROCESS (TS `firstLiveSeq`).
    pub fn first_live_seq(&self) -> usize {
        self.inner.first_live_seq
    }

    /// An immutable snapshot of the append-only event log. The snapshot is
    /// reused until the next append; a previously returned array does not
    /// grow later.
    pub fn events(&self) -> Arc<Vec<SessionEvent>> {
        Arc::new(self.inner.state.lock().log.clone())
    }

    /// Clone only the durable tail at or after `from_seq` without
    /// materializing the full immutable [`Self::events`] snapshot.
    pub fn events_from(&self, from_seq: u64) -> Vec<SessionEvent> {
        let Ok(start) = usize::try_from(from_seq) else {
            return Vec::new();
        };
        let state = self.inner.state.lock();
        state.log.get(start..).unwrap_or_default().to_vec()
    }

    /// Clone the prefix through the last event matching `predicate`, while
    /// holding the session lock only once and never materializing a full-log
    /// snapshot first.
    pub fn prefix_through_last(
        &self,
        predicate: impl Fn(&SessionEvent) -> bool,
    ) -> Vec<SessionEvent> {
        let state = self.inner.state.lock();
        let Some(last) = state.log.iter().rposition(predicate) else {
            return Vec::new();
        };
        state.log[..=last].to_vec()
    }

    /// Clone one event by durable sequence without materializing the full
    /// immutable [`Self::events`] snapshot.
    pub fn event_at(&self, seq: u64) -> Option<SessionEvent> {
        let index = usize::try_from(seq).ok()?;
        self.inner.state.lock().log.get(index).cloned()
    }

    /// The next event's sequence number — always the log length.
    pub fn seq(&self) -> usize {
        self.inner.state.lock().log.len()
    }

    /// The ordered surface over this session's event log (snapshot per
    /// call; TS returns the live manager view).
    pub fn surface(&self) -> Result<crate::surface::SessionSurface, String> {
        let state = &mut *self.inner.state.lock();
        let nodes = state.surface.nodes(&state.log)?;
        let replace_generation = state.surface.replace_generation(&state.log)?;
        Ok(crate::surface::SessionSurface {
            nodes,
            replace_generation,
        })
    }

    /// Append one typed event to the log and notify observers via the
    /// store-owned publication hooks (TS `Session.append`).
    pub fn append(
        &self,
        type_: &str,
        data: JsonValue,
        intent: Option<SurfaceIntent>,
    ) -> Result<SessionEvent, String> {
        self.append_if(type_, data, intent, |_| true)?
            .ok_or_else(|| "unconditional session append was rejected".to_string())
    }

    /// Append only when `condition` accepts the exact durable prefix while
    /// the session log is locked. A rejected condition writes and publishes
    /// nothing. The condition must not call back into this Session.
    pub fn append_if<F>(
        &self,
        type_: &str,
        data: JsonValue,
        intent: Option<SurfaceIntent>,
        condition: F,
    ) -> Result<Option<SessionEvent>, String>
    where
        F: FnOnce(&[SessionEvent]) -> bool,
    {
        let data_snapshot = snapshot_json_value(&data).ok_or_else(|| {
            format!("session event \"{type_}\" carries non-JSON-serializable data")
        })?;
        assert_supported_request_header(
            type_,
            &data_snapshot,
            &format!("session event \"{type_}\""),
        )?;
        let entry = attachment_of(self);
        if let Some(entry) = &entry
            && !entry.try_begin_append()
        {
            return Err(
                "session append cannot reenter while another append is being published".to_string(),
            );
        }
        let outcome =
            (|| -> Result<(Option<SessionEvent>, Vec<(Context, Arc<Listener>)>), String> {
                let state = &mut *self.inner.state.lock();
                if !condition(&state.log) {
                    return Ok((None, Vec::new()));
                }
                let event = SessionEvent {
                    type_: type_.to_string(),
                    seq: state.log.len() as u64,
                    time: now_ms(),
                    data: data_snapshot,
                    ignorable: None,
                    surface_op: intent.as_ref().map(|intent| intent.surface_op.clone()),
                    source_event_seqs: intent.and_then(|intent| intent.source_event_seqs),
                };
                state.surface.validate_next(&state.log, &event)?;
                // Resolve the listener snapshot BEFORE the log push (callbacks
                // run after it, exactly like the TS flow).
                let listeners: Vec<(Context, Arc<Listener>)> = match &entry {
                    Some(entry) => {
                        let dispatch_ctx = entry.emit_ctx.with_filter(entry.carrier.filter.clone());
                        // Public and internal observers receive the same compact
                        // [session, event] shape. Invariants keep incremental
                        // per-session folds instead of requiring an O(history)
                        // authority prefix on every append.
                        let args: Vec<ArcValue> = vec![arc(self.clone()), arc(event.clone())];
                        entry.emit_ctx.events.collect(
                            DispatchMode::Emit,
                            Some(&dispatch_ctx),
                            "session/event",
                            &args,
                        )
                    }
                    None => Vec::new(),
                };
                state.log.push(event.clone());
                Ok((Some(event), listeners))
            })();
        let (event, listeners) = match outcome {
            Ok(result) => result,
            Err(error) => {
                if let Some(entry) = &entry
                    && entry.finish_append()
                {
                    entry.detach_now();
                }
                return Err(error);
            }
        };
        let Some(event) = event else {
            if let Some(entry) = &entry
                && entry.finish_append()
            {
                entry.detach_now();
            }
            return Ok(None);
        };
        // TS runs observers INSIDE the guarded region while `appending` is
        // still true, so a reentrant append rejects instead of deadlocking.
        if let Some(entry) = &entry {
            let args: Vec<ArcValue> = vec![arc(self.clone()), arc(event.clone())];
            invoke_contained_session_observers(
                &entry.emit_ctx,
                "session/event",
                &entry.id,
                &args,
                &listeners,
            );
            if entry.finish_append() {
                entry.detach_now();
            }
        }
        Ok(Some(event))
    }

    /// The [`EpochHeader`] in force after the log's last header event.
    pub fn request_header(&self) -> Option<EpochHeader> {
        let state = &mut *self.inner.state.lock();
        if state.header_fold_seq < state.log.len() {
            let new_events = &state.log[state.header_fold_seq..];
            state.header_fold =
                crate::request_header::fold_request_header(new_events, state.header_fold.clone());
            state.header_fold_seq = state.log.len();
        }
        state.header_fold.clone()
    }

    /// The latest resolved route metadata, or `None` before the first
    /// `request/context` event.
    pub fn request_context(&self) -> Option<RequestContext> {
        let state = &mut *self.inner.state.lock();
        if state.context_fold_seq < state.log.len() {
            for event in &state.log[state.context_fold_seq..] {
                if event.type_ == "request/context" {
                    state.context_fold =
                        serde_json::from_value::<RequestContext>(event.data.clone()).ok();
                }
            }
            state.context_fold_seq = state.log.len();
        }
        state.context_fold.clone()
    }

    /// Derive the LLM message history by walking the ordered sequences of
    /// message-producing events maintained by `surfaceOp` markers.
    pub fn derive_messages(&self) -> Result<Arc<Vec<Message>>, String> {
        let state = &mut *self.inner.state.lock();
        let nodes = state.surface.nodes(&state.log)?;
        let generation = state.surface.replace_generation(&state.log)?;
        if generation != state.derived_generation {
            state.derived = Arc::new(Vec::new());
            state.derived_nodes = 0;
            state.derived_generation = generation;
        }
        if state.derived_nodes < nodes.len() {
            let start = state.derived_nodes;
            let additions = nodes[start..]
                .iter()
                .filter_map(|seq| state.log.get(*seq as usize))
                .filter_map(derive_event_message)
                .collect::<Vec<_>>();
            Arc::make_mut(&mut state.derived).extend(additions);
            state.derived_nodes = nodes.len();
        }
        Ok(Arc::clone(&state.derived))
    }

    /// Instance face of the pure per-node `deriveEventMessage` export.
    pub fn derive_event_message(&self, event: &SessionEvent) -> Option<Message> {
        derive_event_message(event)
    }
}

/// Look up the store attachment for a live session (TS `attachments`).
pub(crate) fn attachment_of(session: &Session) -> Option<Arc<SessionEntry>> {
    let ptr = Arc::as_ptr(&session.inner) as *const () as usize;
    ATTACHMENTS.lock().get(&ptr).and_then(|weak| weak.upgrade())
}

/// Render a caught panic payload for logging. Consumes the boxed payload:
/// `downcast` on `Box<dyn Any + Send>` is the reliable form (the
/// `&(dyn Any + Send)` `downcast_ref` form mismatches string payloads on
/// this toolchain).
fn render_panic(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<&str>() {
        Ok(message) => message.to_string(),
        Err(payload) => match payload.downcast::<String>() {
            Ok(message) => *message,
            Err(_) => "<non-string panic>".to_string(),
        },
    }
}

/// Invoke one resolved observe-only listener snapshot with per-listener
/// containment (TS `invokeContainedSessionObservers`).
///
/// Observers run INLINE, exactly like the TS synchronous callbacks: each
/// listener future is driven to completion with its panic contained and
/// logged. Listeners that need background I/O must spawn their own work and
/// return promptly (the TS contract: a synchronous, quick callback body).
fn invoke_contained_session_observers(
    ctx: &Context,
    name: &str,
    id: &SessionId,
    args: &[ArcValue],
    listeners: &[(Context, Arc<Listener>)],
) {
    let logger = ctx.named_logger(Some("sessions"));
    for (listener_ctx, callback) in listeners {
        let listener_ctx = listener_ctx.clone();
        let callback = callback.clone();
        let listener_args = args.to_vec();
        let prefix = format!("session \"{}\": {name} listener", id.as_str());
        let runtime = tokio::runtime::Handle::try_current().ok();
        let outcome = std::thread::spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let future = callback(&listener_ctx, listener_args);
                match runtime {
                    Some(runtime) => runtime.block_on(future),
                    None => futures::executor::block_on(future),
                }
            }))
        })
        .join();
        match outcome {
            Ok(Ok(_)) => {}
            Ok(Err(payload)) => {
                logger.warn(vec![arc(format!(
                    "{prefix} threw: {}",
                    render_panic(payload)
                ))]);
            }
            Err(payload) => {
                logger.warn(vec![arc(format!(
                    "{prefix} worker threw: {}",
                    render_panic(payload)
                ))]);
            }
        }
    }
}

// ---- SessionStore ----

/// One entered session and its publication state (TS `SessionEntry`).
pub(crate) struct SessionEntry {
    pub id: SessionId,
    pub session: Session,
    pub carrier: ScopeCarrier,
    pub emit_ctx: Context,
    flags: Mutex<EntryFlags>,
    append_released: parking_lot::Condvar,
    /// Store-owned detach transition (TS `entry.detach()` closure).
    detach: Arc<dyn Fn(&Arc<SessionEntry>) + Send + Sync>,
}

#[derive(Default)]
struct EntryFlags {
    announced: bool,
    announcing: bool,
    appending: bool,
    append_owner: Option<std::thread::ThreadId>,
    detach_requested: bool,
}

impl SessionEntry {
    fn begin_announce(&self) -> bool {
        let mut flags = self.flags.lock();
        if flags.announced || flags.announcing {
            return false;
        }
        flags.announced = true;
        flags.announcing = true;
        true
    }

    fn finish_announce(&self) -> bool {
        let mut flags = self.flags.lock();
        flags.announcing = false;
        flags.detach_requested && !flags.appending
    }

    fn request_detach_if_busy(&self) -> bool {
        let mut flags = self.flags.lock();
        if flags.announcing || flags.appending {
            flags.detach_requested = true;
            return true;
        }
        false
    }

    fn is_announced(&self) -> bool {
        self.flags.lock().announced
    }

    fn try_begin_append(&self) -> bool {
        let mut flags = self.flags.lock();
        let current = std::thread::current().id();
        while flags.appending {
            if flags.append_owner == Some(current) {
                return false;
            }
            self.append_released.wait(&mut flags);
        }
        flags.appending = true;
        flags.append_owner = Some(current);
        true
    }

    fn finish_append(&self) -> bool {
        let mut flags = self.flags.lock();
        flags.appending = false;
        flags.append_owner = None;
        let detach = flags.detach_requested && !flags.announcing;
        self.append_released.notify_all();
        detach
    }

    fn set_detach_requested(&self, value: bool) {
        self.flags.lock().detach_requested = value;
    }

    fn detach_now(self: &Arc<Self>) {
        (self.detach)(self);
    }
}

/// A fork source: either the live session object or its live store id.
#[derive(Debug, Clone)]
pub enum SessionForkSource {
    Session(Session),
    Id(SessionId),
}

/// Rejection codes for session forking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionForkErrorCode {
    SessionNotFound,
    SessionNotLive,
    SessionAlreadyExists,
    InvalidBoundary,
    OpenTurn,
}

impl SessionForkErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionForkErrorCode::SessionNotFound => "SESSION_NOT_FOUND",
            SessionForkErrorCode::SessionNotLive => "SESSION_NOT_LIVE",
            SessionForkErrorCode::SessionAlreadyExists => "SESSION_ALREADY_EXISTS",
            SessionForkErrorCode::InvalidBoundary => "INVALID_BOUNDARY",
            SessionForkErrorCode::OpenTurn => "OPEN_TURN",
        }
    }
}

/// Typed error for session fork rejections.
#[derive(Debug)]
pub struct SessionForkError {
    pub message: String,
    pub code: SessionForkErrorCode,
}

impl std::fmt::Display for SessionForkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessionForkError {}

/// A fork rejection: a typed [`SessionForkError`], or a plain store error
/// from the underlying `create` call (TS propagates both).
#[derive(Debug)]
pub enum ForkError {
    Fork(SessionForkError),
    Store(String),
}

impl std::fmt::Display for ForkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForkError::Fork(error) => write!(f, "{error}"),
            ForkError::Store(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ForkError {}

impl From<SessionForkError> for ForkError {
    fn from(error: SessionForkError) -> Self {
        ForkError::Fork(error)
    }
}

impl From<String> for ForkError {
    fn from(message: String) -> Self {
        ForkError::Store(message)
    }
}

/// In-memory session store (`ctx.sessions`). Persistence is intentionally
/// not implemented here — persistence plugins subscribe to `session/event`
/// and flush on `session/flush` / dispose.
pub struct SessionStore {
    pub ctx: Context,
    store: Arc<Mutex<HashMap<String, Arc<SessionEntry>>>>,
    counter: AtomicU64,
}

impl SessionStore {
    /// Create the store, register it as the `sessions` service, and wire
    /// the `typert` session lookup (TS `SessionStore` constructor).
    pub fn install(ctx: &Context) -> Arc<Self> {
        let store = Arc::new(Self {
            ctx: ctx.clone(),
            store: Arc::new(Mutex::new(HashMap::new())),
            counter: AtomicU64::new(0),
        });
        ctx.register_service(store.clone());

        let store_for_inject = Arc::clone(&store);
        ctx.inject(
            InjectSpec::new(["typert"]),
            Arc::new(move |type_ctx: &Context, _config: ArcValue| {
                let store = Arc::clone(&store_for_inject);
                let type_ctx = type_ctx.clone();
                Box::pin(async move {
                    if let Some(typert) = type_ctx.get_typed::<Arc<TypertService>>("typert", false)
                    {
                        let disposer = typert.lookups.register(
                            "session",
                            TypertLookup {
                                key: "session".to_string(),
                                parameter: "session".to_string(),
                                wire: "sessionId".to_string(),
                                host_type_symbol: "@deepseek-ai/dsh-session#Session".to_string(),
                                wire_type_symbol: "@deepseek-ai/dsh-session/types#SessionId"
                                    .to_string(),
                                resolve: Arc::new(move |id| store.get(&session_id(id)).map(arc)),
                            },
                        );
                        // Own the lookup for the inject fiber's lifetime.
                        let _ = type_ctx.effect(
                            "typert lookup session",
                            Box::pin(async move { Some(disposer) }),
                        );
                    }
                    Ok(())
                })
            }),
        );
        store
    }

    /// Create a session owned by the calling fiber: disposing that fiber
    /// stops event notification and removes the session from the store
    /// (TS `SessionStore.create`; the caller context is explicit here —
    /// the TS Proxy rebinds `this.ctx.effect` to the caller's fiber).
    /// `async` in Rust so a vetoing `session/created` listener rolls the
    /// attach back.
    pub async fn create(
        &self,
        caller: &Context,
        id: Option<SessionId>,
        options: Option<CreateSessionOptions>,
    ) -> Result<Session, String> {
        let session = self.prepare(id, options)?;
        let detach = self.enter(&session)?;
        if let Err(error) = self.announce(&session).await {
            detach().await;
            return Err(error);
        }
        // Single effect owned by the calling fiber: detach on unload.
        let _ = caller.effect("sessions.create()", Box::pin(async move { Some(detach) }));
        Ok(session)
    }

    /// Build a session WITHOUT entering it into the store (TS
    /// `SessionStore.prepare`).
    pub fn prepare(
        &self,
        id: Option<SessionId>,
        options: Option<CreateSessionOptions>,
    ) -> Result<Session, String> {
        let options = options.unwrap_or_default();
        let session_id = match id {
            Some(id) => id,
            None => loop {
                let counter = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
                let minted = session_id(format!("session-{counter}"));
                if !self.store.lock().contains_key(minted.as_str()) {
                    break minted;
                }
            },
        };
        if self.store.lock().contains_key(session_id.as_str()) {
            return Err(format!(
                "session \"{}\" already exists",
                session_id.as_str()
            ));
        }
        let meta = &options.meta;
        let header = SessionHeader {
            version: crate::SESSION_FORMAT_VERSION,
            id: session_id.clone(),
            created_at: meta
                .as_ref()
                .and_then(|meta| meta.created_at)
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() as u64),
            cwd: meta.as_ref().and_then(|meta| meta.cwd.clone()),
            parent_session: meta.as_ref().and_then(|meta| meta.parent_session.clone()),
            seed_length: meta.as_ref().and_then(|meta| meta.seed_length),
            origin: meta.as_ref().and_then(|meta| meta.origin.clone()),
            delegation_depth: meta.as_ref().and_then(|meta| meta.delegation_depth),
            agent_preset: meta.as_ref().and_then(|meta| meta.agent_preset.clone()),
        };
        Session::create(session_id, options.seed, Some(&header))
    }

    /// Enter a prepared session into the store: install the publication
    /// hooks and add it to the store. Returns the DETACH disposer; does NOT
    /// emit `session/created` (TS `SessionStore.enter`).
    pub fn enter(&self, session: &Session) -> Result<Disposer, String> {
        let id = session.id().clone();
        let carrier = scope_target(None, scope_of(&self.ctx));
        let attachment_key = Arc::as_ptr(&session.inner) as *const () as usize;

        let store_map = self.store.clone();
        let detach_fn: Arc<dyn Fn(&Arc<SessionEntry>) + Send + Sync> = Arc::new(move |entry| {
            entry.set_detach_requested(false);
            // A stale capability cannot remove observers or storage
            // belonging to a later same-id lifecycle.
            {
                let mut store = store_map.lock();
                let is_current = store
                    .get(entry.id.as_str())
                    .is_some_and(|live| Arc::ptr_eq(live, entry));
                if !is_current {
                    return;
                }
                store.remove(entry.id.as_str());
            }
            ATTACHMENTS
                .lock()
                .remove(&(Arc::as_ptr(&entry.session.inner) as *const () as usize));
            if entry.is_announced() {
                emit_disposed(entry);
            }
        });

        let entry = Arc::new(SessionEntry {
            id: id.clone(),
            session: session.clone(),
            carrier,
            emit_ctx: self.ctx.clone(),
            flags: Mutex::new(EntryFlags::default()),
            append_released: parking_lot::Condvar::new(),
            detach: detach_fn,
        });
        {
            let mut store = self.store.lock();
            let mut attachments = ATTACHMENTS.lock();
            if store.contains_key(id.as_str()) {
                return Err(format!("session \"{}\" already exists", id.as_str()));
            }
            if attachments.contains_key(&attachment_key) {
                return Err(format!(
                    "session \"{}\" is already attached to a store",
                    id.as_str()
                ));
            }
            store.insert(id.as_str().to_string(), Arc::clone(&entry));
            attachments.insert(attachment_key, Arc::downgrade(&entry));
        }

        let entered = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let detach: Disposer = make_disposer(move || {
            let entry = Arc::clone(&entry);
            let entered = Arc::clone(&entered);
            Box::pin(async move {
                if !entered.swap(false, Ordering::SeqCst) {
                    return;
                }
                // A lifecycle listener may own the advanced detach
                // capability: keep the entry live until publication unwinds.
                if entry.request_detach_if_busy() {
                    return;
                }
                entry.detach_now();
            })
        });
        Ok(detach)
    }

    /// Emit `session/created` exactly once for an entered session, with the
    /// carrier captured at enter (TS `SessionStore.announce`).
    pub async fn announce(&self, session: &Session) -> Result<(), String> {
        let entry = self.live_entry_for(session)?;
        if !entry.begin_announce() {
            return Err(format!(
                "session \"{}\" was already announced",
                entry.id.as_str()
            ));
        }
        // Mark before emit so rollback pairs the creation with disposal.

        let dispatch_ctx = entry.emit_ctx.with_filter(entry.carrier.filter.clone());
        let args: Vec<ArcValue> = vec![arc(session.clone())];
        let listeners = entry.emit_ctx.events.collect(
            DispatchMode::Emit,
            Some(&dispatch_ctx),
            "session/created",
            &args,
        );
        let mut veto: Option<String> = None;
        for (listener_ctx, callback) in &listeners {
            let future = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback(listener_ctx, args.clone())
            }));
            match future {
                Ok(future) => {
                    if let Err(payload) = std::panic::AssertUnwindSafe(future).catch_unwind().await
                    {
                        veto = Some(render_panic(payload));
                        break;
                    }
                }
                Err(payload) => {
                    veto = Some(render_panic(payload));
                    break;
                }
            }
        }
        if entry.finish_announce() {
            entry.detach_now();
        }
        match veto {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    /// Dispatch the awaited `session/flush` durability checkpoint for
    /// `session` (TS `SessionStore.flush`).
    pub async fn flush(&self, session: &Session) -> Result<bool, String> {
        let entry = self.live_entry_for(session)?;
        let dispatch_ctx = entry.emit_ctx.with_filter(entry.carrier.filter.clone());
        let args: Vec<ArcValue> = vec![arc(session.clone())];
        let listeners = entry.emit_ctx.events.collect(
            DispatchMode::Parallel,
            Some(&dispatch_ctx),
            "session/flush",
            &args,
        );
        let mut futures: Vec<cordis::BoxFuture<'static, Result<(), String>>> = Vec::new();
        for (listener_ctx, callback) in &listeners {
            let future = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback(listener_ctx, args.clone())
            }));
            futures.push(Box::pin(async move {
                let future = future.map_err(render_panic)?;
                std::panic::AssertUnwindSafe(future)
                    .catch_unwind()
                    .await
                    .map_err(render_panic)
                    .map(|_| ())
            }));
        }
        let results = futures::future::join_all(futures).await;
        let failure = results.into_iter().find_map(Result::err);
        if let Some(failure) = failure {
            return Err(failure);
        }
        Ok(!listeners.is_empty())
    }

    /// Return the exact live entry; detached/prepared objects reject.
    fn live_entry_for(&self, session: &Session) -> Result<Arc<SessionEntry>, String> {
        let entry = attachment_of(session);
        let is_live = entry.as_ref().is_some_and(|entry| {
            self.store
                .lock()
                .get(entry.id.as_str())
                .is_some_and(|live| Arc::ptr_eq(live, entry))
        });
        match (entry, is_live) {
            (Some(entry), true) => Ok(entry),
            _ => Err(format!(
                "session \"{}\" is not live in this store",
                session.id().as_str()
            )),
        }
    }

    /// Look up a live session by id.
    pub fn get(&self, id: &SessionId) -> Option<Session> {
        self.store
            .lock()
            .get(id.as_str())
            .map(|entry| entry.session.clone())
    }

    /// All live sessions, in creation order.
    pub fn list(&self) -> Vec<Session> {
        self.store
            .lock()
            .values()
            .map(|entry| entry.session.clone())
            .collect()
    }

    /// Create a live child session from a stable prefix of a live source
    /// (TS `SessionStore.fork`; the caller context is explicit — the TS
    /// Proxy rebinds the inner `create` effect to the caller's fiber).
    pub async fn fork(
        &self,
        caller: &Context,
        source: SessionForkSource,
        boundary: Option<u64>,
        child_session_id: Option<SessionId>,
    ) -> Result<Session, ForkError> {
        if let Some(child_id) = &child_session_id
            && self.get(child_id).is_some()
        {
            return Err(SessionForkError {
                message: format!("session \"{}\" already exists", child_id.as_str()),
                code: SessionForkErrorCode::SessionAlreadyExists,
            }
            .into());
        }
        let live_source = self.resolve_fork_source(&source)?;
        let seed = self.fork_seed(&live_source, boundary)?;
        let meta = crate::types::CreateSessionMeta {
            cwd: live_source.header().cwd.clone(),
            parent_session: Some(live_source.id().clone()),
            seed_length: Some(seed.len() as u64),
            ..Default::default()
        };
        self.create(
            caller,
            child_session_id,
            Some(CreateSessionOptions {
                seed: Some(seed),
                meta: Some(meta),
            }),
        )
        .await
        .map_err(ForkError::Store)
    }

    fn resolve_fork_source(&self, source: &SessionForkSource) -> Result<Session, SessionForkError> {
        match source {
            SessionForkSource::Id(id) => self.get(id).ok_or_else(|| SessionForkError {
                message: format!("session \"{}\" not found", id.as_str()),
                code: SessionForkErrorCode::SessionNotFound,
            }),
            SessionForkSource::Session(session) => {
                let live = self.get(session.id()).ok_or_else(|| SessionForkError {
                    message: format!("session \"{}\" not found", session.id().as_str()),
                    code: SessionForkErrorCode::SessionNotFound,
                })?;
                if !Arc::ptr_eq(&live.inner, &session.inner) {
                    return Err(SessionForkError {
                        message: format!(
                            "session \"{}\" is not the live store instance",
                            session.id().as_str()
                        ),
                        code: SessionForkErrorCode::SessionNotLive,
                    });
                }
                Ok(live)
            }
        }
    }

    fn fork_seed(
        &self,
        session: &Session,
        requested_boundary: Option<u64>,
    ) -> Result<Vec<SessionEvent>, SessionForkError> {
        let events = session.events();
        let boundary = match requested_boundary {
            Some(boundary) => boundary,
            None => match events.last() {
                None => return Ok(Vec::new()),
                Some(last) => last.seq,
            },
        };
        if boundary >= events.len() as u64 {
            return Err(SessionForkError {
                message: format!(
                    "fork boundary {boundary} does not exist in session \"{}\" (last seq: {})",
                    session.id().as_str(),
                    events
                        .last()
                        .map(|event| event.seq.to_string())
                        .unwrap_or_else(|| "none".to_string())
                ),
                code: SessionForkErrorCode::InvalidBoundary,
            });
        }
        let boundary_event = &events[boundary as usize];
        if boundary_event.seq != boundary {
            return Err(SessionForkError {
                message: format!(
                    "fork boundary {boundary} does not match a contiguous event seq in session \"{}\"",
                    session.id().as_str()
                ),
                code: SessionForkErrorCode::InvalidBoundary,
            });
        }
        let last_turn_boundary = events[..=boundary as usize]
            .iter()
            .rev()
            .find(|event| event.type_ == "turn/start" || event.type_ == "turn/end");
        if last_turn_boundary.is_some_and(|event| event.type_ == "turn/start") {
            let turn = last_turn_boundary
                .and_then(|event| event.data.get("turn"))
                .and_then(|turn| turn.as_u64())
                .map(|turn| turn.to_string())
                .unwrap_or_default();
            return Err(SessionForkError {
                message: format!(
                    "fork boundary {boundary} in session \"{}\" ends inside open turn {turn}",
                    session.id().as_str()
                ),
                code: SessionForkErrorCode::OpenTurn,
            });
        }
        Ok(events[..=boundary as usize].to_vec())
    }
}

/// Emit the paired teardown notification with per-listener containment
/// (TS `SessionStore.emitDisposed`).
fn emit_disposed(entry: &Arc<SessionEntry>) {
    let dispatch_ctx = entry.emit_ctx.with_filter(entry.carrier.filter.clone());
    let args: Vec<ArcValue> = vec![arc(entry.session.clone())];
    let listeners = entry.emit_ctx.events.collect(
        DispatchMode::Emit,
        Some(&dispatch_ctx),
        "session/disposed",
        &args,
    );
    invoke_contained_session_observers(
        &entry.emit_ctx,
        "session/disposed",
        &entry.id,
        &args,
        &listeners,
    );
}

impl Service for SessionStore {
    fn service_name(&self) -> &'static str {
        "sessions"
    }
}
