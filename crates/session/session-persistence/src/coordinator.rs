#![allow(clippy::type_complexity, clippy::redundant_allocation)]
// Nested service handles and preparation callbacks preserve the public lifecycle seams.

//! Shared buffering, serialization, adoption, repair, and disposal
//! orchestration for first-party backends. Rust port of
//! `packages/session/session-persistence/src/coordinator.ts`.
//!
//! # Deviations
//!
//! - `AbortSignal` parameters are omitted (no cancellation wiring yet).
//! - Per-id serialization uses one `tokio::sync::Mutex` per id instead of the
//!   TS promise chain (observably equivalent: strict per-id ordering, errors
//!   never poison the chain).
//! - Operations return `Result<T, String>`; TS rejects with arbitrary values.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use cordis::{Context, EventOptions, Listener, arc, downcast, make_disposer};
use dsh_session::{
    SESSION_FORMAT_VERSION, Session, SessionEvent, SessionHeader, SessionId, SessionStore,
    interrupted_turn_closers, snapshot_json_value,
};
use dsh_timeout::MAX_TIMER_DELAY_MS;
use futures::FutureExt;
use parking_lot::Mutex;
use serde_json::{Map, Value as JsonValue};

use crate::index::{SessionInspection, SessionLocation, SessionReadFromResult};
use crate::preparations::{
    PreparedSource, PreparedSourceLoader, SessionPreparationReservation, SessionPreparations,
};
use crate::revision::SessionPersistenceRevision;
use crate::write_behind::SessionWriteBehind;

/// A coordinator serialization operation.
pub type BoxOpFuture<T> = Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send>>;

/// Default number of detached session preparations retained by a coordinator.
pub const DEFAULT_PREPARED_SESSION_CACHE_SIZE: usize = 5;

/// Default maximum intentional wait before a live session batch starts
/// writing.
pub const DEFAULT_WRITE_BATCH_MAX_DELAY_MS: u64 = 200;

/// Largest write batching delay accepted by the timer implementation.
pub const MAX_WRITE_BATCH_DELAY_MS: u64 = MAX_TIMER_DELAY_MS;

/// Durable session contents failed validation after a successful backend
/// read.
#[derive(Debug)]
pub struct SessionPersistenceCorruptionError {
    pub message: String,
}

impl SessionPersistenceCorruptionError {
    pub fn new(message: String) -> Self {
        Self { message }
    }
}

impl std::fmt::Display for SessionPersistenceCorruptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessionPersistenceCorruptionError {}

/// The stored log is intact but this runtime cannot faithfully interpret it.
#[derive(Debug)]
pub struct SessionFormatUnsupportedError {
    pub message: String,
    pub location: Option<SessionLocation>,
}

impl SessionFormatUnsupportedError {
    pub fn new(message: String, location: Option<SessionLocation>) -> Self {
        Self { message, location }
    }
}

impl std::fmt::Display for SessionFormatUnsupportedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessionFormatUnsupportedError {}

/// Direction-aware refusal text for a stored session whose format version
/// this build does not read.
pub fn session_format_version_refusal(id: &SessionId, version: u64) -> String {
    if version > SESSION_FORMAT_VERSION {
        format!(
            "session \"{}\" uses log format v{version}, but this harness reads only v{SESSION_FORMAT_VERSION}: the log was written by a newer harness — upgrade the harness to open it",
            id.as_str()
        )
    } else {
        format!(
            "session \"{}\" uses log format v{version}, older than the supported v{SESSION_FORMAT_VERSION}, and this build ships no upgrade path for it",
            id.as_str()
        )
    }
}

/// Coordinator policy supplied by a concrete persistence backend.
#[derive(Debug, Clone)]
pub struct PersistenceCoordinatorOptions {
    pub prepared_session_cache_size: usize,
    pub write_batch_max_delay_ms: u64,
}

impl Default for PersistenceCoordinatorOptions {
    fn default() -> Self {
        Self {
            prepared_session_cache_size: DEFAULT_PREPARED_SESSION_CACHE_SIZE,
            write_batch_max_delay_ms: DEFAULT_WRITE_BATCH_MAX_DELAY_MS,
        }
    }
}

/// A stored session's header, valid contiguous event prefix, source-qualified
/// revision, and optional opaque torn-tail marker.
#[derive(Clone)]
pub struct StoredPrefix<TornMarker = ()> {
    pub meta: SessionHeader,
    pub events: Vec<SessionEvent>,
    pub revision: SessionPersistenceRevision,
    pub torn_marker: Option<TornMarker>,
}

/// A stored session's header plus the events at or past a requested seq.
#[derive(Clone)]
pub struct StoredSuffix {
    pub meta: SessionHeader,
    pub events: Vec<SessionEvent>,
}

/// The storage contract between [`PersistenceCoordinator`] and a concrete
/// backend.
#[async_trait::async_trait]
pub trait PersistenceBackend<TornMarker: Clone + Send + Sync + 'static = ()>: Send + Sync {
    /// Human-readable backend name.
    fn name(&self) -> &'static str;

    /// Read a stored prefix by id, scanning every backend storage scope.
    async fn load_stored(&self, id: &SessionId)
    -> Result<Option<StoredPrefix<TornMarker>>, String>;

    /// Read the current source-qualified revision for one stored session.
    async fn read_stored_revision(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionPersistenceRevision>, String>;

    /// Durably append a CONTIGUOUS batch, lazily materializing the session
    /// first when `!is_materialized`.
    async fn append_batch(
        &self,
        meta: &SessionHeader,
        events: &[SessionEvent],
        is_materialized: bool,
    ) -> Result<(), String>;

    /// Make a crash repair durable: truncate the torn tail (iff
    /// `torn_marker.is_some()`) and append `closers` (iff any).
    async fn commit_repair(
        &self,
        meta: &SessionHeader,
        torn_marker: Option<TornMarker>,
        closers: &[SessionEvent],
    ) -> Result<(), String>;

    /// Permanently remove one backend-owned session artifact.
    async fn delete_stored(&self, id: &SessionId) -> Result<bool, String> {
        let _ = id;
        Err("this persistence backend does not support deletion".to_string())
    }

    /// List all stored (materialized) sessions' metadata.
    async fn list(&self) -> Result<Vec<SessionHeader>, String>;

    /// Optional side-effect-free artifact locator.
    fn locate(&self, meta: &SessionHeader) -> Option<SessionLocation> {
        let _ = meta;
        None
    }

    /// Whether the backend can seek by seq (`readFrom` fast path).
    fn seek_capable(&self) -> bool {
        false
    }

    /// Optional seek-capable suffix read; implemented by seek-capable
    /// backends only.
    async fn load_stored_from(
        &self,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<Option<StoredSuffix>, String> {
        let _ = (id, from_seq);
        Err("seek-capable reads are not implemented by this backend".to_string())
    }

    /// Optional lifecycle teardown.
    async fn close(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Per-session write state held by the coordinator's in-memory bookkeeping.
#[derive(Clone)]
struct SessionState {
    meta: SessionHeader,
    /// The next seq the backend expects to append (the stored log length).
    cursor: u64,
    /// Whether lazy creation has produced a durable artifact.
    materialized: bool,
    /// The live Session this state was bound to, if any.
    owner: Option<Session>,
}

#[derive(Clone)]
struct RetirementEntry {
    token: Arc<()>,
    future: futures::future::Shared<BoxOpFuture<()>>,
}

/// One live session's initialization and bounded write-behind controller.
#[derive(Clone)]
struct LiveSessionState {
    session: Session,
    /// Shared initialization future (awaitable by many callers).
    init: Option<futures::future::Shared<BoxOpFuture<()>>>,
    writes: Arc<SessionWriteBehind>,
}

/// One validated cold source and the exact unpublished Session built from it.
struct PreparedSessionSource<TornMarker> {
    inspection: SessionInspection,
    session: Session,
    revision: SessionPersistenceRevision,
    /// Session length after constructor-owned seed markers were appended.
    session_length: usize,
    torn_marker: Option<TornMarker>,
    closers: Vec<SessionEvent>,
}

impl<TornMarker: Clone + Send + Sync + 'static> PreparedSource
    for PreparedSessionSource<TornMarker>
{
    fn session(&self) -> &Session {
        &self.session
    }
}

impl<TornMarker: Clone> Clone for PreparedSessionSource<TornMarker> {
    fn clone(&self) -> Self {
        Self {
            inspection: self.inspection.clone(),
            session: self.session.clone(),
            revision: self.revision.clone(),
            session_length: self.session_length,
            torn_marker: self.torn_marker.clone(),
            closers: self.closers.clone(),
        }
    }
}

/// Whether a live session seed reproduces a persisted prefix exactly.
fn seed_covers_prefix(seed: &[SessionEvent], prefix: &[SessionEvent]) -> bool {
    prefix.len() <= seed.len()
        && prefix
            .iter()
            .enumerate()
            .all(|(index, event)| seed.get(index) == Some(event))
}

/// Reject events from an obsolete v0 vocabulary that this build cannot
/// replay.
fn assert_supported_events(events: &[SessionEvent], id: &SessionId) -> Result<(), String> {
    if let Some(legacy) = events
        .iter()
        .find(|event| event.type_ == "request/header-delta")
    {
        return Err(format!(
            "session \"{}\" contains unsupported legacy request/header-delta event at seq {}",
            id.as_str(),
            legacy.seq
        ));
    }
    if let Some(legacy) = events.iter().find(|event| event.type_ == "mode/set") {
        return Err(format!(
            "session \"{}\" contains unsupported legacy mode/set event at seq {}",
            id.as_str(),
            legacy.seq
        ));
    }
    if let Some(fallback) = events.iter().find(|event| {
        event.type_ == "request/header"
            && event.data.get("reason").and_then(|value| value.as_str()) == Some("fallback")
    }) {
        return Err(format!(
            "session \"{}\" contains unsupported legacy request/header reason \"fallback\" at seq {}",
            id.as_str(),
            fallback.seq
        ));
    }
    Ok(())
}

fn as_record(value: &JsonValue) -> Option<&Map<String, JsonValue>> {
    value.as_object()
}

fn has_only_keys(record: &Map<String, JsonValue>, required: &[&str], optional: &[&str]) -> bool {
    record
        .keys()
        .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
        && required.iter().all(|key| record.contains_key(*key))
}

fn legacy_message_id(id: &SessionId, seq: u64) -> String {
    format!("legacy-message:{}:{seq}", id.as_str())
}

fn replacement_start(event: &SessionEvent) -> Option<u64> {
    match &event.surface_op {
        Some(dsh_session::SurfaceOp::Replace { start, .. }) => Some(*start),
        _ => None,
    }
}

/// Whether one suffix event needs facts available only from the preceding
/// stored prefix.
fn needs_legacy_prefix(event: &SessionEvent) -> bool {
    if event.type_ == "steering/message" {
        return true;
    }
    let Some(data) = as_record(&event.data) else {
        return false;
    };
    match event.type_.as_str() {
        "user/message" => !data.contains_key("id") && data.contains_key("content"),
        "assistant/message" => !data.contains_key("message") && data.contains_key("content"),
        "tool/result" => !data.contains_key("message") && data.contains_key("callId"),
        _ => false,
    }
}

/// Upgrade the removed steering surface event into its current user-message
/// equivalent.
fn migrate_legacy_steering_event(
    event: &SessionEvent,
    id: &SessionId,
) -> Result<SessionEvent, String> {
    if event.type_ != "steering/message" {
        return Ok(event.clone());
    }
    let malformed = || {
        format!(
            "session \"{}\" contains malformed pre-react-loop steering/message at seq {}",
            id.as_str(),
            event.seq
        )
    };
    let Some(data) = as_record(&event.data) else {
        return Err(malformed());
    };
    let turn_is_int = data.get("turn").and_then(|value| value.as_u64()).is_some();
    if let Some(wrapped) = data.get("message").and_then(|value| value.as_object())
        && turn_is_int
        && has_only_keys(data, &["turn", "message"], &[])
    {
        let mut migrated = event.clone();
        migrated.type_ = "user/message".to_string();
        migrated.data = JsonValue::Object(wrapped.clone());
        return Ok(migrated);
    }
    if !turn_is_int || !has_only_keys(data, &["turn", "content", "source"], &[]) {
        return Err(malformed());
    }
    let mut message = Map::new();
    for (key, value) in data {
        if key == "turn" {
            continue;
        }
        message.insert(key.clone(), value.clone());
    }
    message.insert(
        "id".to_string(),
        JsonValue::String(legacy_message_id(id, event.seq)),
    );
    message.insert("role".to_string(), JsonValue::String("user".to_string()));
    let mut migrated = event.clone();
    migrated.type_ = "user/message".to_string();
    migrated.data = JsonValue::Object(message);
    Ok(migrated)
}

/// Remove the obsolete trigger after verifying the complete old turn-start
/// envelope.
fn migrate_legacy_turn_start_event(
    event: &SessionEvent,
    id: &SessionId,
) -> Result<SessionEvent, String> {
    if event.type_ != "turn/start" {
        return Ok(event.clone());
    }
    let Some(data) = as_record(&event.data) else {
        return Ok(event.clone());
    };
    if !data.contains_key("trigger") {
        return Ok(event.clone());
    }
    let trigger = data.get("trigger").and_then(|value| value.as_object());
    let turn = data.get("turn").and_then(|value| value.as_u64());
    if turn.is_none()
        || turn.unwrap() < 1
        || !has_only_keys(data, &["turn", "trigger"], &[])
        || trigger.is_none()
        || trigger
            .and_then(|t| t.get("kind"))
            .and_then(|kind| kind.as_str())
            .is_none_or(|kind| kind.is_empty())
    {
        return Err(format!(
            "session \"{}\" contains malformed pre-react-loop turn/start at seq {}",
            id.as_str(),
            event.seq
        ));
    }
    let mut migrated = event.clone();
    migrated.data = serde_json::json!({ "turn": turn.unwrap() });
    Ok(migrated)
}

/// Upgrade an obsolete turn ending while preserving the latest-master
/// envelope.
fn migrate_legacy_turn_end_event(
    event: &SessionEvent,
    id: &SessionId,
) -> Result<SessionEvent, String> {
    if event.type_ != "turn/end" {
        return Ok(event.clone());
    }
    let Some(data) = as_record(&event.data) else {
        return Ok(event.clone());
    };
    let malformed = || {
        format!(
            "session \"{}\" contains malformed pre-react-loop turn/end at seq {}",
            id.as_str(),
            event.seq
        )
    };
    let Some(reason) = data.get("reason").and_then(|value| value.as_object()) else {
        return Err(malformed());
    };
    let turn = data.get("turn").and_then(|value| value.as_u64());
    if turn.is_none() || turn.unwrap() < 1 || !has_only_keys(data, &["turn", "reason"], &[]) {
        return Err(malformed());
    }
    let Some(kind) = reason.get("kind").and_then(|value| value.as_str()) else {
        return Err(malformed());
    };
    let current_reason: Option<JsonValue> = match kind {
        "completed" | "blocked" | "max-tokens" | "interrupted" => {
            if !has_only_keys(reason, &["kind"], &[]) {
                return Err(malformed());
            }
            None
        }
        "aborted" => {
            if reason.contains_key("reason") {
                None
            } else {
                if !has_only_keys(reason, &["kind"], &[]) {
                    return Err(malformed());
                }
                Some(serde_json::json!({"kind": "aborted", "reason": {"kind": "legacy"}}))
            }
        }
        "disposed" => {
            if !has_only_keys(reason, &["kind"], &[]) {
                return Err(malformed());
            }
            Some(serde_json::json!({"kind": "aborted", "reason": {"kind": "disposed"}}))
        }
        "error" => {
            if reason.contains_key("error") {
                None
            } else {
                let step = reason.get("step").and_then(|value| value.as_u64());
                if step.is_none() {
                    return Err(malformed());
                }
                if let Some(failure) = reason.get("failure").and_then(|value| value.as_object()) {
                    if has_only_keys(reason, &["kind", "step", "failure"], &[])
                        && has_only_keys(
                            failure,
                            &["message", "code"],
                            &["status", "providerRetryAfterMs", "requestId"],
                        )
                        && failure.get("message").is_some_and(|v| v.is_string())
                        && failure.get("code").is_some_and(|v| v.is_string())
                        && failure.get("status").is_none_or(|v| v.is_number())
                        && failure
                            .get("providerRetryAfterMs")
                            .is_none_or(|v| v.is_number())
                        && failure.get("requestId").is_none_or(|v| v.is_string())
                    {
                        Some(serde_json::json!({"kind": "error", "error": failure}))
                    } else {
                        return Err(malformed());
                    }
                } else {
                    let message_keys: &[&str] = if reason.contains_key("code") {
                        &["kind", "step", "message", "code"]
                    } else {
                        &["kind", "step", "message"]
                    };
                    if !has_only_keys(reason, message_keys, &[])
                        || !reason.get("message").is_some_and(|v| v.is_string())
                        || reason.get("code").is_some_and(|v| !v.is_string())
                    {
                        return Err(malformed());
                    }
                    let code = reason
                        .get("code")
                        .and_then(|v| v.as_str())
                        .unwrap_or("UNKNOWN");
                    Some(serde_json::json!({
                        "kind": "error",
                        "error": {
                            "message": reason.get("message").and_then(|v| v.as_str()).unwrap_or(""),
                            "code": code,
                        },
                    }))
                }
            }
        }
        _ => None,
    };
    match current_reason {
        None => Ok(event.clone()),
        Some(reason_value) => {
            let mut migrated = event.clone();
            let mut data_clone = data.clone();
            data_clone.insert("reason".to_string(), reason_value);
            migrated.data = JsonValue::Object(data_clone);
            Ok(migrated)
        }
    }
}

/// Upgrade one pre-identity message event into the current wrapper shape.
fn migrate_legacy_message_event(
    event: &SessionEvent,
    id: &SessionId,
    message_ids: &HashMap<u64, String>,
) -> SessionEvent {
    let mut migrated = event.clone();
    let Some(data) = as_record(&event.data) else {
        return migrated;
    };
    match event.type_.as_str() {
        "user/message" => {
            if data.contains_key("id")
                || data.contains_key("role")
                || data.contains_key("message")
                || !data.contains_key("content")
                || !data.contains_key("source")
            {
                return migrated;
            }
            let mut record = data.clone();
            record.insert(
                "id".to_string(),
                JsonValue::String(legacy_message_id(id, event.seq)),
            );
            record.insert("role".to_string(), JsonValue::String("user".to_string()));
            migrated.data = JsonValue::Object(record);
        }
        "assistant/message" => {
            if data.contains_key("message")
                || !data.contains_key("content")
                || !data.contains_key("provenance")
            {
                return migrated;
            }
            let content = data.get("content").cloned().unwrap_or(JsonValue::Null);
            let provenance = data.get("provenance").cloned().unwrap_or(JsonValue::Null);
            let mut event_data = Map::new();
            for (key, value) in data {
                if key != "content" && key != "provenance" {
                    event_data.insert(key.clone(), value.clone());
                }
            }
            let mut source = provenance.as_object().cloned().unwrap_or_default();
            source.insert("kind".to_string(), JsonValue::String("model".to_string()));
            event_data.insert(
                "message".to_string(),
                serde_json::json!({
                    "id": legacy_message_id(id, event.seq),
                    "role": "assistant",
                    "content": content,
                    "source": source,
                }),
            );
            migrated.data = JsonValue::Object(event_data);
        }
        "tool/result" => {
            if data.contains_key("message")
                || !data.contains_key("callId")
                || !data.contains_key("content")
                || !data.contains_key("isError")
            {
                return migrated;
            }
            let call_id = data.get("callId").cloned().unwrap_or(JsonValue::Null);
            let content = data.get("content").cloned().unwrap_or(JsonValue::Null);
            let is_error = data.get("isError").cloned().unwrap_or(JsonValue::Null);
            let inherited_id = replacement_start(event);
            let mut event_data = Map::new();
            for (key, value) in data {
                if key != "callId" && key != "content" && key != "isError" {
                    event_data.insert(key.clone(), value.clone());
                }
            }
            let message_id = match inherited_id {
                None => legacy_message_id(id, event.seq),
                Some(start) => message_ids
                    .get(&start)
                    .cloned()
                    .unwrap_or_else(|| legacy_message_id(id, event.seq)),
            };
            event_data.insert(
                "message".to_string(),
                serde_json::json!({
                    "id": message_id,
                    "role": "user",
                    "content": [{
                        "type": "tool-result",
                        "toolCallId": call_id,
                        "content": content,
                        "isError": is_error,
                    }],
                    "source": { "kind": "tool", "callId": call_id },
                }),
            );
            migrated.data = JsonValue::Object(event_data);
        }
        _ => {}
    }
    migrated
}

/// Read the identified message carried by one validated current event.
fn event_message_id(event: &SessionEvent) -> Option<String> {
    let data = as_record(&event.data);
    let message = match event.type_.as_str() {
        "user/message" => data,
        _ => data
            .and_then(|d| d.get("message"))
            .and_then(|v| v.as_object()),
    };
    message
        .and_then(|m| m.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Materialize stored events as upgraded, validated snapshots with immutable
/// messages (TS `snapshotStoredEvents`).
fn snapshot_stored_events(
    events: &[SessionEvent],
    id: &SessionId,
) -> Result<Vec<SessionEvent>, String> {
    assert_supported_events(events, id)?;
    let mut message_ids = HashMap::new();
    let mut out = Vec::with_capacity(events.len());
    for event in events {
        let migrated_start = migrate_legacy_turn_start_event(event, id)?;
        let migrated_turn = migrate_legacy_turn_end_event(&migrated_start, id)?;
        let migrated_steering = migrate_legacy_steering_event(&migrated_turn, id)?;
        let snapshot = migrate_legacy_message_event(&migrated_steering, id, &message_ids);
        if let Some(message_id) = event_message_id(&snapshot) {
            message_ids.insert(snapshot.seq, message_id);
        }
        out.push(snapshot);
    }
    Ok(out)
}

/// Owns the backend-agnostic session write-path orchestration.
pub struct PersistenceCoordinator<TornMarker: Clone + Send + Sync + 'static = ()> {
    ctx: Context,
    backend: Arc<dyn PersistenceBackend<TornMarker>>,
    states: Mutex<HashMap<String, SessionState>>,
    live: Mutex<HashMap<usize, LiveSessionState>>,
    retirements: Mutex<HashMap<String, RetirementEntry>>,
    preparations: SessionPreparations<PreparedSessionSource<TornMarker>, SessionState>,
    /// Per-id serialization: one async mutex per id.
    chains: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    write_batch_max_delay_ms: u64,
}

fn session_ptr(session: &Session) -> usize {
    session.identity()
}

impl<TornMarker: Clone + Send + Sync + 'static> PersistenceCoordinator<TornMarker> {
    pub fn new(
        ctx: &Context,
        backend: Arc<dyn PersistenceBackend<TornMarker>>,
        options: PersistenceCoordinatorOptions,
    ) -> Arc<Self> {
        assert!(
            options.prepared_session_cache_size >= 1,
            "preparedSessionCacheSize must be a positive safe integer"
        );
        assert!(
            options.write_batch_max_delay_ms >= 1
                && options.write_batch_max_delay_ms <= MAX_WRITE_BATCH_DELAY_MS,
            "writeBatchMaxDelayMs must be an integer between 1 and {MAX_WRITE_BATCH_DELAY_MS}"
        );
        let coordinator = Arc::new(Self {
            ctx: ctx.clone(),
            backend,
            states: Mutex::new(HashMap::new()),
            live: Mutex::new(HashMap::new()),
            retirements: Mutex::new(HashMap::new()),
            preparations: SessionPreparations::new(options.prepared_session_cache_size),
            chains: Mutex::new(HashMap::new()),
            write_batch_max_delay_ms: options.write_batch_max_delay_ms,
        });
        coordinator.install_write_path();
        coordinator
    }

    // ---- Public API (the backend's service methods delegate here) ----

    /// Register detached session metadata for lazy creation on the first
    /// append.
    pub async fn create(self: &Arc<Self>, meta: SessionHeader) -> Result<(), String> {
        let snapshot_value = serde_json::to_value(&meta)
            .map_err(|_| "session metadata must be losslessly JSON-serializable".to_string())?;
        let snapshot = snapshot_json_value(&snapshot_value)
            .ok_or_else(|| "session metadata must be losslessly JSON-serializable".to_string())?;
        let snapshot: SessionHeader = serde_json::from_value(snapshot)
            .map_err(|_| "session metadata must be losslessly JSON-serializable".to_string())?;
        let id = snapshot.id.clone();
        self.serialize(&id, self.create_core(snapshot)).await
    }

    fn create_core(self: &Arc<Self>, meta: SessionHeader) -> BoxOpFuture<()> {
        let coordinator = Arc::clone(self);
        Box::pin(async move {
            let id = meta.id.clone();
            if coordinator.states.lock().contains_key(id.as_str())
                || coordinator.preparations.has(&id)
            {
                return Err(format!(
                    "session \"{}\" already exists in this backend",
                    id.as_str()
                ));
            }
            if coordinator.backend.load_stored(&id).await?.is_some() {
                return Err(format!(
                    "session \"{}\" already has a persisted log on disk; load/resume it instead of creating",
                    id.as_str()
                ));
            }
            coordinator.states.lock().insert(
                id.as_str().to_string(),
                SessionState {
                    meta,
                    cursor: 0,
                    materialized: false,
                    owner: None,
                },
            );
            Ok(())
        })
    }

    /// Durably persist a batch of events.
    pub async fn append(
        self: &Arc<Self>,
        id: &SessionId,
        events: &[SessionEvent],
    ) -> Result<(), String> {
        let batch_value = serde_json::to_value(events).map_err(|_| {
            "session event batch is not losslessly JSON-serializable because it contains non-JSON-serializable data"
                .to_string()
        })?;
        let batch = snapshot_json_value(&batch_value).ok_or_else(|| {
            "session event batch is not losslessly JSON-serializable because it contains non-JSON-serializable data"
                .to_string()
        })?;
        let batch: Vec<SessionEvent> = serde_json::from_value(batch).map_err(|_| {
            "session event batch is not losslessly JSON-serializable because it contains non-JSON-serializable data"
                .to_string()
        })?;
        let id_owned = id.clone();
        self.serialize(id, self.append_core(id_owned, batch)).await
    }

    /// Permanently delete one detached session after its retirement drained.
    pub async fn delete(self: &Arc<Self>, id: &SessionId) -> Result<bool, String> {
        if self.sessions()?.get(id).is_some() {
            return Err(format!(
                "cannot delete session \"{}\" while it is live",
                id.as_str()
            ));
        }
        // `session/disposed` is an emitted event. Permanent deletion may race
        // ahead of its listener, so synchronously retire any persistence-owned
        // live controller before deleting the backend artifact. Otherwise a
        // late write-behind drain can recreate the file after delete returns.
        let owner = self
            .live
            .lock()
            .values()
            .find(|live| live.session.id() == id)
            .map(|live| live.session.clone())
            .or_else(|| {
                self.states
                    .lock()
                    .get(id.as_str())
                    .and_then(|state| state.owner.clone())
            });
        if let Some(owner) = owner {
            self.retire_core(&owner).await?;
        }
        self.wait_for_retirement(id).await?;

        let coordinator = Arc::clone(self);
        let id_owned = id.clone();
        self.serialize(
            id,
            Box::pin(async move {
                coordinator.preparations.assert_writable(&id_owned)?;
                let deleted = coordinator.backend.delete_stored(&id_owned).await?;
                coordinator.states.lock().remove(id_owned.as_str());
                coordinator.preparations.invalidate(&id_owned);
                Ok(deleted)
            }),
        )
        .await
    }

    fn append_core(self: &Arc<Self>, id: SessionId, events: Vec<SessionEvent>) -> BoxOpFuture<()> {
        let coordinator = Arc::clone(self);
        Box::pin(async move {
            assert_supported_events(&events, &id)?;
            if events.is_empty() {
                return Ok(());
            }
            coordinator.preparations.assert_writable(&id)?;
            let existing = { coordinator.states.lock().get(id.as_str()).cloned() };
            let mut state = match existing {
                Some(state) => state,
                None => coordinator.adopt(&id).await?,
            };
            for (index, event) in events.iter().enumerate() {
                let expected = state.cursor + index as u64;
                if event.seq != expected {
                    return Err(format!(
                        "append seq mismatch for \"{}\": expected {expected} at index {index}, got {}",
                        id.as_str(),
                        event.seq
                    ));
                }
            }
            coordinator
                .backend
                .append_batch(&state.meta, &events, state.materialized)
                .await?;
            state.materialized = true;
            state.cursor += events.len() as u64;
            coordinator
                .states
                .lock()
                .insert(id.as_str().to_string(), state);
            coordinator.preparations.invalidate(&id);
            Ok(())
        })
    }

    /// Prepare and reserve the exact unpublished Session used by resume.
    pub async fn prepare(
        self: &Arc<Self>,
        id: &SessionId,
    ) -> Result<dsh_session::SessionPreparation, String> {
        loop {
            self.wait_for_retirement(id).await?;
            let sessions = self.sessions()?;
            if sessions.get(id).is_some() {
                return Err(format!(
                    "cannot prepare session \"{}\" while it is live",
                    id.as_str()
                ));
            }
            let reservation = self
                .preparations
                .reserve(id, self.prepare_loader(id.clone()), self.commit_loader())
                .await?;
            let Some(reservation) = reservation else {
                continue;
            };
            if sessions.get(id).is_some() {
                self.preparations.release(&reservation, false);
                return Err(format!(
                    "cannot prepare session \"{}\" while it is live",
                    id.as_str()
                ));
            }
            let source = reservation.source.clone();
            let session = source.session.clone();
            let reusable = reservation.state.owner.is_none()
                && session.events().len() == source.session_length;
            let coordinator = Arc::clone(self);
            let reservation = reservation.clone();
            return Ok(dsh_session::SessionPreparation::create(
                session,
                dsh_session::SessionPreparationOptions {
                    release: Some(Box::new(move || {
                        coordinator.preparations.release(&reservation, reusable);
                    })),
                },
            ));
        }
    }

    /// Commit recovery and return its immutable logical view without
    /// publication.
    pub async fn load(self: &Arc<Self>, id: &SessionId) -> Result<SessionInspection, String> {
        loop {
            self.wait_for_retirement(id).await?;
            let sessions = self.sessions()?;
            if let Some(live) = sessions.get(id) {
                return self.load_live_snapshot(&live).await;
            }
            let reservation = self
                .preparations
                .reserve(id, self.prepare_loader(id.clone()), self.commit_loader())
                .await?;
            let Some(reservation) = reservation else {
                continue;
            };
            if let Some(attached) = sessions.get(id) {
                self.preparations.discard(&reservation);
                return self.load_live_snapshot(&attached).await;
            }
            let inspection = reservation.source.inspection.clone();
            self.preparations.discard(&reservation);
            return Ok(inspection);
        }
    }

    /// Inspect a logical session without publishing it or committing
    /// recovery.
    pub async fn inspect(self: &Arc<Self>, id: &SessionId) -> Result<SessionInspection, String> {
        loop {
            if self.retirements.lock().contains_key(id.as_str()) {
                self.wait_for_retirement(id).await?;
            }
            let sessions = self.sessions()?;
            if let Some(live) = sessions.get(id) {
                return Ok(self.inspect_live(&live));
            }
            match self
                .preparations
                .inspect(id, self.prepare_loader(id.clone()))
                .await
            {
                Ok(source) => {
                    if let Some(attached) = sessions.get(id) {
                        return Ok(self.inspect_live(&attached));
                    }
                    let current = self
                        .serialize(id, self.is_prepared_source_current(source.clone()))
                        .await?;
                    if let Some(published) = sessions.get(id) {
                        return Ok(self.inspect_live(&published));
                    }
                    if current {
                        return Ok(source.inspection.clone());
                    }
                    if matches!(
                        self.preparations.discard_ready(id, &source),
                        crate::preparations::DiscardOutcome::Retained
                    ) {
                        return Ok(source.inspection.clone());
                    }
                }
                Err(error) => {
                    if let Some(attached) = sessions.get(id) {
                        return Ok(self.inspect_live(&attached));
                    }
                    return Err(error);
                }
            }
        }
    }

    /// Read the stored events from `fromSeq` onward, detached and
    /// non-mutating.
    pub async fn read_from(
        self: &Arc<Self>,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<SessionReadFromResult, String> {
        if self.retirements.lock().contains_key(id.as_str()) {
            self.wait_for_retirement(id).await?;
        }
        let id_owned = id.clone();
        self.serialize(&id_owned, self.read_from_core(id_owned.clone(), from_seq))
            .await
    }

    fn read_from_core(
        self: &Arc<Self>,
        id: SessionId,
        from_seq: u64,
    ) -> BoxOpFuture<SessionReadFromResult> {
        let coordinator = Arc::clone(self);
        Box::pin(async move {
            if coordinator.backend.seek_capable() {
                let suffix = coordinator
                    .backend
                    .load_stored_from(&id, from_seq)
                    .await?
                    .ok_or_else(|| format!("session \"{}\" not found", id.as_str()))?;
                coordinator.assert_stored_id(&id, &suffix.meta)?;
                coordinator.assert_version(&suffix.meta)?;
                if suffix.events.iter().any(needs_legacy_prefix) {
                    let (meta, events) = coordinator.read_stored_prefix(&id).await?;
                    return Ok(SessionReadFromResult {
                        meta,
                        events: events
                            .into_iter()
                            .filter(|event| event.seq >= from_seq)
                            .collect(),
                    });
                }
                let events = snapshot_stored_events(&suffix.events, &id)?;
                coordinator.assert_events_supported(&suffix.meta, &events)?;
                return Ok(SessionReadFromResult {
                    meta: suffix.meta,
                    events,
                });
            }
            let (meta, events) = coordinator.read_stored_prefix(&id).await?;
            Ok(SessionReadFromResult {
                meta,
                events: events
                    .into_iter()
                    .filter(|event| event.seq >= from_seq)
                    .collect(),
            })
        })
    }

    /// Read one detached physical prefix without logical recovery or caching.
    async fn read_stored_prefix(
        &self,
        id: &SessionId,
    ) -> Result<(SessionHeader, Vec<SessionEvent>), String> {
        let stored = self
            .backend
            .load_stored(id)
            .await?
            .ok_or_else(|| format!("session \"{}\" not found", id.as_str()))?;
        self.assert_stored_id(id, &stored.meta)?;
        self.assert_version(&stored.meta)?;
        let events = snapshot_stored_events(&stored.events, id)?;
        self.assert_events_supported(&stored.meta, &events)?;
        Ok((stored.meta, events))
    }

    /// Read, repair in memory, validate, and freeze one cold source once
    /// (TS `prepareCore`).
    async fn prepare_core(
        &self,
        id: &SessionId,
    ) -> Result<Arc<PreparedSessionSource<TornMarker>>, String> {
        let stored = self
            .backend
            .load_stored(id)
            .await?
            .ok_or_else(|| format!("session \"{}\" not found", id.as_str()))?;
        let meta = stored.meta.clone();
        self.assert_stored_id(id, &meta)?;
        self.assert_version(&meta)?;
        let stored_events = snapshot_stored_events(&stored.events, id)?;
        self.assert_events_supported(&meta, &stored_events)?;

        // Preserve complete interrupted events and synthesize only missing
        // closers.
        let closers = interrupted_turn_closers(&stored_events);
        let mut balanced = stored_events.clone();
        balanced.extend(closers.clone());
        let sessions = self.sessions()?;
        let session = sessions.prepare(
            Some(id.clone()),
            Some(dsh_session::CreateSessionOptions {
                seed: Some(balanced.clone()),
                meta: Some(dsh_session::CreateSessionMeta {
                    cwd: meta.cwd.clone(),
                    parent_session: meta.parent_session.clone(),
                    created_at: Some(meta.created_at),
                    seed_length: meta.seed_length,
                    origin: meta.origin.clone(),
                    delegation_depth: meta.delegation_depth,
                    agent_preset: meta.agent_preset.clone(),
                }),
            }),
        )?;
        let inspection = SessionInspection {
            meta: session.header().clone(),
            events: balanced,
        };
        Ok(Arc::new(PreparedSessionSource {
            session_length: session.events().len(),
            session,
            inspection,
            revision: stored.revision,
            torn_marker: stored.torn_marker,
            closers,
        }))
    }

    /// Commit one prepared repair and establish its ownerless durable cursor
    /// (TS `commitPrepared`).
    async fn commit_prepared(
        self: &Arc<Self>,
        source: Arc<PreparedSessionSource<TornMarker>>,
    ) -> Result<Option<(Arc<PreparedSessionSource<TornMarker>>, SessionState)>, String> {
        let id = source.inspection.meta.id.clone();
        let cursor = source.inspection.events.len() as u64;
        if self
            .states
            .lock()
            .get(id.as_str())
            .is_some_and(|state| state.owner.is_some())
        {
            return Err(format!(
                "session \"{}\" already has a live persistence owner",
                id.as_str()
            ));
        }
        if !self.is_prepared_source_current(source.clone()).await? {
            return Ok(None);
        }
        if source.torn_marker.is_some() || !source.closers.is_empty() {
            self.backend
                .commit_repair(
                    &source.inspection.meta,
                    source.torn_marker.clone(),
                    &source.closers,
                )
                .await?;
            // The repair changed the durable revision: reload instead of
            // associating the old in-memory view.
            return Ok(None);
        }
        let existing = self.states.lock().get(id.as_str()).cloned();
        let state = existing.unwrap_or(SessionState {
            meta: source.inspection.meta.clone(),
            cursor,
            materialized: true,
            owner: None,
        });
        let mut state = state;
        state.meta = source.inspection.meta.clone();
        state.cursor = cursor;
        state.materialized = true;
        self.states
            .lock()
            .insert(id.as_str().to_string(), state.clone());
        Ok(Some((source, state)))
    }

    fn is_prepared_source_current(
        self: &Arc<Self>,
        source: Arc<PreparedSessionSource<TornMarker>>,
    ) -> BoxOpFuture<bool> {
        let coordinator = Arc::clone(self);
        Box::pin(async move {
            Ok(coordinator
                .backend
                .read_stored_revision(&source.inspection.meta.id)
                .await?
                == Some(source.revision.clone()))
        })
    }

    /// Return one durable immutable view of an already-live Session.
    async fn load_live_snapshot(
        self: &Arc<Self>,
        session: &Session,
    ) -> Result<SessionInspection, String> {
        let events = session.events();
        self.flush(session).await?;
        let state = self
            .states
            .lock()
            .get(session.id().as_str())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "session \"{}\" lost persistence state during load",
                    session.id().as_str()
                )
            })?;
        if events.is_empty() {
            return Err(format!("session \"{}\" not found", session.id().as_str()));
        }
        if !interrupted_turn_closers(&events).is_empty() {
            return Err(format!(
                "cannot load session \"{}\" while its live turn is open; use the live Session or wait for the turn to close",
                session.id().as_str()
            ));
        }
        Ok(SessionInspection {
            meta: state.meta,
            events: events.as_ref().clone(),
        })
    }

    /// Borrow one immutable view from an already-live Session.
    fn inspect_live(&self, session: &Session) -> SessionInspection {
        SessionInspection {
            meta: session.header().clone(),
            events: session.events().as_ref().clone(),
        }
    }

    /// Await one retiring lifecycle.
    async fn wait_for_retirement(&self, id: &SessionId) -> Result<(), String> {
        let retirement = self
            .retirements
            .lock()
            .get(id.as_str())
            .map(|entry| entry.future.clone());
        if let Some(retirement) = retirement {
            retirement.await?;
        }
        Ok(())
    }

    // ---- per-id serialization + adoption helpers ----

    /// Run `op` after any in-flight operation for the same session id.
    async fn serialize<T: Send + 'static>(
        &self,
        id: &SessionId,
        op: BoxOpFuture<T>,
    ) -> Result<T, String> {
        let lock = {
            let mut chains = self.chains.lock();
            chains
                .entry(id.as_str().to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        op.await
    }

    /// Build a state for a session discovered in storage but not yet in
    /// memory.
    async fn adopt(self: &Arc<Self>, id: &SessionId) -> Result<SessionState, String> {
        loop {
            let source = match self.preparations.take_ready(id) {
                Some(source) => source,
                None => self.prepare_core(id).await?,
            };
            if let Some((_, state)) = self.commit_prepared(source).await? {
                return Ok(state);
            }
        }
    }

    fn assert_version(&self, meta: &SessionHeader) -> Result<(), String> {
        if meta.version == SESSION_FORMAT_VERSION {
            return Ok(());
        }
        Err(self
            .unsupported(meta, session_format_version_refusal(&meta.id, meta.version))
            .message)
    }

    /// Refuse a log containing an event type this build does not know,
    /// unless the writer marked the event ignorable.
    fn assert_events_supported(
        &self,
        meta: &SessionHeader,
        events: &[SessionEvent],
    ) -> Result<(), String> {
        for event in events {
            if dsh_session::is_known_session_event_type(&event.type_)
                || event.ignorable == Some(true)
            {
                continue;
            }
            return Err(self
                .unsupported(
                    meta,
                    format!(
                        "session \"{}\" contains event type \"{}\" (seq {}) unknown to this harness and not marked ignorable; refusing to interpret the log — it was likely written by a newer harness",
                        meta.id.as_str(),
                        event.type_,
                        event.seq
                    ),
                )
                .message);
        }
        Ok(())
    }

    /// Build a format refusal that points at the raw artifact when the
    /// backend has one.
    fn unsupported(&self, meta: &SessionHeader, reason: String) -> SessionFormatUnsupportedError {
        let location = self.backend.locate(meta);
        SessionFormatUnsupportedError::new(
            match &location {
                None => reason,
                Some(location) => format!("{reason} (raw log: {})", location.path),
            },
            location,
        )
    }

    /// Reject backend metadata that is not bound to the requested session
    /// id.
    fn assert_stored_id(&self, id: &SessionId, meta: &SessionHeader) -> Result<(), String> {
        if meta.id != *id {
            return Err(format!(
                "stored session identity mismatch: requested \"{}\", header contains \"{}\"",
                id.as_str(),
                meta.id.as_str()
            ));
        }
        Ok(())
    }

    // ---- write path (session/event → flush drain) ----

    fn install_write_path(self: &Arc<Self>) {
        let ctx = self.ctx.clone();
        let coordinator = Arc::clone(self);

        // Dispose effect: flush every live session, then close the backend.
        let backend_name = coordinator.backend.name();
        let coordinator_for_dispose = Arc::clone(&coordinator);
        let _ = ctx.effect(
            &format!("{backend_name} write path"),
            Box::pin(async move {
                Some(make_disposer(move || {
                    let coordinator = Arc::clone(&coordinator_for_dispose);
                    let backend_name = backend_name;
                    Box::pin(async move {
                        let mut errors = Vec::new();
                        for session in coordinator.live_sessions_snapshot() {
                            if let Err(error) = coordinator.flush(&session).await {
                                errors.push(error);
                            }
                        }
                        if !errors.is_empty() {
                            coordinator
                                .ctx
                                .named_logger(Some(backend_name))
                                .error(vec![arc(format!("dispose failed: {}", errors.join("; ")))]);
                        }
                        let _ = coordinator.backend.close().await;
                    })
                }))
            }),
        );

        // session/created: capture the header on creation and persist a
        // fork's seed once.
        let created_coordinator = Arc::clone(&coordinator);
        let created_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let coordinator = Arc::clone(&created_coordinator);
            Box::pin(async move {
                let live = coordinator.init_for(&session);
                if let Some(init) = live.init
                    && let Err(error) = init.await
                {
                    panic!("{error}");
                }
                None
            })
        });
        let _ = ctx.events.register(
            &ctx,
            "session-persistence: session/created",
            "session/created",
            created_listener,
            &EventOptions::default().global(true),
        );

        // session/event: keep a persistence-owned copy of each frozen event.
        let event_coordinator = Arc::clone(&coordinator);
        let event_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let event = downcast::<SessionEvent>(&args[1])
                .expect("event arg")
                .clone();
            let coordinator = Arc::clone(&event_coordinator);
            Box::pin(async move {
                let live = coordinator.init_for(&session);
                live.writes.enqueue(event);
                None
            })
        });
        let _ = ctx.events.register(
            &ctx,
            "session-persistence: session/event",
            "session/event",
            event_listener,
            &EventOptions::default().global(true),
        );

        // session/flush: the immediate durability barrier for buffered
        // writes.
        let flush_coordinator = Arc::clone(&coordinator);
        let flush_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let coordinator = Arc::clone(&flush_coordinator);
            Box::pin(async move {
                if let Err(error) = coordinator.flush(&session).await {
                    panic!("{error}");
                }
                None
            })
        });
        let _ = ctx.events.register(
            &ctx,
            "session-persistence: session/flush",
            "session/flush",
            flush_listener,
            &EventOptions::default().global(true),
        );

        // session/disposed: retirement contains its own failure.
        let disposed_coordinator = Arc::clone(&coordinator);
        let disposed_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let coordinator = Arc::clone(&disposed_coordinator);
            Box::pin(async move {
                coordinator.retire(&session);
                None
            })
        });
        let _ = ctx.events.register(
            &ctx,
            "session-persistence: session/disposed",
            "session/disposed",
            disposed_listener,
            &EventOptions::default().global(true),
        );

        // HMR: seed existing live sessions.
        if let Ok(sessions) = coordinator.sessions() {
            for session in sessions.list() {
                coordinator.init_for(&session);
            }
        }
    }

    fn sessions(&self) -> Result<Arc<Arc<SessionStore>>, String> {
        self.ctx
            .get_typed("sessions", false)
            .ok_or_else(|| "cannot prepare a session: SessionStore is not configured".to_string())
    }

    fn live_sessions_snapshot(&self) -> Vec<Session> {
        self.states
            .lock()
            .values()
            .filter_map(|state| state.owner.clone())
            .collect()
    }

    /// Start and observe one disposed session's final drain.
    fn retire(self: &Arc<Self>, session: &Session) {
        if !self.live.lock().contains_key(&session_ptr(session)) {
            return;
        }
        let coordinator = Arc::clone(self);
        let session = session.clone();
        let id = session.id().clone();
        let retirement: BoxOpFuture<()> =
            Box::pin(async move { coordinator.retire_core(&session).await });
        let shared = retirement.shared();
        let token = Arc::new(());
        self.retirements.lock().insert(
            id.as_str().to_string(),
            RetirementEntry {
                token: Arc::clone(&token),
                future: shared.clone(),
            },
        );
        let coordinator = Arc::clone(self);
        tokio::spawn(async move {
            let result = shared.await;
            let mut retirements = coordinator.retirements.lock();
            if retirements
                .get(id.as_str())
                .is_some_and(|entry| Arc::ptr_eq(&entry.token, &token))
            {
                retirements.remove(id.as_str());
            }
            drop(retirements);
            if let Err(error) = result {
                coordinator
                    .ctx
                    .named_logger(Some(coordinator.backend.name()))
                    .warn(vec![arc(format!(
                        "session \"{}\" retirement failed: {error}",
                        id.as_str()
                    ))]);
            }
        });
    }

    /// Drain and release state owned by one exact disposed Session lifecycle.
    async fn retire_core(self: &Arc<Self>, session: &Session) -> Result<(), String> {
        self.flush(session).await?;
        let id = session.id().clone();
        let coordinator = Arc::clone(self);
        let session = session.clone();
        let id_for_block = id.clone();
        self.serialize(
            &id,
            Box::pin(async move {
                coordinator.live.lock().remove(&session_ptr(&session));
                let remove_state = coordinator
                    .states
                    .lock()
                    .get(id_for_block.as_str())
                    .is_some_and(|state| {
                        state
                            .owner
                            .as_ref()
                            .is_some_and(|owner| owner.ptr_eq(&session))
                    });
                if remove_state {
                    coordinator.states.lock().remove(id_for_block.as_str());
                }
                Ok(())
            }),
        )
        .await
    }

    /// Return the one lifecycle controller for a live session, creating it
    /// if needed.
    fn init_for(self: &Arc<Self>, session: &Session) -> LiveSessionState {
        // Close the check/create/publish window under one lifecycle lock. The
        // session/created and first session/event listeners may run in
        // parallel; publishing two controllers would let both materialize the
        // same initial durable log.
        let ptr = session_ptr(session);
        let mut live_states = self.live.lock();
        if let Some(existing) = live_states.get(&ptr).cloned() {
            return existing;
        }
        let live = match self.preparations.reservation_for(session).unwrap_or(None) {
            Some(reservation) => self.attach_prepared(session, &reservation),
            None => {
                let seed = session.events().as_ref().clone();
                let coordinator = Arc::clone(self);
                let session_for_init = session.clone();
                let init: BoxOpFuture<()> =
                    Box::pin(async move { coordinator.on_created(&session_for_init, &seed).await });
                let init = init.shared();
                let writes = self.create_write_behind(session, Some(init.clone()));
                LiveSessionState {
                    session: session.clone(),
                    init: Some(init),
                    writes,
                }
            }
        };
        live_states.insert(ptr, live.clone());
        live
    }

    /// Bind one exact prepared Session and persist only its unpublished
    /// suffix.
    fn attach_prepared(
        self: &Arc<Self>,
        session: &Session,
        reservation: &Arc<
            SessionPreparationReservation<PreparedSessionSource<TornMarker>, SessionState>,
        >,
    ) -> LiveSessionState {
        let source = reservation.source.clone();
        let state = reservation.state.clone();
        if !source.session.ptr_eq(session)
            || state.owner.is_some()
            || state.cursor != source.inspection.events.len() as u64
            || session.first_live_seq() as u64 != state.cursor
        {
            panic!(
                "session \"{}\" preparation no longer matches its persistence state",
                session.id().as_str()
            );
        }
        let suffix: Vec<SessionEvent> = session
            .events()
            .iter()
            .skip(state.cursor as usize)
            .cloned()
            .collect();
        self.preparations
            .attach(reservation)
            .unwrap_or_else(|error| panic!("{error}"));
        {
            let mut states = self.states.lock();
            if let Some(state) = states.get_mut(session.id().as_str()) {
                state.owner = Some(session.clone());
            }
        }
        let init = if suffix.is_empty() {
            None
        } else {
            let coordinator = Arc::clone(self);
            let id = session.id().clone();
            let init: BoxOpFuture<()> =
                Box::pin(async move { coordinator.append_core(id, suffix).await });
            Some(init.shared())
        };
        let writes = self.create_write_behind(session, init.clone());
        LiveSessionState {
            session: session.clone(),
            init,
            writes,
        }
    }

    /// Whether a live session's `seed` reproduces the first `cursor`
    /// persisted events.
    async fn seed_matches_persisted(
        self: &Arc<Self>,
        id: &SessionId,
        seed: &[SessionEvent],
        cursor: u64,
    ) -> Result<bool, String> {
        if cursor == 0 {
            return Ok(true);
        }
        let Some(stored) = self.backend.load_stored(id).await? else {
            return Ok(false);
        };
        self.assert_stored_id(id, &stored.meta)?;
        let stored_events = snapshot_stored_events(&stored.events, id)?;
        let prefix: Vec<SessionEvent> = stored_events.into_iter().take(cursor as usize).collect();
        Ok(seed_covers_prefix(seed, &prefix))
    }

    /// On session/created: sync the backend's in-memory state to a live
    /// Session (TS `onCreated`).
    async fn on_created(
        self: &Arc<Self>,
        session: &Session,
        seed: &[SessionEvent],
    ) -> Result<(), String> {
        let id = session.id().clone();
        let tracked = self.states.lock().get(id.as_str()).cloned();
        if let Some(tracked) = tracked {
            if tracked
                .owner
                .as_ref()
                .is_some_and(|owner| owner.ptr_eq(session))
            {
                return Ok(());
            }
            if tracked.owner.is_none() {
                // Ownerless state from the public create()/load() API.
                if tracked.meta.cwd != session.header().cwd {
                    return Err(format!(
                        "session \"{}\" is already persisted at a different cwd (persisted: {}, live: {}) (id collision)",
                        id.as_str(),
                        tracked
                            .meta
                            .cwd
                            .clone()
                            .unwrap_or_else(|| "undefined".to_string()),
                        session
                            .header()
                            .cwd
                            .clone()
                            .unwrap_or_else(|| "undefined".to_string())
                    ));
                }
                if !self
                    .seed_matches_persisted(&id, seed, tracked.cursor)
                    .await?
                {
                    return Err(format!(
                        "session \"{}\" is already persisted with {} event(s) that do not match this live session (id collision)",
                        id.as_str(),
                        tracked.cursor
                    ));
                }
                {
                    let mut states = self.states.lock();
                    if let Some(state) = states.get_mut(id.as_str()) {
                        state.owner = Some(session.clone());
                    }
                }
                let suffix: Vec<SessionEvent> =
                    seed.iter().skip(tracked.cursor as usize).cloned().collect();
                if !suffix.is_empty() {
                    self.append_core(id.clone(), suffix).await?;
                }
                return Ok(());
            }
            let has_work = self.live.lock().values().any(|live| live.writes.has_work());
            if !tracked.materialized && !has_work {
                self.states.lock().remove(id.as_str());
            } else {
                return Err(format!(
                    "session \"{}\" is already bound to a different live session in this backend (id collision)",
                    id.as_str()
                ));
            }
        }

        let stored = self.backend.load_stored(&id).await?;
        if let Some(stored) = stored {
            self.adopt_live_prefix(session, seed, stored).await?;
            return Ok(());
        }

        // Case 4: a genuinely new session. Register meta (lazy), then persist
        // its seed once.
        let meta = session.header().clone();
        self.create_core(meta).await?;
        {
            let mut states = self.states.lock();
            if let Some(created) = states.get_mut(id.as_str()) {
                created.owner = Some(session.clone());
            }
        }
        if !seed.is_empty() {
            self.append_core(id, seed.to_vec()).await?;
        }
        Ok(())
    }

    /// Adopt a stored prefix as a live session's history (HMR/reload).
    async fn adopt_live_prefix(
        self: &Arc<Self>,
        session: &Session,
        seed: &[SessionEvent],
        stored: StoredPrefix<TornMarker>,
    ) -> Result<(), String> {
        let meta = stored.meta.clone();
        self.assert_stored_id(session.id(), &meta)?;
        if meta.cwd != session.header().cwd {
            return Err(format!(
                "session \"{}\" is already persisted at a different cwd (persisted: {}, live: {}) (id collision)",
                session.id().as_str(),
                meta.cwd.clone().unwrap_or_else(|| "undefined".to_string()),
                session
                    .header()
                    .cwd
                    .clone()
                    .unwrap_or_else(|| "undefined".to_string())
            ));
        }
        self.assert_version(&meta)?;
        let stored_events = snapshot_stored_events(&stored.events, session.id())?;
        self.assert_events_supported(&meta, &stored_events)?;
        if !seed_covers_prefix(seed, &stored_events) {
            return Err(format!(
                "session \"{}\" already has a persisted log on disk that does not match this live session (id collision)",
                session.id().as_str()
            ));
        }
        // Truncate-only repair (no closers): the open turn is NOT closed here.
        if stored.torn_marker.is_some() {
            self.backend
                .commit_repair(&meta, stored.torn_marker, &[])
                .await?;
        }
        let cursor = stored_events.len() as u64;
        self.states.lock().insert(
            session.id().as_str().to_string(),
            SessionState {
                meta,
                cursor,
                materialized: true,
                owner: Some(session.clone()),
            },
        );
        let suffix: Vec<SessionEvent> = seed.iter().skip(cursor as usize).cloned().collect();
        if !suffix.is_empty() {
            self.append_core(session.id().clone(), suffix).await?;
        }
        Ok(())
    }

    async fn flush(self: &Arc<Self>, session: &Session) -> Result<(), String> {
        let live = self.init_for(session);
        live.writes.cancel_automatic_wait();
        if let Some(init) = &live.init {
            init.clone().await?;
        }
        live.writes.flush().await
    }

    /// Build one package-private write controller around initialization and
    /// id serialization.
    fn create_write_behind(
        self: &Arc<Self>,
        session: &Session,
        init: Option<futures::future::Shared<BoxOpFuture<()>>>,
    ) -> Arc<SessionWriteBehind> {
        let coordinator = Arc::clone(self);
        let coordinator_for_write = Arc::clone(self);
        let coordinator_for_report = Arc::clone(self);
        let live_session = session.clone();
        SessionWriteBehind::new(crate::write_behind::SessionWriteBehindOptions {
            max_delay_ms: coordinator.write_batch_max_delay_ms,
            write: Arc::new(move |batch| {
                let coordinator = Arc::clone(&coordinator_for_write);
                let id = live_session.id().clone();
                let init = init.clone();
                Box::pin(async move {
                    if let Some(init) = init {
                        init.await?;
                    }
                    coordinator
                        .serialize(&id, coordinator.append_live_batch(id.clone(), batch))
                        .await
                })
            }),
            report_background_failure: Arc::new({
                let session = session.clone();
                move |error| {
                    coordinator_for_report.ctx.named_logger(Some(coordinator_for_report.backend.name())).warn(vec![arc(format!(
                        "background write for session \"{}\" failed (buffered events retained): {error}",
                        session.id().as_str()
                    ))]);
                }
            }),
        })
    }

    fn append_live_batch(
        self: &Arc<Self>,
        id: SessionId,
        batch: Vec<SessionEvent>,
    ) -> BoxOpFuture<()> {
        let coordinator = Arc::clone(self);
        Box::pin(async move {
            let cursor = coordinator
                .states
                .lock()
                .get(id.as_str())
                .map(|state| state.cursor)
                .unwrap_or(0);
            let fresh: Vec<SessionEvent> = batch
                .into_iter()
                .filter(|event| event.seq >= cursor)
                .collect();
            coordinator.append_core(id, fresh).await
        })
    }

    fn prepare_loader(
        self: &Arc<Self>,
        id: SessionId,
    ) -> PreparedSourceLoader<PreparedSessionSource<TornMarker>> {
        let coordinator = Arc::clone(self);
        Arc::new(move || {
            let coordinator = Arc::clone(&coordinator);
            let id = id.clone();
            Box::pin(async move { coordinator.prepare_core(&id).await })
        })
    }

    fn commit_loader(
        self: &Arc<Self>,
    ) -> Arc<
        dyn Fn(
                Arc<PreparedSessionSource<TornMarker>>,
            )
                -> BoxOpFuture<Option<(Arc<PreparedSessionSource<TornMarker>>, SessionState)>>
            + Send
            + Sync,
    > {
        let coordinator = Arc::clone(self);
        Arc::new(move |source| {
            let coordinator = Arc::clone(&coordinator);
            Box::pin(async move { coordinator.commit_prepared(source).await })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::revision::session_persistence_revision;
    use dsh_session::{CreateSessionMeta, SessionStore, SurfaceOp, session_id};

    /// An in-memory backend for coordinator tests.
    struct MemoryBackend {
        name: &'static str,
        sessions: parking_lot::Mutex<HashMap<String, (SessionHeader, Vec<SessionEvent>, u64, u64)>>,
    }

    impl MemoryBackend {
        fn new(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                sessions: parking_lot::Mutex::new(HashMap::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl PersistenceBackend<()> for MemoryBackend {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn load_stored(&self, id: &SessionId) -> Result<Option<StoredPrefix<()>>, String> {
            let sessions = self.sessions.lock();
            Ok(sessions
                .get(id.as_str())
                .map(|(meta, events, revision, _)| StoredPrefix {
                    meta: meta.clone(),
                    events: events.clone(),
                    revision: session_persistence_revision(format!("rev-{revision}")),
                    torn_marker: None,
                }))
        }

        async fn read_stored_revision(
            &self,
            id: &SessionId,
        ) -> Result<Option<SessionPersistenceRevision>, String> {
            let sessions = self.sessions.lock();
            Ok(sessions
                .get(id.as_str())
                .map(|(_, _, revision, _)| session_persistence_revision(format!("rev-{revision}"))))
        }

        async fn append_batch(
            &self,
            meta: &SessionHeader,
            events: &[SessionEvent],
            _is_materialized: bool,
        ) -> Result<(), String> {
            let mut sessions = self.sessions.lock();
            let entry = sessions
                .entry(meta.id.as_str().to_string())
                .or_insert_with(|| (meta.clone(), Vec::new(), 0, 0));
            entry.0 = meta.clone();
            entry.1.extend(events.iter().cloned());
            entry.2 += 1;
            entry.3 += events.len() as u64;
            Ok(())
        }

        async fn commit_repair(
            &self,
            meta: &SessionHeader,
            _torn_marker: Option<()>,
            closers: &[SessionEvent],
        ) -> Result<(), String> {
            let mut sessions = self.sessions.lock();
            let entry = sessions
                .entry(meta.id.as_str().to_string())
                .or_insert_with(|| (meta.clone(), Vec::new(), 0, 0));
            entry.1.extend(closers.iter().cloned());
            entry.2 += 1;
            entry.3 += closers.len() as u64;
            Ok(())
        }

        async fn list(&self) -> Result<Vec<SessionHeader>, String> {
            Ok(self
                .sessions
                .lock()
                .values()
                .map(|(meta, _, _, _)| meta.clone())
                .collect())
        }
    }

    fn header(id: &str, created_at: u64) -> SessionHeader {
        SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: session_id(id),
            created_at,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }
    }

    fn turn_event(seq: u64, turn: u64) -> SessionEvent {
        SessionEvent {
            type_: "turn/start".to_string(),
            seq,
            time: 0,
            data: serde_json::json!({"turn": turn}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    fn user_event(seq: u64) -> SessionEvent {
        SessionEvent {
            type_: "user/message".to_string(),
            seq,
            time: 0,
            data: serde_json::json!({
                "id": format!("m{seq}"),
                "role": "user",
                "content": [{"type": "text", "text": "hi"}],
                "source": {"kind": "user"},
            }),
            ignorable: None,
            surface_op: Some(SurfaceOp::Append),
            source_event_seqs: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_and_append_are_lazy_then_durable() {
        let ctx = Context::root();
        let _store = SessionStore::install(&ctx);
        let backend = MemoryBackend::new("memory");
        let coordinator = PersistenceCoordinator::new(
            &ctx,
            backend.clone(),
            PersistenceCoordinatorOptions::default(),
        );

        // Lazy creation: no artifact until the first append.
        coordinator.create(header("s1", 1)).await.unwrap();
        assert!(
            backend.sessions.lock().is_empty(),
            "lazy creation writes nothing"
        );

        // Contiguity contract.
        let error = coordinator
            .append(&session_id("s1"), &[turn_event(5, 1)])
            .await
            .unwrap_err();
        assert!(error.contains("append seq mismatch"), "{error}");

        coordinator
            .append(&session_id("s1"), &[turn_event(0, 1), user_event(1)])
            .await
            .unwrap();
        {
            let stored = backend.sessions.lock();
            assert_eq!(stored.get("s1").unwrap().1.len(), 2, "durable after append");
        }

        // Duplicate creation rejects.
        let error = coordinator.create(header("s1", 2)).await.unwrap_err();
        assert!(error.contains("already exists"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn load_prepares_and_repairs_interrupted_tail() {
        let ctx = Context::root();
        let _store = SessionStore::install(&ctx);
        let backend = MemoryBackend::new("memory");
        // Seed the backend with an interrupted turn (open turn, dangling
        // tool call, no turn/end).
        {
            let mut sessions = backend.sessions.lock();
            sessions.insert(
                "s1".to_string(),
                (header("s1", 1), vec![turn_event(0, 1), user_event(1)], 1, 2),
            );
        }
        let coordinator = PersistenceCoordinator::new(
            &ctx,
            backend.clone(),
            PersistenceCoordinatorOptions::default(),
        );

        // Cold load: commits the repair (synthetic turn/end) and returns the
        // balanced inspection.
        let inspection = coordinator.load(&session_id("s1")).await.unwrap();
        assert_eq!(inspection.meta.id.as_str(), "s1");
        assert_eq!(
            inspection.events.len(),
            3,
            "balanced with synthetic turn/end"
        );
        assert_eq!(inspection.events[2].type_, "turn/end");
        // The repair went durable.
        assert_eq!(backend.sessions.lock().get("s1").unwrap().1.len(), 3);

        // prepare returns an unpublished Session.
        let preparation = coordinator.prepare(&session_id("s1")).await.unwrap();
        assert_eq!(
            preparation.session.events().len(),
            4,
            "session + end-seed marker"
        );
        assert_eq!(preparation.session.first_live_seq(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unsupported_format_and_unknown_events_refuse() {
        let ctx = Context::root();
        let _store = SessionStore::install(&ctx);
        let backend = MemoryBackend::new("memory");
        {
            let mut sessions = backend.sessions.lock();
            // A future format version.
            let mut future = header("f1", 1);
            future.version = SESSION_FORMAT_VERSION + 1;
            sessions.insert("f1".to_string(), (future, Vec::new(), 1, 0));
            // A current version with an unknown required event type.
            sessions.insert(
                "u1".to_string(),
                (
                    header("u1", 1),
                    vec![SessionEvent {
                        type_: "future/event".to_string(),
                        seq: 0,
                        time: 0,
                        data: serde_json::json!({}),
                        ignorable: None,
                        surface_op: None,
                        source_event_seqs: None,
                    }],
                    1,
                    1,
                ),
            );
            // An unknown event marked ignorable is accepted.
            sessions.insert(
                "ok1".to_string(),
                (
                    header("ok1", 1),
                    vec![SessionEvent {
                        type_: "future/event".to_string(),
                        seq: 0,
                        time: 0,
                        data: serde_json::json!({}),
                        ignorable: Some(true),
                        surface_op: None,
                        source_event_seqs: None,
                    }],
                    1,
                    1,
                ),
            );
        }
        let coordinator = PersistenceCoordinator::new(
            &ctx,
            backend.clone(),
            PersistenceCoordinatorOptions::default(),
        );

        let error = coordinator.load(&session_id("f1")).await.unwrap_err();
        assert!(error.contains("written by a newer harness"), "{error}");
        let error = coordinator.load(&session_id("u1")).await.unwrap_err();
        assert!(error.contains("unknown to this harness"), "{error}");
        let inspection = coordinator.load(&session_id("ok1")).await.unwrap();
        assert_eq!(inspection.events.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn live_session_events_flow_through_flush() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let backend = MemoryBackend::new("memory");
        let coordinator = PersistenceCoordinator::new(
            &ctx,
            backend.clone(),
            PersistenceCoordinatorOptions {
                write_batch_max_delay_ms: 60_000,
                ..Default::default()
            },
        );

        // A live session: session/created initializes the coordinator state.
        let session = store
            .create(
                &ctx,
                Some(session_id("live")),
                Some(dsh_session::CreateSessionOptions {
                    seed: None,
                    meta: Some(CreateSessionMeta::default()),
                }),
            )
            .await
            .unwrap();
        // Seed event (constructor-owned): persisted once at creation.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        coordinator.flush(&session).await.unwrap();
        assert!(
            backend.sessions.lock().get("live").is_none(),
            "an empty session stays lazy (no materialized artifact)"
        );

        // Live append → session/event → write-behind queue → flush drains.
        session
            .append("turn/start", serde_json::json!({"turn": 1}), None)
            .unwrap();
        coordinator.flush(&session).await.unwrap();
        assert_eq!(backend.sessions.lock().get("live").unwrap().1.len(), 1);
        assert_eq!(backend.sessions.lock().get("live").unwrap().1[0].seq, 0);

        // The live view is inspectable without loading.
        let inspection = coordinator.inspect(&session_id("live")).await.unwrap();
        assert_eq!(inspection.events.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_live_initialization_reuses_one_write_controller() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let backend = MemoryBackend::new("memory");
        let coordinator =
            PersistenceCoordinator::new(&ctx, backend, PersistenceCoordinatorOptions::default());
        let session = store
            .create(&ctx, Some(session_id("init-race")), None)
            .await
            .unwrap();
        coordinator.live.lock().remove(&session_ptr(&session));

        let gate = Arc::new(std::sync::Barrier::new(33));
        let controllers = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let runtime = tokio::runtime::Handle::current();
        std::thread::scope(|scope| {
            for _ in 0..32 {
                let coordinator = coordinator.clone();
                let session = session.clone();
                let gate = gate.clone();
                let controllers = controllers.clone();
                let runtime = runtime.clone();
                scope.spawn(move || {
                    gate.wait();
                    let _runtime = runtime.enter();
                    controllers
                        .lock()
                        .push(coordinator.init_for(&session).writes);
                });
            }
            gate.wait();
        });

        let controllers = controllers.lock();
        let first = controllers.first().expect("at least one controller");
        assert!(
            controllers
                .iter()
                .all(|controller| Arc::ptr_eq(first, controller)),
            "concurrent init_for calls returned different controllers"
        );
        assert_eq!(coordinator.live.lock().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn retirement_clears_state_and_dispose_drains() {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let backend = MemoryBackend::new("memory");
        let coordinator = PersistenceCoordinator::new(
            &ctx,
            backend.clone(),
            PersistenceCoordinatorOptions {
                write_batch_max_delay_ms: 60_000,
                ..Default::default()
            },
        );

        let session = store
            .create(&ctx, Some(session_id("r1")), None)
            .await
            .unwrap();
        session
            .append("turn/start", serde_json::json!({"turn": 1}), None)
            .unwrap();
        coordinator.flush(&session).await.unwrap();
        assert_eq!(backend.sessions.lock().get("r1").unwrap().1.len(), 1);

        // Detach: retirement drains and releases the state.
        let live_count = coordinator.live.lock().len();
        assert_eq!(live_count, 1);
        // Retirement runs through session/disposed; simulate by calling
        // retire directly (the store detach path is covered in session
        // tests).
        coordinator.retire(&session);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            coordinator.live.lock().len(),
            0,
            "retirement released the controller"
        );
        assert!(coordinator.states.lock().get("r1").is_none());
    }
}
