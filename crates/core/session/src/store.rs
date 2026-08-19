//! Event-sourced session service: append-only session log, in-memory store,
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
    match record.get("type").and_then(|value| value.as_str()) {
        Some("request/header" | "user/message" | "assistant/message" | "tool/result") => {
            assert_current_llm_shape(record, index)?;
        }
        _ => {}
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
        if let Some(effort) = config_record.get("reasoningEffort") {
            if effort.as_str().is_none_or(|effort| effort.is_empty()) {
                return Err(format!(
                    "seed request/header at index {index} has an invalid reasoningEffort"
                ));
            }
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
    events_snapshot: Option<Arc<Vec<SessionEvent>>>,
    header_fold: Option<EpochHeader>,
    header_fold_seq: usize,
    context_fold: Option<RequestContext>,
    context_fold_seq: usize,
    derived: Vec<Message>,
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
        let state = &mut *self.inner.state.lock();
        if state.events_snapshot.is_none() {
            state.events_snapshot = Some(Arc::new(state.log.clone()));
        }
        state.events_snapshot.as_ref().unwrap().clone()
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
        let outcome = (|| -> Result<(SessionEvent, Vec<(Context, Arc<Listener>)>), String> {
            let state = &mut *self.inner.state.lock();
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
                    let prefix = match &state.events_snapshot {
                        Some(prefix) => prefix.clone(),
                        None => {
                            let prefix = Arc::new(state.log.clone());
                            state.events_snapshot = Some(prefix.clone());
                            prefix
                        }
                    };
                    // The third argument is internal-only authority context:
                    // the immutable durable prefix before `event`. Public
                    // session/event observers still receive two arguments.
                    let args: Vec<ArcValue> =
                        vec![arc(self.clone()), arc(event.clone()), arc(prefix)];
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
            state.events_snapshot = None;
            Ok((event, listeners))
        })();
        let (event, listeners) = match outcome {
            Ok(result) => result,
            Err(error) => {
                if let Some(entry) = &entry {
                    if entry.finish_append() {
                        entry.detach_now();
                    }
                }
                return Err(error);
            }
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
        Ok(event)
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
    pub fn derive_messages(&self) -> Result<Vec<Message>, String> {
        let state = &mut *self.inner.state.lock();
        let nodes = state.surface.nodes(&state.log)?;
        let generation = state.surface.replace_generation(&state.log)?;
        if generation != state.derived_generation {
            state.derived = Vec::new();
            state.derived_nodes = 0;
            state.derived_generation = generation;
        }
        for seq in &nodes[state.derived_nodes..] {
            if let Some(event) = state.log.get(*seq as usize) {
                if let Some(message) = derive_event_message(event) {
                    state.derived.push(message);
                }
            }
        }
        state.derived_nodes = nodes.len();
        Ok(state.derived.clone())
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
        let future = callback(listener_ctx, args.to_vec());
        let prefix = format!("session \"{}\": {name} listener", id.as_str());
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures::executor::block_on(future)
        })) {
            Ok(_) => {}
            Err(payload) => {
                logger.warn(vec![arc(format!(
                    "{prefix} threw: {}",
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
    /// Store-owned detach transition (TS `entry.detach()` closure).
    detach: Arc<dyn Fn(&Arc<SessionEntry>) + Send + Sync>,
}

#[derive(Default)]
struct EntryFlags {
    announced: bool,
    announcing: bool,
    appending: bool,
    detach_requested: bool,
}

impl SessionEntry {
    fn is_announced(&self) -> bool {
        self.flags.lock().announced
    }

    fn is_announcing(&self) -> bool {
        self.flags.lock().announcing
    }

    fn is_appending(&self) -> bool {
        self.flags.lock().appending
    }

    fn try_begin_append(&self) -> bool {
        let mut flags = self.flags.lock();
        if flags.appending {
            return false;
        }
        flags.appending = true;
        true
    }

    fn finish_append(&self) -> bool {
        let mut flags = self.flags.lock();
        flags.appending = false;
        flags.detach_requested && !flags.announcing
    }

    fn is_detach_requested(&self) -> bool {
        self.flags.lock().detach_requested
    }

    fn set_announced(&self, value: bool) {
        self.flags.lock().announced = value;
    }

    fn set_announcing(&self, value: bool) {
        self.flags.lock().announcing = value;
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
                                resolve: Arc::new(move |id| {
                                    store.get(&session_id(id)).map(|session| arc(session))
                                }),
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
        {
            let store = self.store.lock();
            if store.contains_key(id.as_str()) {
                return Err(format!("session \"{}\" already exists", id.as_str()));
            }
        }
        {
            let attachments = ATTACHMENTS.lock();
            if attachments.contains_key(&(Arc::as_ptr(&session.inner) as *const () as usize)) {
                return Err(format!(
                    "session \"{}\" is already attached to a store",
                    id.as_str()
                ));
            }
        }

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
            detach: detach_fn,
        });
        self.store
            .lock()
            .insert(id.as_str().to_string(), Arc::clone(&entry));
        ATTACHMENTS.lock().insert(
            Arc::as_ptr(&session.inner) as *const () as usize,
            Arc::downgrade(&entry),
        );

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
                if entry.is_announcing() || entry.is_appending() {
                    entry.set_detach_requested(true);
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
        if entry.is_announced() || entry.is_announcing() {
            return Err(format!(
                "session \"{}\" was already announced",
                entry.id.as_str()
            ));
        }
        // Mark before emit so rollback pairs the creation with disposal.
        entry.set_announced(true);
        entry.set_announcing(true);
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
            let future = callback(listener_ctx, args.clone());
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                futures::executor::block_on(future)
            })) {
                Ok(_) => {}
                Err(payload) => {
                    veto = Some(render_panic(payload));
                    break;
                }
            }
        }
        entry.set_announcing(false);
        if entry.is_detach_requested() && !entry.is_appending() {
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
        if let Some(child_id) = &child_session_id {
            if self.get(child_id).is_some() {
                return Err(SessionForkError {
                    message: format!("session \"{}\" already exists", child_id.as_str()),
                    code: SessionForkErrorCode::SessionAlreadyExists,
                }
                .into());
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SurfaceOp, TurnEndReason};
    use cordis::downcast;
    use dsh_llm::{ContentBlock, Role};
    use std::sync::atomic::{AtomicU32, Ordering as MemOrder};

    fn user_data(id: &str, text: &str) -> JsonValue {
        serde_json::json!({
            "id": id,
            "role": "user",
            "content": [{"type": "text", "text": text}],
            "source": {"kind": "user"},
        })
    }

    fn append_user(session: &Session, id: &str, text: &str) -> SessionEvent {
        session
            .append(
                "user/message",
                user_data(id, text),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap()
    }

    fn append_turn_boundary(session: &Session, type_: &str, turn: u64, reason: bool) {
        let data = if reason {
            crate::types::turn_end_data(turn, &TurnEndReason::Completed)
        } else {
            crate::types::turn_start_data(turn)
        };
        session.append(type_, data, None).unwrap();
    }

    #[test]
    fn detached_session_creation_and_seed() {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        assert_eq!(session.id().as_str(), "s1");
        assert_eq!(session.seq(), 0);
        assert_eq!(session.events().len(), 0);
        assert_eq!(session.first_live_seq(), 0);
        assert_eq!(session.header().version, crate::SESSION_FORMAT_VERSION);
        assert!(session.header().created_at > 0);
        assert!(session.request_header().is_none());
        assert!(session.request_context().is_none());
        assert_eq!(session.derive_messages().unwrap().len(), 0);
        assert_eq!(session.surface().unwrap().nodes, Vec::<u64>::new());

        // seeded creation appends the end-seed marker (constructor-owned)
        let seed = vec![SessionEvent {
            type_: "user/message".to_string(),
            seq: 0,
            time: 10,
            data: user_data("m1", "seed"),
            ignorable: None,
            surface_op: Some(SurfaceOp::Append),
            source_event_seqs: None,
        }];
        let seeded = Session::create(session_id("s2"), Some(seed), None).unwrap();
        assert_eq!(seeded.events().len(), 2);
        assert_eq!(seeded.events()[1].type_, "session/end-seed");
        assert_eq!(seeded.events()[1].seq, 1);
        assert_eq!(seeded.first_live_seq(), 1);
        assert_eq!(seeded.derive_messages().unwrap().len(), 1);

        // a seed already ending in end-seed is not re-marked
        let with_marker = vec![
            SessionEvent {
                type_: "user/message".to_string(),
                seq: 0,
                time: 10,
                data: user_data("m1", "seed"),
                ignorable: None,
                surface_op: Some(SurfaceOp::Append),
                source_event_seqs: None,
            },
            SessionEvent {
                type_: "session/end-seed".to_string(),
                seq: 1,
                time: 11,
                data: serde_json::json!({}),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
        ];
        let re_seeded = Session::create(session_id("s3"), Some(with_marker), None).unwrap();
        assert_eq!(re_seeded.events().len(), 2, "no duplicate marker");
    }

    #[test]
    fn seed_validation_rejects_bad_envelopes() {
        let bad_json = SessionEvent {
            type_: "user/message".to_string(),
            seq: 0,
            time: 0,
            data: serde_json::json!({
                "id": "",
                "role": "user",
                "content": [{"type": "text", "text": "x"}],
                "source": {"kind": "user"},
            }),
            ignorable: None,
            surface_op: Some(SurfaceOp::Append),
            source_event_seqs: None,
        };
        let error = Session::create(session_id("s1"), Some(vec![bad_json]), None).unwrap_err();
        assert!(error.contains("lacks an identified message"), "{error}");

        // wrong role
        let wrong_role = SessionEvent {
            type_: "user/message".to_string(),
            seq: 0,
            time: 0,
            data: serde_json::json!({
                "id": "m1",
                "role": "assistant",
                "content": [{"type": "text", "text": "x"}],
                "source": {"kind": "model", "provider": "p", "model": "m"},
            }),
            ignorable: None,
            surface_op: Some(SurfaceOp::Append),
            source_event_seqs: None,
        };
        let error = Session::create(session_id("s1"), Some(vec![wrong_role]), None).unwrap_err();
        assert!(error.contains("must have role \"user\""), "{error}");

        // non-contiguous seq
        let gap = SessionEvent {
            type_: "turn/start".to_string(),
            seq: 3,
            time: 0,
            data: serde_json::json!({"turn": 1}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        };
        let error = Session::create(session_id("s1"), Some(vec![gap]), None).unwrap_err();
        assert!(error.contains("must be contiguous from 0"), "{error}");

        // ignorable must be true or absent
        let false_ignorable = SessionEvent {
            type_: "turn/start".to_string(),
            seq: 0,
            time: 0,
            data: serde_json::json!({"turn": 1}),
            ignorable: Some(false),
            surface_op: None,
            source_event_seqs: None,
        };
        assert!(Session::create(session_id("s1"), Some(vec![false_ignorable]), None).is_err());

        // legacy header delta vocabulary is rejected
        let legacy = SessionEvent {
            type_: "request/header-delta".to_string(),
            seq: 0,
            time: 0,
            data: serde_json::json!({}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        };
        let error = Session::create(session_id("s1"), Some(vec![legacy]), None).unwrap_err();
        assert!(
            error.contains("unsupported legacy request/header-delta format"),
            "{error}"
        );
    }

    #[test]
    fn append_validates_data_and_surface() {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        // non-JSON data (negative zero) rejected at the source
        let negative_zero = serde_json::Number::from_f64(-0.0)
            .map(JsonValue::Number)
            .unwrap();
        let error = session
            .append(
                "turn/start",
                serde_json::json!({"turn": 1, "bad": negative_zero}),
                None,
            )
            .unwrap_err();
        assert!(error.contains("non-JSON-serializable"), "{error}");

        // surface-eligible event without a marker rejected
        let error = session
            .append("user/message", user_data("m1", "x"), None)
            .unwrap_err();
        assert!(error.contains("requires a surfaceOp marker"), "{error}");

        // marker on a log-only event rejected
        let error = session
            .append(
                "turn/start",
                serde_json::json!({"turn": 1}),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap_err();
        assert!(error.contains("not surface-eligible"), "{error}");

        assert_eq!(session.seq(), 0, "rejected appends never enter the log");
    }

    #[test]
    fn append_assigns_seq_and_time_and_snapshots() {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        let first = append_user(&session, "m1", "a");
        assert_eq!(first.seq, 0);
        assert!(first.time > 0);
        let second = append_user(&session, "m2", "b");
        assert_eq!(second.seq, 1);
        assert!(second.time >= first.time);
        assert_eq!(session.seq(), 2);

        // the events snapshot is reused until the next append
        let snapshot_a = session.events();
        let snapshot_b = session.events();
        assert!(
            Arc::ptr_eq(&snapshot_a, &snapshot_b),
            "no append → same snapshot"
        );
        append_user(&session, "m3", "c");
        let snapshot_c = session.events();
        assert!(
            !Arc::ptr_eq(&snapshot_a, &snapshot_c),
            "append → fresh snapshot"
        );
        assert_eq!(
            snapshot_a.len(),
            2,
            "previously returned snapshots never grow"
        );
        assert_eq!(snapshot_c.len(), 3);

        // returned event.data is the logged snapshot, not the caller's input
        let mut input = user_data("m4", "d");
        input["content"][0]["text"] = serde_json::json!("mutated later");
        let logged = session
            .append(
                "user/message",
                input.clone(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        input["content"][0]["text"] = serde_json::json!("mutated");
        assert_eq!(logged.data["content"][0]["text"], "mutated later");
    }

    #[test]
    fn incremental_event_reads_do_not_materialize_full_snapshot() {
        let session = Session::create(session_id("incremental-events"), None, None).unwrap();
        append_user(&session, "m1", "a");
        append_user(&session, "m2", "b");

        assert!(session.inner.state.lock().events_snapshot.is_none());

        let tail = session.events_from(1);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].seq, 1);
        assert!(session.inner.state.lock().events_snapshot.is_none());

        let first = session.event_at(0).expect("event at seq 0");
        assert_eq!(first.seq, 0);
        assert!(session.event_at(2).is_none());
        assert!(session.inner.state.lock().events_snapshot.is_none());
    }

    #[test]
    fn derive_messages_and_folds() {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        append_turn_boundary(&session, "turn/start", 1, false);
        append_user(&session, "m1", "hello");

        // assistant message with usage
        session
            .append(
                "assistant/message",
                crate::types::assistant_message_data(
                    1,
                    1,
                    &dsh_llm::create_assistant_message(
                        vec![ContentBlock::Text {
                            text: "hi".to_string(),
                        }],
                        dsh_llm::ModelMessageSource {
                            provider: "p".to_string(),
                            model: "m".to_string(),
                            replay_state: None,
                        },
                    ),
                    Some(&dsh_llm::TokenUsage {
                        input_tokens: 3,
                        output_tokens: 1,
                        ..Default::default()
                    }),
                ),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();

        let messages = session.derive_messages().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);

        // an empty-content assistant/message derives to nothing
        session
            .append(
                "assistant/message",
                serde_json::json!({
                    "turn": 1, "step": 2,
                    "message": {
                        "id": "empty", "role": "assistant", "content": [],
                        "source": {"kind": "model", "provider": "p", "model": "m"},
                    },
                }),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        assert_eq!(session.derive_messages().unwrap().len(), 2);

        // surface replace: derived history follows the fold
        let mut history = session.events().as_ref().clone();
        let _ = &mut history;
        let replacement = session
            .append(
                "user/message",
                user_data("summary", "condensed"),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Replace { start: 1, end: 1 },
                    source_event_seqs: Some(vec![1]),
                }),
            )
            .unwrap();
        assert_eq!(replacement.seq, 4);
        let surface = session.surface().unwrap();
        assert_eq!(surface.replace_generation, 1);
        let messages = session.derive_messages().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id.as_str(), "summary");
        assert_eq!(messages[1].role, Role::Assistant);
    }

    #[test]
    fn request_header_and_context_folds() {
        let session = Session::create(session_id("s1"), None, None).unwrap();
        assert!(session.request_header().is_none());
        assert!(session.request_context().is_none());

        session
            .append(
                "request/header",
                crate::types::request_header_data(
                    &EpochHeader {
                        config: dsh_llm::LlmCallConfig {
                            provider: "deepseek".to_string(),
                            model: "deepseek-chat".to_string(),
                            ..Default::default()
                        },
                        adapter_defaults: None,
                        system: Some("be helpful".to_string()),
                        tools: None,
                    },
                    crate::types::RequestHeaderReason::Initial,
                ),
                None,
            )
            .unwrap();
        let header = session.request_header().unwrap();
        assert_eq!(header.config.model, "deepseek-chat");
        assert_eq!(header.system.as_deref(), Some("be helpful"));

        session
            .append(
                "request/context",
                serde_json::json!({"provider": "deepseek", "model": "deepseek-chat", "contextWindow": 128000}),
                None,
            )
            .unwrap();
        let context = session.request_context().unwrap();
        assert_eq!(context.context_window, Some(128000));

        // the folds are incremental and stable
        let again = session.request_header().unwrap();
        assert_eq!(again, header);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_create_announces_and_observes_events() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);

        let created = Arc::new(AtomicU32::new(0));
        let c = created.clone();
        ctx.on(
            "session/created",
            Arc::new(move |_ctx, args| {
                let c = c.clone();
                Box::pin(async move {
                    let session = downcast::<Session>(&args[0]).expect("session arg");
                    assert_eq!(session.id().as_str().starts_with("session-"), true);
                    c.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;

        let observed = Arc::new(AtomicU32::new(0));
        let o = observed.clone();
        ctx.on(
            "session/event",
            Arc::new(move |_ctx, args| {
                let o = o.clone();
                Box::pin(async move {
                    let event = downcast::<SessionEvent>(&args[1]).expect("event arg");
                    assert_eq!(event.type_, "user/message");
                    o.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;

        let session = store.create(&ctx, None, None).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(created.load(MemOrder::SeqCst), 1);
        assert_eq!(
            store.get(session.id()).map(|s| s.id().clone()),
            Some(session.id().clone())
        );
        assert_eq!(store.list().len(), 1);

        append_user(&session, "m1", "hello");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(observed.load(MemOrder::SeqCst), 1);

        // disposed pair on fiber teardown: dispose the root-owned effect via
        // a fresh fiber is not available here, so detach through the store's
        // effect — verify the disposed notification on ctx dispose by
        // re-entering through enter/announce instead.
        let detached = store.enter(&session);
        assert!(detached.is_err(), "a live session cannot enter twice");
    }

    #[test]
    fn probe_panic_payload_through_block_on() {
        let future: cordis::BoxFuture<'static, Option<ArcValue>> =
            Box::pin(async { panic!("veto") });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures::executor::block_on(future)
        }));
        let payload = result.unwrap_err();
        assert_eq!(render_panic(payload), "veto");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_veto_rolls_back_and_pairs_disposal() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);

        let disposed = Arc::new(AtomicU32::new(0));
        let d = disposed.clone();
        ctx.on(
            "session/disposed",
            Arc::new(move |_ctx, _args| {
                let d = d.clone();
                Box::pin(async move {
                    d.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;

        // a panicking creation listener vetoes publication
        ctx.on(
            "session/created",
            Arc::new(|_ctx, _args| {
                Box::pin(async move {
                    panic!("veto");
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;

        let error = store
            .create(&ctx, Some(session_id("vetoed")), None)
            .await
            .unwrap_err();
        assert_eq!(error, "veto");
        assert!(
            store.get(&session_id("vetoed")).is_none(),
            "attach rolled back"
        );
        assert_eq!(store.list().len(), 0);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            disposed.load(MemOrder::SeqCst),
            1,
            "paired disposal edge emitted"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_ids_reject_at_prepare_and_enter() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let session = store
            .create(&ctx, Some(session_id("dup")), None)
            .await
            .unwrap();

        let error = store
            .create(&ctx, Some(session_id("dup")), None)
            .await
            .unwrap_err();
        assert!(error.contains("already exists"), "{error}");

        // prepare a distinct session, then enter the SAME prepared session twice
        let prepared = store.prepare(Some(session_id("fresh")), None).unwrap();
        let detach = store.enter(&prepared).unwrap();
        assert!(store.enter(&prepared).is_err());
        detach().await;
        assert!(store.get(&session_id("fresh")).is_none());
        assert_eq!(store.get(session.id()).is_some(), true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn flush_runs_listeners_and_reports_failure() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let session = store.create(&ctx, None, None).await.unwrap();

        let flushed = Arc::new(AtomicU32::new(0));
        let f = flushed.clone();
        ctx.on(
            "session/flush",
            Arc::new(move |_ctx, _args| {
                let f = f.clone();
                Box::pin(async move {
                    f.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;
        assert!(
            store.flush(&session).await.unwrap(),
            "listener participated"
        );
        assert_eq!(flushed.load(MemOrder::SeqCst), 1);

        // a failing listener rejects the flush after every listener settles
        ctx.on(
            "session/flush",
            Arc::new(|_ctx, _args| {
                Box::pin(async move {
                    panic!("disk full");
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;
        let error = store.flush(&session).await.unwrap_err();
        assert_eq!(error, "disk full");
        assert_eq!(
            flushed.load(MemOrder::SeqCst),
            2,
            "every listener still ran"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn flush_contains_synchronous_callback_panics_and_runs_other_listeners() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let session = store.create(&ctx, None, None).await.unwrap();
        let completed = Arc::new(AtomicU32::new(0));

        for _ in 0..2 {
            let completed = completed.clone();
            ctx.on(
                "session/flush",
                Arc::new(move |_ctx, _args| {
                    let completed = completed.clone();
                    Box::pin(async move {
                        completed.fetch_add(1, MemOrder::SeqCst);
                        None
                    })
                }),
                cordis::EventOptions::default(),
            )
            .await;
        }
        ctx.on(
            "session/flush",
            Arc::new(
                |_ctx, _args| -> cordis::BoxFuture<'static, Option<ArcValue>> {
                    panic!("sync callback panic")
                },
            ),
            cordis::EventOptions::default().prepend(true),
        )
        .await;

        let error = store
            .flush(&session)
            .await
            .expect_err("sync panic must be contained");
        assert_eq!(error, "sync callback panic");
        assert_eq!(
            completed.load(MemOrder::SeqCst),
            2,
            "every non-panicking listener must still run"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn flush_does_not_block_tokio_tasks_needed_by_async_listeners() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let session = store.create(&ctx, None, None).await.unwrap();
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fallback_used = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let notify = Arc::new(tokio::sync::Notify::new());

        let released_for_listener = released.clone();
        let notify_for_listener = notify.clone();
        ctx.on(
            "session/flush",
            Arc::new(move |_ctx, _args| {
                let released = released_for_listener.clone();
                let notify = notify_for_listener.clone();
                Box::pin(async move {
                    let released_for_task = released.clone();
                    let notify_for_task = notify.clone();
                    tokio::spawn(async move {
                        released_for_task.store(true, MemOrder::SeqCst);
                        notify_for_task.notify_waiters();
                    });
                    while !released.load(MemOrder::SeqCst) {
                        notify.notified().await;
                    }
                    None
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;

        let released_for_fallback = released.clone();
        let fallback_for_thread = fallback_used.clone();
        let notify_for_fallback = notify.clone();
        let fallback = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !released_for_fallback.swap(true, MemOrder::SeqCst) {
                fallback_for_thread.store(true, MemOrder::SeqCst);
                notify_for_fallback.notify_waiters();
            }
        });

        assert!(store.flush(&session).await.unwrap());
        fallback.join().expect("fallback thread");
        assert!(
            !fallback_used.load(MemOrder::SeqCst),
            "flush synchronously blocked the Tokio task required by its listener"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn flush_rejects_unlive_sessions() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let detached = Session::create(session_id("detached"), None, None).unwrap();
        let error = store.flush(&detached).await.unwrap_err();
        assert!(error.contains("not live in this store"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn append_publication_claim_is_atomic_across_threads() {
        let session =
            Session::create(session_id("append-claim-race"), None, None).expect("session");
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let _detach = store.enter(&session).expect("enter");
        let entry = attachment_of(&session).expect("attachment");
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let results = Arc::new(Mutex::new(Vec::new()));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let entry = entry.clone();
            let barrier = barrier.clone();
            let results = results.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                let claimed = entry.try_begin_append();
                results.lock().push(claimed);
                barrier.wait();
                if claimed {
                    entry.finish_append();
                }
            }));
        }
        barrier.wait();
        barrier.wait();
        for thread in threads {
            thread.join().expect("claim thread");
        }
        let claims = results.lock().clone();
        assert_eq!(claims.iter().filter(|claimed| **claimed).count(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn append_reentrancy_is_rejected() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let session = store.create(&ctx, None, None).await.unwrap();
        let id = session.id().clone();

        let reentered = Arc::new(AtomicU32::new(0));
        let r = reentered.clone();
        let session_for_listener = session.clone();
        ctx.on(
            "session/event",
            Arc::new(move |_ctx, _args| {
                let r = r.clone();
                let session = session_for_listener.clone();
                Box::pin(async move {
                    let result = session.append("turn/start", serde_json::json!({"turn": 9}), None);
                    assert!(result.unwrap_err().contains("cannot reenter"));
                    r.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;

        append_user(&session, "m1", "outer");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(reentered.load(MemOrder::SeqCst), 1);
        assert_eq!(session.events().len(), 1, "only the outer append committed");
        assert_eq!(store.get(&id).map(|s| s.seq()), Some(1));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fork_creates_seeded_child() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let parent = store
            .create(&ctx, Some(session_id("parent")), None)
            .await
            .unwrap();
        append_turn_boundary(&parent, "turn/start", 1, false);
        append_user(&parent, "m1", "a");
        append_user(&parent, "m2", "b");
        append_turn_boundary(&parent, "turn/end", 1, true);

        let child = store
            .fork(
                &ctx,
                SessionForkSource::Session(parent.clone()),
                Some(3),
                Some(session_id("child")),
            )
            .await
            .unwrap();
        assert_eq!(child.id().as_str(), "child");
        assert_eq!(
            child.header().parent_session.as_ref().map(|s| s.as_str()),
            Some("parent")
        );
        assert_eq!(child.header().seed_length, Some(4));
        assert_eq!(child.events().len(), 5, "4 seed events + end-seed marker");
        assert_eq!(child.first_live_seq(), 4);
        assert_eq!(child.derive_messages().unwrap().len(), 2);

        // fork from a detached lookalike rejects with SESSION_NOT_LIVE
        let lookalike = Session::create(session_id("parent"), None, None).unwrap();
        let error = store
            .fork(&ctx, SessionForkSource::Session(lookalike), None, None)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ForkError::Fork(SessionForkError {
                code: SessionForkErrorCode::SessionNotLive,
                ..
            })
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fork_boundary_errors() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let parent = store
            .create(&ctx, Some(session_id("parent")), None)
            .await
            .unwrap();
        append_turn_boundary(&parent, "turn/start", 1, false);
        append_user(&parent, "m1", "a");

        // boundary beyond the log
        let error = store
            .fork(
                &ctx,
                SessionForkSource::Id(session_id("parent")),
                Some(5),
                None,
            )
            .await
            .unwrap_err();
        match error {
            ForkError::Fork(SessionForkError { code, .. }) => {
                assert_eq!(code, SessionForkErrorCode::InvalidBoundary)
            }
            other => panic!("expected fork error, got {other}"),
        }

        // unknown source id
        let error = store
            .fork(
                &ctx,
                SessionForkSource::Id(session_id("missing")),
                None,
                None,
            )
            .await
            .unwrap_err();
        match error {
            ForkError::Fork(SessionForkError { code, .. }) => {
                assert_eq!(code, SessionForkErrorCode::SessionNotFound)
            }
            other => panic!("expected fork error, got {other}"),
        }

        // boundary inside an open turn
        let error = store
            .fork(
                &ctx,
                SessionForkSource::Id(session_id("parent")),
                Some(1),
                None,
            )
            .await
            .unwrap_err();
        match error {
            ForkError::Fork(SessionForkError { code, .. }) => {
                assert_eq!(code, SessionForkErrorCode::OpenTurn)
            }
            other => panic!("expected fork error, got {other}"),
        }

        // taken child id
        let _child = store
            .create(&ctx, Some(session_id("taken")), None)
            .await
            .unwrap();
        let error = store
            .fork(
                &ctx,
                SessionForkSource::Id(session_id("parent")),
                Some(2),
                Some(session_id("taken")),
            )
            .await
            .unwrap_err();
        match error {
            ForkError::Fork(SessionForkError { code, .. }) => {
                assert_eq!(code, SessionForkErrorCode::SessionAlreadyExists)
            }
            other => panic!("expected fork error, got {other}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn enter_announce_separate_transaction() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);

        let announced = Arc::new(AtomicU32::new(0));
        let a = announced.clone();
        ctx.on(
            "session/created",
            Arc::new(move |_ctx, _args| {
                let a = a.clone();
                Box::pin(async move {
                    a.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;

        let session = store.prepare(Some(session_id("tx")), None).unwrap();
        assert!(
            store.get(&session_id("tx")).is_none(),
            "prepared session is not live"
        );

        let detach = store.enter(&session).unwrap();
        assert!(store.get(&session_id("tx")).is_some());
        assert_eq!(
            announced.load(MemOrder::SeqCst),
            0,
            "enter does not announce"
        );

        store.announce(&session).await.unwrap();
        assert_eq!(announced.load(MemOrder::SeqCst), 1);

        // re-announcing rejects
        let error = store.announce(&session).await.unwrap_err();
        assert!(error.contains("already announced"), "{error}");

        // detach removes from store and emits the disposal edge
        let disposed = Arc::new(AtomicU32::new(0));
        let d = disposed.clone();
        ctx.on(
            "session/disposed",
            Arc::new(move |_ctx, _args| {
                let d = d.clone();
                Box::pin(async move {
                    d.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            cordis::EventOptions::default(),
        )
        .await;
        detach().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(store.get(&session_id("tx")).is_none());
        assert_eq!(disposed.load(MemOrder::SeqCst), 1);

        // an unannounced entry detaches without the disposal pair
        let quiet = store.prepare(Some(session_id("quiet")), None).unwrap();
        let quiet_detach = store.enter(&quiet).unwrap();
        quiet_detach().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            disposed.load(MemOrder::SeqCst),
            1,
            "no second disposal edge"
        );
    }

    #[test]
    fn from_restore_validates_header_and_seed() {
        let header = SessionHeader {
            version: crate::SESSION_FORMAT_VERSION,
            id: session_id("r1"),
            created_at: 5,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        let seed = vec![SessionEvent {
            type_: "turn/start".to_string(),
            seq: 0,
            time: 1,
            data: serde_json::json!({"turn": 1}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }];
        let session = Session::from_restore(session_id("r1"), seed, &header).unwrap();
        assert_eq!(session.events().len(), 2);
        assert_eq!(session.header().created_at, 5);

        // a mismatched header rejects
        let bad_header = SessionHeader {
            id: session_id("other"),
            ..header.clone()
        };
        assert!(Session::from_restore(session_id("r1"), Vec::new(), &bad_header).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn store_mints_unique_ids() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let a = store.prepare(None, None).unwrap();
        let b = store.prepare(None, None).unwrap();
        assert_eq!(a.id().as_str(), "session-1");
        assert_eq!(b.id().as_str(), "session-2");
        assert_ne!(a.id(), b.id());
    }
}
