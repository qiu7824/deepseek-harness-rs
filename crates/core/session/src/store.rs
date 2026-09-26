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
//! - `Session::append` stays synchronous; observers must complete inline.
//!   Pending observers are rejected with a diagnostic.
//! - `deepFreeze`/`structuredClone` collapse to the identity function:
//!   Rust values are owned.
//! - Pre-commit hooks run outside the state mutex while publication ownership
//!   serializes writers; recursive writes are rejected.

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
    CreateSessionOptions, EpochHeader, LEGACY_SESSION_FORMAT_VERSION, RequestContext, SessionEvent,
    SessionHeader, SessionId, SessionLogOffset, SessionSeq, SurfaceIntent, end_seed_data,
    session_id, snapshot_session_header, validate_session_header,
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
    if !crate::surface::is_surface_eligible_type(type_) {
        return Ok(());
    }
    assert_message_event_shape(
        type_,
        record.get("data"),
        &format!("seed {type_} at index {index}"),
    )
}

/// Validate only the event-specific invariants needed to safely replay a
/// message (TS `assertMessageEventShape`).
fn assert_message_event_shape(
    type_: &str,
    data_value: Option<&JsonValue>,
    subject: &str,
) -> Result<(), String> {
    if !crate::surface::is_surface_eligible_type(type_) {
        return Ok(());
    }
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
    let expected_role = match type_ {
        "assistant/message" => "assistant",
        "system/message" => "system",
        "developer/message" => "developer",
        "tool/result" if message_record.get("role").and_then(JsonValue::as_str) == Some("tool") => {
            "tool"
        }
        _ => "user",
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
    if expected_role == "tool" {
        if message_record.get("toolCallId") != source.get("callId") {
            return Err(format!("{subject} message has mismatched tool call ids"));
        }
        if message_record
            .get("isError")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(format!("{subject} message has invalid isError"));
        }
        if content
            .iter()
            .any(|block| block.get("type").and_then(JsonValue::as_str) == Some("tool-result"))
        {
            return Err(format!(
                "{subject} tool message must contain flat result content"
            ));
        }
        return Ok(());
    }
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
    if type_ == "request/header" && data.pointer("/header/system").is_some() {
        return Err(format!(
            "{location} must omit header.system; use system/message"
        ));
    }
    Ok(())
}

// ---- Session ----

/// Mutable log state of one session.
#[derive(Default)]
pub(crate) struct SessionState {
    // Full snapshots share the durable log. Append only copies when a
    // consumer still owns a previous immutable snapshot.
    log: Arc<Vec<SessionEvent>>,
    archive: Option<Arc<crate::event_archive::EventArchive>>,
    surface: SurfaceManager,
    header_fold: Option<EpochHeader>,
    header_fold_seq: usize,
    context_fold: Option<RequestContext>,
    context_fold_seq: usize,
    derived: Arc<Vec<Message>>,
    derived_nodes: usize,
    derived_generation: u64,
}

impl SessionState {
    fn prefix_len(&self) -> usize {
        self.archive.as_ref().map_or(0, |archive| archive.len())
    }

    fn len(&self) -> usize {
        self.prefix_len() + self.log.len()
    }

    fn read_event(&self, index: usize) -> Result<Option<SessionEvent>, String> {
        let prefix = self.prefix_len();
        if index < prefix {
            return self.archive.as_ref().unwrap().read(index);
        }
        Ok(self.log.get(index - prefix).cloned())
    }

    fn visit(
        &self,
        start: usize,
        end: usize,
        mut visitor: impl FnMut(&SessionEvent) -> Result<bool, String>,
    ) -> Result<(), String> {
        let prefix = self.prefix_len();
        let mut keep_going = true;
        if let Some(archive) = &self.archive {
            archive.visit(start.min(prefix)..end.min(prefix), |event| {
                keep_going = visitor(event)?;
                Ok(keep_going)
            })?;
        }
        if keep_going && end > prefix {
            for event in &self.log[start.saturating_sub(prefix)..end - prefix] {
                if !visitor(event)? {
                    break;
                }
            }
        }
        Ok(())
    }

    fn snapshot(&self, start: usize, end: usize) -> Arc<Vec<SessionEvent>> {
        if self.archive.is_none() && start == 0 && end == self.log.len() {
            return self.log.clone();
        }
        let mut events = Vec::with_capacity(end - start);
        self.visit(start, end, |event| {
            events.push(event.clone());
            Ok(true)
        })
        .expect("private Session event archive must remain readable");
        Arc::new(events)
    }

    fn validate_next(&mut self, event: &SessionEvent) -> Result<(), String> {
        let prefix = self.prefix_len();
        let length = self.len();
        let archive = &self.archive;
        let log = &self.log;
        self.surface.validate_indexed(length as u64, event, |seq| {
            let index = seq as usize;
            if index < prefix {
                archive.as_ref().unwrap().read(index)
            } else {
                Ok(log.get(index - prefix).cloned())
            }
        })
    }

    fn push_validated(&mut self, event: SessionEvent) {
        self.surface.commit_indexed(&event);
        Arc::make_mut(&mut self.log).push(event);
    }
}

/// A coherent, read-only event prefix available inside a Session read or
/// conditional append. Events are owned only for the duration of each read;
/// cold payloads are never promoted to a whole-history cache.
pub struct SessionEventReader<'a> {
    state: &'a SessionState,
}

impl SessionEventReader<'_> {
    pub fn len(&self) -> u64 {
        self.state.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn read(&self, seq: u64) -> Result<Option<SessionEvent>, String> {
        let Ok(index) = usize::try_from(seq) else {
            return Ok(None);
        };
        self.state.read_event(index)
    }

    pub fn derive_event_message(&self, event: &SessionEvent) -> Option<Message> {
        derive_event_message(event)
            .map(|message| self.state.surface.project_message(event.seq.get(), message))
    }

    pub fn visit(
        &self,
        from_seq: u64,
        to_seq_exclusive: Option<u64>,
        visitor: impl FnMut(&SessionEvent) -> Result<bool, String>,
    ) -> Result<(), String> {
        let end = to_seq_exclusive.unwrap_or(self.len()).min(self.len()) as usize;
        let start = from_seq.min(end as u64) as usize;
        self.state.visit(start, end, visitor)
    }

    pub fn find_rev(
        &self,
        mut predicate: impl FnMut(&SessionEvent) -> bool,
    ) -> Result<Option<SessionEvent>, String> {
        if let Some(event) = self.state.log.iter().rev().find(|event| predicate(event)) {
            return Ok(Some(event.clone()));
        }
        if let Some(archive) = &self.state.archive {
            for index in (0..archive.len()).rev() {
                let event = archive.read(index)?.expect("index is in captured prefix");
                if predicate(&event) {
                    return Ok(Some(event));
                }
            }
        }
        Ok(None)
    }
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
    pub first_live_seq: SessionLogOffset,
    pub inherited_event_count: SessionLogOffset,
    pub state: Mutex<SessionState>,
    derived_caches: Mutex<HashMap<std::any::TypeId, Arc<dyn std::any::Any + Send + Sync>>>,
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
        inherited_event_count: Option<SessionLogOffset>,
    ) -> Result<Session, String> {
        Self::construct(id, seed, header, false, inherited_event_count)
    }

    /// Restore a detached session by taking ownership of fresh persistence
    /// values (TS `Session.fromRestore`).
    pub fn from_restore(
        id: SessionId,
        seed: Vec<SessionEvent>,
        header: &SessionHeader,
        inherited_event_count: SessionLogOffset,
    ) -> Result<Session, String> {
        Self::construct(
            id,
            Some(seed),
            Some(header),
            true,
            Some(inherited_event_count),
        )
    }

    /// Restore a complete, dense cold prefix without retaining its bodies
    /// in the resident event vector. Full snapshots remain explicit reads
    /// of the same immutable events, including every historical chunk.
    pub fn from_event_archive(
        id: SessionId,
        archive: crate::event_archive::EventArchive,
        header: &SessionHeader,
        inherited_event_count: SessionLogOffset,
        closers: Vec<SessionEvent>,
    ) -> Result<Session, String> {
        let header = validate_session_header(
            &id,
            &serde_json::to_value(header).map_err(|error| error.to_string())?,
        )?;
        if (!header.is_seeded && inherited_event_count != SessionLogOffset::ZERO)
            || inherited_event_count.get() > archive.len() as u64
        {
            return Err("invalid inherited prefix for Session event archive".into());
        }
        let mut surface = crate::StreamingSurfaceFold::default();
        let mut last_type = None;
        let mut header_fold = None;
        let mut context_fold = None;
        archive.visit(0..archive.len(), |event| {
            let index = event.seq.get() as usize;
            let value = serde_json::to_value(event).map_err(|error| error.to_string())?;
            assert_session_event_envelope(&value, index)?;
            drop(value);
            assert_supported_request_header(
                &event.type_,
                &event.data,
                &format!("seed event at index {index}"),
            )?;
            surface.push(event)?;
            header_fold = crate::request_header::fold_request_header(
                std::slice::from_ref(event),
                header_fold.take(),
            );
            if event.type_ == "request/context" {
                context_fold = serde_json::from_value::<RequestContext>(event.data.clone()).ok();
            }
            last_type = Some(event.type_.clone());
            Ok(true)
        })?;
        let prefix_len = archive.len();
        let mut state = SessionState {
            archive: Some(Arc::new(archive)),
            surface: surface.into_manager(),
            header_fold,
            header_fold_seq: prefix_len,
            context_fold,
            context_fold_seq: prefix_len,
            ..Default::default()
        };
        for event in closers {
            last_type = Some(event.type_.clone());
            state.validate_next(&event)?;
            state.push_validated(event);
        }
        let first_live_seq = SessionLogOffset::new(state.len() as u64)?;
        let inherited_marker =
            header.is_seeded && inherited_event_count.get() == state.len() as u64;
        if inherited_marker || last_type.as_deref() != Some("session/end-seed") {
            let event = SessionEvent {
                type_: "session/end-seed".into(),
                seq: SessionSeq::new(state.len() as u64)?,
                time: now_ms(),
                data: if inherited_marker {
                    serde_json::json!({"inherited":true})
                } else {
                    end_seed_data()
                },
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            };
            state.validate_next(&event)?;
            state.push_validated(event);
        }
        Ok(Session {
            inner: Arc::new(SessionInner {
                id,
                header,
                first_live_seq,
                inherited_event_count,
                state: Mutex::new(state),
                derived_caches: Mutex::new(HashMap::new()),
            }),
        })
    }

    fn construct(
        id: SessionId,
        seed: Option<Vec<SessionEvent>>,
        header: Option<&SessionHeader>,
        _restore: bool,
        supplied_inherited_event_count: Option<SessionLogOffset>,
    ) -> Result<Session, String> {
        let had_seed = seed.is_some();
        let mut owned_header = header.cloned();
        let mut seed = seed;
        let mut supplied_inherited_event_count = supplied_inherited_event_count;
        if let Some(meta) = owned_header
            .as_mut()
            .filter(|meta| matches!(meta.version, 0 | 3))
        {
            let mut events = seed.take().unwrap_or_default();
            let mut inherited = supplied_inherited_event_count.unwrap_or(SessionLogOffset::ZERO);
            if meta.version == LEGACY_SESSION_FORMAT_VERSION {
                let report = crate::migrate_v0_to_v3(meta.clone(), &events)?;
                inherited = SessionLogOffset::new(
                    *report
                        .source_cuts
                        .get(inherited.get() as usize)
                        .ok_or("inherited cut exceeds historical seed")? as u64,
                )?;
                *meta = report.header;
                events = report.events;
            }
            if !_restore && meta.is_seeded && inherited.get() == events.len() as u64 {
                events.push(SessionEvent {
                    type_: "session/end-seed".into(),
                    seq: SessionSeq::new(events.len() as u64)?,
                    time: now_ms(),
                    data: serde_json::json!({"inherited":true}),
                    ignorable: None,
                    surface_op: None,
                    source_event_seqs: None,
                });
            }
            let (current, cut, current_events) =
                crate::format_v4::upgrade_v3_events(meta.clone(), inherited, events, vec![])?;
            *meta = current;
            supplied_inherited_event_count = Some(cut);
            seed = had_seed.then_some(current_events);
        }
        let header = owned_header.as_ref();
        let mut state = SessionState::default();
        if let Some(seed) = seed {
            for (index, snapshot) in seed.into_iter().enumerate() {
                let snapshot_value = serde_json::to_value(&snapshot).map_err(|_| {
                    format!("seed event at index {index} is not losslessly JSON-serializable")
                })?;
                // SessionEvent and its JSON values are already owned. Validate
                // the serialized envelope, then move the original record into
                // the log instead of cloning and deserializing it repeatedly.
                assert_session_event_envelope(&snapshot_value, index)?;
                assert_supported_request_header(
                    &snapshot.type_,
                    &snapshot.data,
                    &format!("seed event at index {index}"),
                )?;
                if snapshot.seq.get() != index as u64 {
                    return Err(format!(
                        "seed event at index {index} has seq {} (expected {index}); seed must be contiguous from 0",
                        snapshot.seq
                    ));
                }
                state
                    .validate_next(&snapshot)
                    .map_err(|error| format!("invalid seed event at index {index}: {error}"))?;
                state.push_validated(snapshot);
            }
        }
        let first_live_seq = SessionLogOffset::new(state.log.len() as u64)?;
        let header = match header {
            Some(header) => {
                let value = serde_json::to_value(header).map_err(|_| {
                    "session header is not losslessly JSON-serializable".to_string()
                })?;
                validate_session_header(&id, &value)?
            }
            None => snapshot_session_header(&id, None)?,
        };
        if header.is_seeded && !had_seed {
            return Err("seeded session requires an explicit constructor seed".to_string());
        }
        if header.is_seeded && supplied_inherited_event_count.is_none() {
            return Err("seeded session requires an inherited event count".to_string());
        }
        let inherited_event_count =
            supplied_inherited_event_count.unwrap_or(SessionLogOffset::ZERO);
        if !header.is_seeded && inherited_event_count != SessionLogOffset::ZERO {
            return Err("unseeded session inherited event count must be 0".to_string());
        }
        if inherited_event_count.get() > state.log.len() as u64 {
            return Err("session inherited event count exceeds its event log".to_string());
        }
        // Appended here so the marker is already in `events` when a backend
        // captures the creation seed; re-marking is skipped.
        let inherited_marker =
            header.is_seeded && inherited_event_count.get() == state.log.len() as u64;
        if had_seed
            && (inherited_marker
                || state.log.last().map(|event| event.type_.as_str()) != Some("session/end-seed"))
        {
            let event = SessionEvent {
                type_: "session/end-seed".to_string(),
                seq: SessionSeq::new(state.log.len() as u64)
                    .expect("a Rust Vec length fits the public Session wire"),
                time: now_ms(),
                data: if inherited_marker {
                    serde_json::json!({"inherited":true})
                } else {
                    end_seed_data()
                },
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            };
            state
                .validate_next(&event)
                .expect("the end-seed marker carries no surface metadata");
            state.push_validated(event);
        }
        Ok(Session {
            inner: Arc::new(SessionInner {
                id,
                header,
                first_live_seq,
                inherited_event_count,
                state: Mutex::new(state),
                derived_caches: Mutex::new(HashMap::new()),
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

    /// Share a disposable projection cache within this exact Session's
    /// lifetime. The cache is neither persisted nor shared by resumed or
    /// independently created sessions that have the same durable id.
    /// Cache values must not retain this Session or its owning agent.
    pub fn derived_cache<T: Default + Send + Sync + 'static>(&self) -> Arc<T> {
        self.inner
            .derived_caches
            .lock()
            .entry(std::any::TypeId::of::<T>())
            .or_insert_with(|| Arc::new(T::default()))
            .clone()
            .downcast::<T>()
            .expect("derived cache type agrees with its TypeId")
    }

    /// Only attached sessions emit lifecycle events that can retire service
    /// caches. Detached snapshots must not acquire persistent observer state.
    pub fn is_attached_to_store(&self) -> bool {
        attachment_of(self).is_some()
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_lifetime(&self) -> std::sync::Weak<dyn std::any::Any + Send + Sync> {
        let value: Arc<dyn std::any::Any + Send + Sync> = self.inner.clone();
        Arc::downgrade(&value)
    }

    /// Detached, deep-frozen creation metadata.
    pub fn header(&self) -> &SessionHeader {
        &self.inner.header
    }

    /// The first seq appended IN THIS PROCESS (TS `firstLiveSeq`).
    pub fn first_live_seq(&self) -> SessionLogOffset {
        self.inner.first_live_seq
    }

    /// Number of leading events inherited from this Session's fork parent.
    pub fn inherited_event_count(&self) -> SessionLogOffset {
        self.inner.inherited_event_count
    }

    /// Materialize an immutable snapshot of a half-open event range.
    pub fn snapshot_events(
        &self,
        from_seq: SessionLogOffset,
        to_seq_exclusive: Option<SessionLogOffset>,
    ) -> Arc<Vec<SessionEvent>> {
        let state = self.inner.state.lock();
        let end = to_seq_exclusive
            .map(|value| value.get() as usize)
            .unwrap_or(state.len())
            .min(state.len());
        let start = (from_seq.get() as usize).min(end);
        state.snapshot(start, end)
    }

    /// Return this Session's child-owned events after its inherited prefix.
    pub fn own_events(&self) -> Arc<Vec<SessionEvent>> {
        self.snapshot_events(self.inherited_event_count(), None)
    }

    /// Compatibility wrapper for Rust consumers while they migrate to the
    /// explicit-cost snapshot/read APIs.
    pub fn events(&self) -> Arc<Vec<SessionEvent>> {
        self.snapshot_events(SessionLogOffset::ZERO, None)
    }

    /// Compatibility boundary for consumers that require a complete slice.
    /// Cold prefixes are explicitly materialized here; resident consumers
    /// should use `with_event_reader` or `visit_events` instead. The callback
    /// must not re-enter this Session.
    pub fn with_events<R>(&self, read: impl FnOnce(&[SessionEvent]) -> R) -> R {
        let state = self.inner.state.lock();
        if state.archive.is_none() {
            read(&state.log)
        } else {
            read(&state.snapshot(0, state.len()))
        }
    }

    /// Read a coherent prefix through bounded indexed/visitor operations.
    /// The callback must not re-enter this Session.
    pub fn with_event_reader<R>(&self, read: impl FnOnce(&SessionEventReader<'_>) -> R) -> R {
        let state = self.inner.state.lock();
        read(&SessionEventReader { state: &state })
    }

    /// Read one coherent surface and indexed event prefix without expanding
    /// the cold log. The callback must not re-enter this Session.
    pub fn with_surface_reader<R>(
        &self,
        read: impl FnOnce(&SessionEventReader<'_>, &[u64]) -> R,
    ) -> R {
        let state = self.inner.state.lock();
        read(
            &SessionEventReader { state: &state },
            state.surface.current_nodes(),
        )
    }

    /// Visit a coherent half-open range without materializing an immutable
    /// full-history snapshot. Return false to stop at the current event.
    /// The callback must not re-enter this Session.
    pub fn visit_events(
        &self,
        from_seq: u64,
        to_seq_exclusive: Option<u64>,
        visitor: impl FnMut(&SessionEvent) -> Result<bool, String>,
    ) -> Result<(), String> {
        self.with_event_reader(|reader| reader.visit(from_seq, to_seq_exclusive, visitor))
    }

    /// Read only the latest matching event. The predicate must not re-enter
    /// this Session; the returned event is independently owned.
    pub fn find_event_rev(
        &self,
        predicate: impl FnMut(&SessionEvent) -> bool,
    ) -> Result<Option<SessionEvent>, String> {
        self.with_event_reader(|reader| reader.find_rev(predicate))
    }

    /// Fallible indexed access for consumers that can propagate archive I/O
    /// failures instead of using the historical infallible snapshot API.
    pub fn read_event(&self, seq: u64) -> Result<Option<SessionEvent>, String> {
        self.with_event_reader(|reader| reader.read(seq))
    }

    /// Whether one existing event belongs to this Session rather than its parent.
    pub fn is_own_seq(&self, seq: SessionSeq) -> bool {
        seq.get() >= self.inherited_event_count().get() && seq.get() < self.seq().get()
    }

    /// Clone only the durable tail at or after `from_seq` without
    /// materializing the full immutable [`Self::events`] snapshot.
    pub fn events_from(&self, from_seq: u64) -> Vec<SessionEvent> {
        let Ok(start) = usize::try_from(from_seq) else {
            return Vec::new();
        };
        let state = self.inner.state.lock();
        let events = state.snapshot(start.min(state.len()), state.len());
        Arc::try_unwrap(events).unwrap_or_else(|events| events.as_ref().clone())
    }

    /// Clone the prefix through the last event matching `predicate`, while
    /// holding the session lock only once and never materializing a full-log
    /// snapshot first.
    pub fn prefix_through_last(
        &self,
        predicate: impl Fn(&SessionEvent) -> bool,
    ) -> Vec<SessionEvent> {
        let state = self.inner.state.lock();
        let mut last = None;
        state
            .visit(0, state.len(), |event| {
                if predicate(event) {
                    last = Some(event.seq.get() as usize);
                }
                Ok(true)
            })
            .expect("private Session event archive must remain readable");
        last.map_or_else(Vec::new, |last| {
            let events = state.snapshot(0, last + 1);
            Arc::try_unwrap(events).unwrap_or_else(|events| events.as_ref().clone())
        })
    }

    /// Clone one event by durable sequence without materializing the full
    /// immutable [`Self::events`] snapshot.
    pub fn event_at(&self, seq: SessionSeq) -> Option<SessionEvent> {
        let index = seq.get() as usize;
        self.inner
            .state
            .lock()
            .read_event(index)
            .expect("private Session event archive must remain readable")
    }

    /// The next event's sequence number — always the log length.
    pub fn seq(&self) -> SessionLogOffset {
        SessionLogOffset::new(self.inner.state.lock().len() as u64)
            .expect("a Rust Vec length fits the public Session wire")
    }

    /// The ordered surface over this session's event log (snapshot per
    /// call; TS returns the live manager view).
    pub fn surface(&self) -> Result<crate::surface::SessionSurface, String> {
        let state = &mut *self.inner.state.lock();
        let nodes = state.surface.current_nodes().to_vec();
        let replace_generation = state.surface.current_generation();
        Ok(crate::surface::SessionSurface {
            nodes,
            replace_generation,
        })
    }

    /// Compatibility boundary for a complete event slice and its coherent
    /// surface. Cold prefixes are explicitly materialized; bounded readers
    /// should use `with_surface_reader`. The callback must not re-enter.
    pub fn with_surface_events<R>(
        &self,
        read: impl FnOnce(&[SessionEvent], &[u64]) -> R,
    ) -> Result<R, String> {
        let state = &mut *self.inner.state.lock();
        let nodes = state.surface.current_nodes().to_vec();
        Ok(read(&state.snapshot(0, state.len()), &nodes))
    }

    /// Append one typed event to the log and notify observers via the
    /// store-owned publication hooks (TS `Session.append`).
    pub fn append(
        &self,
        type_: &str,
        data: JsonValue,
        intent: Option<SurfaceIntent>,
    ) -> Result<SessionEvent, String> {
        self.append_core(
            type_,
            data,
            intent,
            None::<fn(&SessionEventReader<'_>) -> Result<bool, String>>,
        )?
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
        self.append_core(
            type_,
            data,
            intent,
            Some(|reader: &SessionEventReader<'_>| {
                Ok(condition(&reader.state.snapshot(0, reader.state.len())))
            }),
        )
    }

    /// Atomically evaluate a condition through bounded event reads before
    /// appending. This is the cold-prefix-safe conditional append boundary.
    pub fn append_if_read<F>(
        &self,
        type_: &str,
        data: JsonValue,
        intent: Option<SurfaceIntent>,
        condition: F,
    ) -> Result<Option<SessionEvent>, String>
    where
        F: FnOnce(&SessionEventReader<'_>) -> Result<bool, String>,
    {
        self.append_core(type_, data, intent, Some(condition))
    }

    fn append_core<F>(
        &self,
        type_: &str,
        data: JsonValue,
        intent: Option<SurfaceIntent>,
        condition: Option<F>,
    ) -> Result<Option<SessionEvent>, String>
    where
        F: FnOnce(&SessionEventReader<'_>) -> Result<bool, String>,
    {
        let data_snapshot = snapshot_json_value(&data).ok_or_else(|| {
            format!("session event \"{type_}\" carries non-JSON-serializable data")
        })?;
        assert_supported_request_header(
            type_,
            &data_snapshot,
            &format!("session event \"{type_}\""),
        )?;
        assert_message_event_shape(
            type_,
            Some(&data_snapshot),
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
        // Release publication ownership even if validation or a pre-hook panics.
        struct AppendGuard(Option<Arc<SessionEntry>>);
        impl Drop for AppendGuard {
            fn drop(&mut self) {
                if let Some(entry) = &self.0
                    && entry.finish_append()
                {
                    entry.detach_now();
                }
            }
        }
        let _guard = AppendGuard(entry.clone());
        let mut state = self.inner.state.lock();
        if let Some(condition) = condition {
            if !condition(&SessionEventReader { state: &state })? {
                return Ok(None);
            }
        }
        let event = SessionEvent {
            type_: type_.to_string(),
            seq: SessionSeq::new(state.len() as u64)?,
            time: now_ms(),
            data: data_snapshot,
            ignorable: matches!(
                type_,
                "request/phase" | "tools/discovery" | "computer-use/activity"
            )
            .then_some(true),
            surface_op: intent.as_ref().map(|intent| intent.surface_op.clone()),
            source_event_seqs: intent.and_then(|intent| intent.source_event_seqs),
        };
        {
            let state = &mut *state;
            state.validate_next(&event)?;
        }
        if let Some(entry) = &entry {
            // The append guard serializes writers while pre-commit hooks read
            // the durable prefix without holding the session state mutex.
            drop(state);
            let dispatch_ctx = entry.emit_ctx.with_filter(entry.carrier.filter.clone());
            let args: Vec<ArcValue> = vec![arc(self.clone()), arc(event.clone())];
            let listeners = entry.emit_ctx.events.collect(
                DispatchMode::Emit,
                Some(&dispatch_ctx),
                "session/event",
                &args,
            );
            self.inner.state.lock().push_validated(event.clone());
            invoke_contained_session_observers(
                &entry.emit_ctx,
                "session/event",
                &entry.id,
                &args,
                &listeners,
            );
        } else {
            state.push_validated(event.clone());
        }
        Ok(Some(event))
    }

    /// The [`EpochHeader`] in force after the log's last header event.
    pub fn request_header(&self) -> Option<EpochHeader> {
        let state = &mut *self.inner.state.lock();
        if state.header_fold_seq < state.len() {
            let mut fold = state.header_fold.clone();
            state
                .visit(state.header_fold_seq, state.len(), |event| {
                    fold = crate::request_header::fold_request_header(
                        std::slice::from_ref(event),
                        fold.take(),
                    );
                    Ok(true)
                })
                .expect("private Session event archive must remain readable");
            state.header_fold = fold;
            state.header_fold_seq = state.len();
        }
        state.header_fold.clone()
    }

    /// The latest resolved route metadata, or `None` before the first
    /// `request/context` event.
    pub fn request_context(&self) -> Option<RequestContext> {
        let state = &mut *self.inner.state.lock();
        if state.context_fold_seq < state.len() {
            let mut fold = state.context_fold.clone();
            state
                .visit(state.context_fold_seq, state.len(), |event| {
                    if event.type_ == "request/context" {
                        fold = serde_json::from_value::<RequestContext>(event.data.clone()).ok();
                    }
                    Ok(true)
                })
                .expect("private Session event archive must remain readable");
            state.context_fold = fold;
            state.context_fold_seq = state.len();
        }
        state.context_fold.clone()
    }

    /// Derive the LLM message history by walking the ordered sequences of
    /// message-producing events maintained by `surfaceOp` markers.
    pub fn derive_messages(&self) -> Result<Arc<Vec<Message>>, String> {
        let state = &mut *self.inner.state.lock();
        let nodes = state.surface.current_nodes().to_vec();
        let generation = state.surface.current_generation();
        if generation != state.derived_generation {
            state.derived = Arc::new(Vec::new());
            state.derived_nodes = 0;
            state.derived_generation = generation;
        }
        if state.derived_nodes < nodes.len() {
            let start = state.derived_nodes;
            let mut additions = Vec::new();
            for seq in &nodes[start..] {
                if let Some(event) = state.read_event(*seq as usize)?
                    && let Some(message) = derive_event_message(&event)
                {
                    additions.push(state.surface.project_message(event.seq.get(), message));
                }
            }
            Arc::make_mut(&mut state.derived).extend(additions);
            state.derived_nodes = nodes.len();
        }
        Ok(Arc::clone(&state.derived))
    }

    /// Instance face of the pure per-node `deriveEventMessage` export.
    pub fn derive_event_message(&self, event: &SessionEvent) -> Option<Message> {
        let state = &mut *self.inner.state.lock();
        derive_event_message(event)
            .map(|message| state.surface.project_message(event.seq.get(), message))
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
        let prefix = format!("session \"{}\": {name} listener", id.as_str());
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let future = callback(listener_ctx, args.to_vec());
            drive_observer_inline(future);
        }));
        if let Err(payload) = outcome {
            logger.warn(vec![arc(format!(
                "{prefix} threw: {}",
                render_panic(payload)
            ))]);
        }
    }
}

/// Session observers have a synchronous publication contract. A pending
/// observer violates that contract: waiting would deadlock a current-thread
/// runtime (or a task awaited by its own publisher). Reject it inside the
/// per-listener panic boundary and let subsequent observers run.
fn drive_observer_inline(mut future: cordis::BoxFuture<'static, Option<ArcValue>>) {
    let waker = futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    assert!(
        future.as_mut().poll(&mut context).is_ready(),
        "session observers must complete synchronously; spawn background work explicitly"
    );
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
            is_seeded: meta
                .as_ref()
                .and_then(|meta| meta.is_seeded)
                .unwrap_or(false),
            origin: meta.as_ref().and_then(|meta| meta.origin.clone()),
            delegation_depth: meta.as_ref().and_then(|meta| meta.delegation_depth),
            agent_preset: meta.as_ref().and_then(|meta| meta.agent_preset.clone()),
        };
        Session::create(
            session_id,
            options.seed,
            Some(&header),
            options.inherited_event_count,
        )
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
            is_seeded: Some(true),
            ..Default::default()
        };
        self.create(
            caller,
            child_session_id,
            Some(CreateSessionOptions {
                inherited_event_count: Some(
                    SessionLogOffset::new(seed.len() as u64).map_err(ForkError::Store)?,
                ),
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
                Some(last) => last.seq.get(),
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
mod ownership_tests {
    use super::*;

    fn record(seq: u64) -> SessionEvent {
        serde_json::from_value(serde_json::json!({
            "seq":seq,"time":1700000000000i64,"type":"user/message","surfaceOp":"append",
            "data":{"id":"owned-message","role":"user","content":[{"type":"text","text":"x".repeat(128 * 1024)}],"source":{"kind":"user"}}
        })).unwrap()
    }

    #[test]
    fn restore_moves_large_owned_payloads_without_reserializing_them() {
        let event = record(0);
        let address = event
            .data
            .pointer("/content/0/text")
            .unwrap()
            .as_str()
            .unwrap()
            .as_ptr();
        let header = snapshot_session_header(&session_id("owned-restore"), None).unwrap();
        let restored = Session::from_restore(
            header.id.clone(),
            vec![event],
            &header,
            SessionLogOffset::ZERO,
        )
        .unwrap();
        assert_eq!(
            restored.events()[0]
                .data
                .pointer("/content/0/text")
                .unwrap()
                .as_str()
                .unwrap()
                .as_ptr(),
            address
        );
    }

    #[test]
    fn owned_restore_keeps_envelope_and_sequence_validation() {
        let header = snapshot_session_header(&session_id("invalid-restore"), None).unwrap();
        assert!(
            Session::from_restore(
                header.id.clone(),
                vec![record(1)],
                &header,
                SessionLogOffset::ZERO
            )
            .is_err()
        );
        let mut missing_surface = record(0);
        missing_surface.surface_op = None;
        assert!(
            Session::from_restore(
                header.id.clone(),
                vec![missing_surface],
                &header,
                SessionLogOffset::ZERO
            )
            .is_err()
        );
    }

    #[test]
    fn archived_restore_keeps_every_event_and_validates_live_surface_rewrites() {
        let mut seed = vec![record(0)];
        seed.push(serde_json::from_value(serde_json::json!({
            "seq":1,"time":1700000000000i64,"type":"tool/result","surfaceOp":"append",
            "data":{"turn":1,"step":1,"message":{
                "id":"result-1","role":"tool","source":{"kind":"tool","callId":"call-1"},
                "toolCallId":"call-1","isError":false,"content":[{"type":"text","text":"original"}]
            }},"sourceEventSeqs":[0]
        })).unwrap());
        for (offset, kind) in [
            "assistant/chunk",
            "todo/write",
            "goal/change",
            "agent/inbox/spliced",
            "session/title",
        ]
        .into_iter()
        .enumerate()
        {
            seed.push(SessionEvent {
                type_: kind.into(),
                seq: SessionSeq::new(offset as u64 + 2).unwrap(),
                time: 1700000000001,
                data: serde_json::json!({"opaque":[kind,{"exact":"历史\n"}]}),
                surface_op: None,
                source_event_seqs: None,
                ignorable: None,
            });
        }
        let mut builder =
            crate::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        for event in &seed {
            builder.push(event).unwrap();
        }
        let header = snapshot_session_header(&session_id("archive-preservation"), None).unwrap();
        let restored = Session::from_event_archive(
            header.id.clone(),
            builder.finish().unwrap(),
            &header,
            SessionLogOffset::ZERO,
            vec![],
        )
        .unwrap();
        assert_eq!(restored.first_live_seq().get(), seed.len() as u64);
        assert_eq!(restored.seq().get(), seed.len() as u64 + 1);
        let mut replayed = Vec::new();
        restored
            .visit_events(0, Some(seed.len() as u64), |event| {
                replayed.push(event.clone());
                Ok(true)
            })
            .unwrap();
        assert_eq!(replayed, seed);
        assert_eq!(restored.read_event(1).unwrap(), Some(seed[1].clone()));
        assert_eq!(
            restored
                .find_event_rev(|event| event.type_ == "todo/write")
                .unwrap(),
            Some(seed[3].clone())
        );
        assert_eq!(&restored.events()[..seed.len()], seed.as_slice());

        let mut rewritten = seed[1].data.clone();
        rewritten["message"]["content"][0]["text"] = "rewritten".into();
        let intent = SurfaceIntent {
            surface_op: crate::SurfaceOp::Replace { start: 1, end: 1 },
            source_event_seqs: Some(vec![1]),
        };
        let mut invalid = rewritten.clone();
        invalid["message"]["id"] = "different-id".into();
        let before = restored.seq();
        assert!(
            restored
                .append("tool/result", invalid, Some(intent.clone()))
                .is_err()
        );
        assert_eq!(restored.seq(), before);
        let appended = restored
            .append("tool/result", rewritten, Some(intent))
            .unwrap();
        assert_eq!(appended.seq.get(), before.get());
        assert_eq!(
            restored.surface().unwrap().nodes,
            vec![0, appended.seq.get()]
        );
        let messages = restored.derive_messages().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1].content,
            vec![dsh_llm::ContentBlock::Text {
                text: "rewritten".into()
            }]
        );
        assert_eq!(restored.read_event(1).unwrap(), Some(seed[1].clone()));

        assert!(
            restored
                .append_if_read(
                    "session/title",
                    serde_json::json!({"title":"next"}),
                    None,
                    |reader| {
                        assert_eq!(reader.len(), appended.seq.get() + 1);
                        Ok(reader
                            .find_rev(|event| event.type_ == "session/title")?
                            .is_none())
                    }
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(restored.seq().get(), appended.seq.get() + 1);
    }
}
