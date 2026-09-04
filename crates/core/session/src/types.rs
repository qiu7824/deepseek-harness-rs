//! Core session value types: Rust port of
//! `packages/core/session/src/types.ts`.
//!
//! The TS `SessionEventMap` is merge-extensible (plugins append event types
//! without touching core types). Rust models the merge-extensible map the
//! same way the runtime sees it: a string `type` plus a lossless-JSON
//! `data` payload, with typed constructors for every core event.

use std::fmt;
use std::path::Path;

use dsh_brand::Branded;
use dsh_llm::{LlmCallConfig, LlmCallConfigAdapterDefaults, LlmFailure, ToolSchema};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

use crate::json::snapshot_json_value;

/// Identifies one session in the store (and its persistence artifacts).
#[doc(hidden)]
pub enum SessionIdTag {}
pub type SessionId = Branded<SessionIdTag>;

/// Brand a string as a [`SessionId`] (a compile-time cast — no runtime
/// cost; TS `SessionId(id)`).
pub fn session_id(id: impl Into<String>) -> SessionId {
    Branded::new(id)
}

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

macro_rules! session_position {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[repr(transparent)]
        #[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            pub const ZERO: Self = Self(0);

            pub fn new(value: u64) -> Result<Self, String> {
                if value > MAX_SAFE_INTEGER {
                    return Err(format!(
                        "{} must be a non-negative safe integer, got {value}",
                        stringify!($name)
                    ));
                }
                Ok(Self(value))
            }

            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl PartialEq<u64> for $name {
            fn eq(&self, other: &u64) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for u64 {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl std::ops::Add<u64> for $name {
            type Output = u64;

            fn add(self, rhs: u64) -> Self::Output {
                self.0 + rhs
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_u64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = u64::deserialize(deserializer)?;
                Self::new(value).map_err(D::Error::custom)
            }
        }
    };
}

session_position!(SessionSeq, "Sequence number of one existing Session event.");
session_position!(
    SessionLogOffset,
    "A Session log gap, prefix length, or read offset."
);

/// The on-disk session format version, stamped into every newly-written
/// [`SessionHeader`] and enforced by every persistence backend on load.
pub const SESSION_FORMAT_VERSION: u64 = 0;

/// Immutable validated storage metadata, kept outside the conversation
/// event log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionHeader {
    /// On-disk format version (must equal [`SESSION_FORMAT_VERSION`]).
    pub version: u64,
    /// The session's id (mirrors the [`crate::Session`]'s id).
    pub id: SessionId,
    /// Non-negative Unix epoch milliseconds when the session was created.
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    /// Absolute working directory the session was created in (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The session this one was forked from (seed lineage), if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "parentSession"
    )]
    pub parent_session: Option<SessionId>,
    /// Whether this session contains a fork-inherited event prefix.
    #[serde(rename = "isSeeded")]
    pub is_seeded: bool,
    /// Coarse product classification for a subagent child session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Delegation depth; absent (zero) for a top-level session.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "delegationDepth"
    )]
    pub delegation_depth: Option<u64>,
    /// Id of the agent preset this session's agent was composed from.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "agentPreset"
    )]
    pub agent_preset: Option<String>,
}

/// Options for creating a [`crate::Session`] via the store.
#[derive(Debug, Clone, Default)]
pub struct CreateSessionOptions {
    /// Initial replay or fork history supplied at construction.
    pub seed: Option<Vec<crate::SessionEvent>>,
    /// Exact fork-inherited prefix length for a seeded Session.
    pub inherited_event_count: Option<SessionLogOffset>,
    /// Storage metadata folded into the [`SessionHeader`].
    pub meta: Option<CreateSessionMeta>,
}

/// Caller-supplied storage metadata (TS `CreateSessionOptions.meta`).
#[derive(Debug, Clone, Default)]
pub struct CreateSessionMeta {
    pub cwd: Option<String>,
    pub parent_session: Option<SessionId>,
    pub created_at: Option<u64>,
    pub is_seeded: Option<bool>,
    pub origin: Option<String>,
    pub delegation_depth: Option<u64>,
    pub agent_preset: Option<String>,
}

/// Why an active agent driver was cancelled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AgentCancelCause {
    User,
    Parent,
    Hook { reason: String },
    Disposed,
}

/// Durable cancellation cause, including imports whose original coarse
/// record carried no cause.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TurnEndCancelCause {
    User,
    Parent,
    Hook { reason: String },
    Disposed,
    Legacy,
}

impl From<AgentCancelCause> for TurnEndCancelCause {
    fn from(cause: AgentCancelCause) -> Self {
        match cause {
            AgentCancelCause::User => TurnEndCancelCause::User,
            AgentCancelCause::Parent => TurnEndCancelCause::Parent,
            AgentCancelCause::Hook { reason } => TurnEndCancelCause::Hook { reason },
            AgentCancelCause::Disposed => TurnEndCancelCause::Disposed,
        }
    }
}

/// Why a turn ended (merge-extensible sum type in TS).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TurnEndReason {
    Completed,
    /// A cancellation request interrupted the live turn.
    Aborted {
        reason: TurnEndCancelCause,
    },
    Blocked,
    /// The turn failed with a structured failure.
    Error {
        error: LlmFailure,
    },
    /// At least one step reached its output-token ceiling.
    MaxTokens,
    /// A persistence backend closed a crash-orphaned turn on reload.
    Interrupted,
}

impl TurnEndReason {
    pub fn kind(&self) -> &'static str {
        match self {
            TurnEndReason::Completed => "completed",
            TurnEndReason::Aborted { .. } => "aborted",
            TurnEndReason::Blocked => "blocked",
            TurnEndReason::Error { .. } => "error",
            TurnEndReason::MaxTokens => "max-tokens",
            TurnEndReason::Interrupted => "interrupted",
        }
    }
}

/// One entry in an agent's todo list — the unit of the `todo/write`
/// event's whole-list snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    /// What this task is — a short imperative line shown in the UI.
    pub content: String,
    /// Lifecycle state.
    pub status: TodoStatus,
}

/// Three-state todo lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// Logged request state outside derived history: call config, system
/// prompt, and tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpochHeader {
    /// The conversation's call configuration.
    pub config: LlmCallConfig,
    /// Effective config fields materialized from the exact adapter.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "adapterDefaults"
    )]
    pub adapter_defaults: Option<LlmCallConfigAdapterDefaults>,
    /// Rendered system prompt text; absent for a system-less request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Assembled tool schemas; absent for a tool-less request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSchema>>,
}

/// Registration-bound metadata for one resolved model route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestContext {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "contextWindowEstimated"
    )]
    pub context_window_estimated: Option<bool>,
    /// Registered provider route the metadata belongs to.
    pub provider: String,
    /// Provider-owned model id the metadata belongs to.
    pub model: String,
    /// Maximum combined request and response context in tokens.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "contextWindow"
    )]
    pub context_window: Option<u64>,
}

/// Why a `request/header` snapshot was appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestHeaderReason {
    Initial,
    Resume,
    Change,
}

/// How a session event entered the ordered surface.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceOp {
    /// Added to the tail — the normal path.
    Append,
    /// Replaces surface nodes from `start` (inclusive) through `end`
    /// (inclusive) with this node.
    Replace { start: u64, end: u64 },
}

impl SurfaceOp {
    pub fn is_append(&self) -> bool {
        matches!(self, SurfaceOp::Append)
    }
}

impl Serialize for SurfaceOp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            SurfaceOp::Append => serializer.serialize_str("append"),
            SurfaceOp::Replace { start, end } => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("op", "replace")?;
                map.serialize_entry("start", start)?;
                map.serialize_entry("end", end)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for SurfaceOp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SurfaceOpVisitor;

        impl<'de> serde::de::Visitor<'de> for SurfaceOpVisitor {
            type Value = SurfaceOp;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a surface operation ('append' or a replace object)")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                match value {
                    "append" => Ok(SurfaceOp::Append),
                    other => Err(E::custom(format!("invalid surface op string {other:?}"))),
                }
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut op: Option<String> = None;
                let mut start: Option<u64> = None;
                let mut end: Option<u64> = None;
                let mut count = 0;
                while let Some((key, value)) = map.next_entry::<String, JsonValue>()? {
                    count += 1;
                    match key.as_str() {
                        "op" => {
                            op = value
                                .as_str()
                                .map(|s| s.to_string())
                                .or_else(|| Some(value.to_string()));
                        }
                        "start" => start = value.as_u64(),
                        "end" => end = value.as_u64(),
                        _ => {}
                    }
                }
                if count != 3 || op.as_deref() != Some("replace") {
                    return Err(A::Error::custom("invalid replace surfaceOp shape"));
                }
                match (start, end) {
                    (Some(start), Some(end)) => Ok(SurfaceOp::Replace { start, end }),
                    _ => Err(A::Error::custom(
                        "replace surfaceOp start/end must be non-negative integers",
                    )),
                }
            }
        }

        deserializer.deserialize_any(SurfaceOpVisitor)
    }
}

/// Surface placement and cited source-event seqs for
/// [`crate::Session::append`].
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceIntent {
    pub surface_op: SurfaceOp,
    /// Complete set of known source-event seqs; absent means the event does
    /// not record which earlier events produced it.
    pub source_event_seqs: Option<Vec<u64>>,
}

/// One immutable entry in the session log.
///
/// The TS discriminated union collapses to this runtime envelope: `type` +
/// `seq` + `time` + lossless-JSON `data`, with surface metadata only on
/// surface-eligible events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    #[serde(rename = "type")]
    pub type_: String,
    /// Monotonic sequence number within the session.
    pub seq: SessionSeq,
    /// Unix epoch milliseconds.
    pub time: i64,
    pub data: JsonValue,
    /// `true` marks an event a reader may safely skip when it does not
    /// recognize `type`; absent means required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignorable: Option<bool>,
    /// How this event entered the surface; absent for non-surface events.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "surfaceOp")]
    pub surface_op: Option<SurfaceOp>,
    /// Seq numbers of earlier events that this event cites as sources.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "sourceEventSeqs"
    )]
    pub source_event_seqs: Option<Vec<u64>>,
}

// ---- Core event data constructors ----

/// `turn/start` event data.
pub fn turn_start_data(turn: u64) -> JsonValue {
    serde_json::json!({ "turn": turn })
}

/// `turn/end` event data.
pub fn turn_end_data(turn: u64, reason: &TurnEndReason) -> JsonValue {
    serde_json::json!({ "turn": turn, "reason": reason })
}

/// `step/start` / `step/end` event data.
pub fn step_data(turn: u64, step: u64) -> JsonValue {
    serde_json::json!({ "turn": turn, "step": step })
}

/// `assistant/chunk` event data.
pub fn assistant_chunk_data(turn: u64, step: u64, chunk: &dsh_llm::StreamChunk) -> JsonValue {
    serde_json::json!({ "turn": turn, "step": step, "chunk": chunk })
}

/// `assistant/message` event data.
pub fn assistant_message_data(
    turn: u64,
    step: u64,
    message: &dsh_llm::Message,
    usage: Option<&dsh_llm::TokenUsage>,
) -> JsonValue {
    let mut value = serde_json::json!({ "turn": turn, "step": step, "message": message });
    if let Some(usage) = usage {
        value["usage"] = serde_json::to_value(usage).unwrap_or_default();
    }
    value
}

/// `tool/call` event data.
pub fn tool_call_data(
    turn: u64,
    step: u64,
    call_id: &dsh_llm::CallId,
    name: &str,
    arguments: &str,
) -> JsonValue {
    serde_json::json!({
        "turn": turn,
        "step": step,
        "callId": call_id,
        "name": name,
        "arguments": arguments,
    })
}

/// `tool/result` event data.
pub fn tool_result_data(
    turn: u64,
    step: u64,
    message: &dsh_llm::Message,
    error: Option<(&str, &str)>,
    meta: Option<&JsonValue>,
) -> JsonValue {
    let mut value = serde_json::json!({ "turn": turn, "step": step, "message": message });
    if let Some((name, code)) = error {
        value["error"] = serde_json::json!({ "name": name, "code": code });
    }
    if let Some(meta) = meta {
        value["meta"] = meta.clone();
    }
    value
}

/// `todo/write` event data.
pub fn todo_write_data(todos: &[TodoItem]) -> JsonValue {
    serde_json::json!({ "todos": todos })
}

/// `request/header` event data.
pub fn request_header_data(header: &EpochHeader, reason: RequestHeaderReason) -> JsonValue {
    serde_json::json!({ "header": header, "reason": reason })
}

/// `session/end-seed` event data (the empty payload).
pub fn end_seed_data() -> JsonValue {
    serde_json::json!({})
}

// ---- Header validation (TS index.ts `validateSessionHeader`) ----

/// Validate and freeze one detached creation header in place.
pub fn validate_session_header(id: &SessionId, input: &JsonValue) -> Result<SessionHeader, String> {
    let Some(record) = input.as_object() else {
        return Err("session header is not a plain JSON record".to_string());
    };
    let version = record
        .get("version")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| {
            format!(
                "session header version must be {SESSION_FORMAT_VERSION}, got {}",
                render_unknown(record.get("version"))
            )
        })?;
    if version != SESSION_FORMAT_VERSION {
        return Err(format!(
            "session header version must be {SESSION_FORMAT_VERSION}, got {version}"
        ));
    }
    let header_id = record
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "session header id must be a string".to_string())?;
    if header_id != id.as_str() {
        return Err(format!(
            "session header id \"{header_id}\" does not match session id \"{}\"",
            id.as_str()
        ));
    }
    let created_at = record
        .get("createdAt")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| {
            "session header createdAt must be a non-negative safe integer".to_string()
        })?;
    if let Some(cwd) = record.get("cwd") {
        let cwd = cwd
            .as_str()
            .ok_or_else(|| "session header cwd must be a string".to_string())?;
        if !Path::new(cwd).is_absolute() {
            return Err(format!(
                "session header cwd must be an absolute path, got \"{cwd}\""
            ));
        }
    }
    if let Some(parent) = record.get("parentSession")
        && !parent.is_string()
    {
        return Err("session header parentSession must be a string".to_string());
    }
    if record.contains_key("seedLength") {
        return Err("session header has invalid field \"seedLength\"".to_string());
    }
    let is_seeded = record
        .get("isSeeded")
        .and_then(JsonValue::as_bool)
        .ok_or_else(|| "session header isSeeded must be a boolean".to_string())?;
    if let Some(value) = record.get("delegationDepth")
        && value.as_u64().is_none()
    {
        return Err(
            "session header delegationDepth must be a non-negative safe integer".to_string(),
        );
    }
    if let Some(origin) = record.get("origin")
        && origin.as_str() != Some("subagent")
    {
        return Err("session header origin must be \"subagent\"".to_string());
    }
    if let Some(preset) = record.get("agentPreset")
        && !preset.is_string()
    {
        return Err("session header agentPreset must be a string".to_string());
    }
    Ok(SessionHeader {
        version,
        id: id.clone(),
        created_at,
        cwd: record
            .get("cwd")
            .and_then(|value| value.as_str().map(str::to_string)),
        parent_session: record
            .get("parentSession")
            .and_then(|value| value.as_str().map(session_id)),
        is_seeded,
        origin: record
            .get("origin")
            .and_then(|value| value.as_str().map(str::to_string)),
        delegation_depth: record
            .get("delegationDepth")
            .and_then(|value| value.as_u64()),
        agent_preset: record
            .get("agentPreset")
            .and_then(|value| value.as_str().map(str::to_string)),
    })
}

/// Detach, validate, and freeze the creation metadata published by a
/// session (TS `snapshotSessionHeader`).
pub fn snapshot_session_header(
    id: &SessionId,
    source: Option<&SessionHeader>,
) -> Result<SessionHeader, String> {
    let now = chrono::Utc::now().timestamp_millis() as u64;
    let input: JsonValue = match source {
        Some(source) => serde_json::to_value(source)
            .map_err(|_| "session header is not losslessly JSON-serializable".to_string())?,
        None => serde_json::json!({
            "version": SESSION_FORMAT_VERSION,
            "id": id,
            "createdAt": now,
            "isSeeded": false,
        }),
    };
    let snapshot = snapshot_json_value(&input)
        .ok_or_else(|| "session header is not losslessly JSON-serializable".to_string())?;
    validate_session_header(id, &snapshot)
}

fn render_unknown(value: Option<&JsonValue>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "undefined".to_string(),
    }
}
