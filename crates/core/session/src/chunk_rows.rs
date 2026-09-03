//! Lossless storage packing for `assistant/chunk` delta runs. Rust port of
//! `packages/core/session/src/chunk-rows.ts`.
//!
//! Storage rows are a durable-encoding vocabulary, NOT session events:
#![allow(clippy::enum_variant_names, clippy::too_many_arguments)]
// Delta suffixes and full wire-event fixtures intentionally mirror the durable protocol. they
//! never enter `Session.events` and use bare (slash-less) type tags. The
//! encoder whitelists exact shapes — anything it does not fully recognize is
//! stored verbatim.

use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

use crate::json::has_exact_keys;
use crate::types::SessionEvent;

/// Minimum members before a run packs (format constant, not a tunable).
const MIN_RUN: usize = 3;

/// The chunk kinds that may pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeltaKind {
    TextDelta,
    ReasoningDelta,
    ToolCallDelta,
}

/// Fields shared by every packed run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRunData {
    pub turn: u64,
    pub step: u64,
    /// The stream block index every member shares.
    pub index: u64,
    /// Epoch-ms gaps between consecutive members; length is one less than
    /// the member count.
    pub dt: Vec<i64>,
    /// One entry per member, never joined — token boundaries are data.
    pub texts: Vec<String>,
}

/// Payload of a `tool-call-chunks` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRunData {
    pub turn: u64,
    pub step: u64,
    pub index: u64,
    pub dt: Vec<i64>,
    /// The run-constant call identity.
    pub id: String,
    /// Present iff every member carried it, with one uniform value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Each member's raw arguments fragment.
    pub args: Vec<String>,
}

/// A packed run of consecutive delta chunk events, discriminated on `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ChunkRow {
    TextChunks {
        #[serde(rename = "seq0")]
        seq0: u64,
        #[serde(rename = "time0")]
        time0: i64,
        data: TextRunData,
    },
    ReasoningChunks {
        #[serde(rename = "seq0")]
        seq0: u64,
        #[serde(rename = "time0")]
        time0: i64,
        data: TextRunData,
    },
    ToolCallChunks {
        #[serde(rename = "seq0")]
        seq0: u64,
        #[serde(rename = "time0")]
        time0: i64,
        data: ToolCallRunData,
    },
}

impl ChunkRow {
    pub fn type_tag(&self) -> &'static str {
        match self {
            ChunkRow::TextChunks { .. } => "text-chunks",
            ChunkRow::ReasoningChunks { .. } => "reasoning-chunks",
            ChunkRow::ToolCallChunks { .. } => "tool-call-chunks",
        }
    }
}

/// One durable log line's JSON value: a session event verbatim, or a packed
/// chunk row. Events are carried as raw JSON so unknown envelope fields from
/// newer harnesses round-trip verbatim (the TS `StorageRecord` passthrough).
#[derive(Debug, Clone, PartialEq)]
pub enum StorageRecord {
    Event(JsonValue),
    Row(ChunkRow),
}

impl StorageRecord {
    /// Classify a raw parsed JSON value (TS `decodeStorageRecord`'s tag
    /// test).
    pub fn from_json(value: JsonValue) -> Result<Self, String> {
        let tag = value
            .get("type")
            .and_then(|record| record.as_str())
            .map(str::to_string);
        match tag.as_deref() {
            Some("text-chunks" | "reasoning-chunks" | "tool-call-chunks") => {
                let tag = tag.as_deref().unwrap();
                validate_row_value(tag, &value)?;
                let row: ChunkRow = serde_json::from_value(value)
                    .map_err(|error| format!("malformed {tag} storage row: {error}"))?;
                Ok(StorageRecord::Row(row))
            }
            _ => Ok(StorageRecord::Event(value)),
        }
    }

    pub fn to_json(&self) -> JsonValue {
        match self {
            StorageRecord::Event(value) => value.clone(),
            StorageRecord::Row(row) => serde_json::to_value(row).unwrap_or(JsonValue::Null),
        }
    }
}

impl Serialize for StorageRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_json().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StorageRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = JsonValue::deserialize(deserializer)?;
        StorageRecord::from_json(value).map_err(D::Error::custom)
    }
}

/// Classify an event for packing; `None` stores the event verbatim.
fn classify(event: &JsonValue) -> Option<DeltaKind> {
    if event.get("type")?.as_str() != Some("assistant/chunk") {
        return None;
    }
    let record = event.as_object()?;
    if !has_exact_keys(record, &["type", "seq", "time", "data"]) {
        return None;
    }
    event.get("seq")?.as_u64()?;
    if !is_integer_number(event.get("time")?) {
        return None;
    }
    let data = event.get("data")?.as_object()?;
    if !has_exact_keys(data, &["turn", "step", "chunk"]) {
        return None;
    }
    if data.get("turn")?.as_u64().is_none() || data.get("step")?.as_u64().is_none() {
        return None;
    }
    let chunk = data.get("chunk")?.as_object()?;
    chunk.get("index")?.as_u64()?;
    match chunk.get("type")?.as_str()? {
        "text-delta" | "reasoning-delta" => {
            if has_exact_keys(chunk, &["type", "index", "text"]) && chunk.get("text")?.is_string() {
                Some(
                    if chunk.get("type").and_then(|v| v.as_str()) == Some("text-delta") {
                        DeltaKind::TextDelta
                    } else {
                        DeltaKind::ReasoningDelta
                    },
                )
            } else {
                None
            }
        }
        "tool-call-delta" => {
            let with_name =
                has_exact_keys(chunk, &["type", "index", "id", "name", "argumentsDelta"])
                    && chunk.get("name")?.is_string();
            let without_name = has_exact_keys(chunk, &["type", "index", "id", "argumentsDelta"]);
            let shape_ok = with_name || without_name;
            if shape_ok && chunk.get("id")?.is_string() && chunk.get("argumentsDelta")?.is_string()
            {
                Some(DeltaKind::ToolCallDelta)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// A JSON number that is an integer (safe-integer semantics on the Rust
/// integer types).
fn is_integer_number(value: &JsonValue) -> bool {
    value.is_i64() || value.is_u64()
}

/// The block index of a whitelisted delta chunk.
fn index_of(event: &JsonValue) -> Option<u64> {
    event.get("data")?.get("chunk")?.get("index")?.as_u64()
}

/// The tool-call fields of a whitelisted delta chunk.
fn tool_call_of(event: &JsonValue) -> Option<(String, Option<String>)> {
    let chunk = event.get("data")?.get("chunk")?;
    Some((
        chunk.get("id")?.as_str()?.to_string(),
        chunk
            .get("name")
            .and_then(|name| name.as_str().map(str::to_string)),
    ))
}

/// Whether `next` extends a run ending in `prev` (same kind already checked
/// by the caller).
fn continues(prev: &JsonValue, next: &JsonValue, kind: DeltaKind) -> bool {
    let prev_seq = prev.get("seq").and_then(|value| value.as_u64());
    let next_seq = next.get("seq").and_then(|value| value.as_u64());
    let (Some(prev_seq), Some(next_seq)) = (prev_seq, next_seq) else {
        return false;
    };
    if next_seq != prev_seq + 1 {
        return false;
    }
    let prev_time = prev.get("time").and_then(|value| value.as_i64());
    let next_time = next.get("time").and_then(|value| value.as_i64());
    let (Some(prev_time), Some(next_time)) = (prev_time, next_time) else {
        return false;
    };
    let gap = next_time.checked_sub(prev_time);
    if gap.is_none() {
        return false;
    }
    let prev_data = prev.get("data");
    let next_data = next.get("data");
    let turn_matches =
        prev_data.and_then(|d| d.get("turn")) == next_data.and_then(|d| d.get("turn"));
    let step_matches =
        prev_data.and_then(|d| d.get("step")) == next_data.and_then(|d| d.get("step"));
    if !turn_matches || !step_matches {
        return false;
    }
    if index_of(next) != index_of(prev) {
        return false;
    }
    if kind != DeltaKind::ToolCallDelta {
        return true;
    }
    let (Some(prev_call), Some(next_call)) = (tool_call_of(prev), tool_call_of(next)) else {
        return false;
    };
    prev_call.0 == next_call.0
        && prev_call.1.is_some() == next_call.1.is_some()
        && prev_call.1 == next_call.1
}

/// Build the row for a completed run.
fn build_row(kind: DeltaKind, run: &[JsonValue]) -> ChunkRow {
    let first = &run[0];
    let first_data = first.get("data").expect("whitelisted event carries data");
    let turn = first_data
        .get("turn")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let step = first_data
        .get("step")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let index = index_of(first).unwrap_or(0);
    let dt: Vec<i64> = run
        .windows(2)
        .map(|pair| {
            let prev_time = pair[0]
                .get("time")
                .and_then(|value| value.as_i64())
                .unwrap_or(0);
            let next_time = pair[1]
                .get("time")
                .and_then(|value| value.as_i64())
                .unwrap_or(0);
            next_time - prev_time
        })
        .collect();
    let seq0 = first
        .get("seq")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let time0 = first
        .get("time")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    match kind {
        DeltaKind::ToolCallDelta => {
            let (id, name) = tool_call_of(first).expect("whitelisted tool-call chunk");
            let args: Vec<String> = run
                .iter()
                .map(|event| {
                    event
                        .get("data")
                        .and_then(|data| data.get("chunk"))
                        .and_then(|chunk| chunk.get("argumentsDelta"))
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                        .unwrap_or_default()
                })
                .collect();
            ChunkRow::ToolCallChunks {
                seq0,
                time0,
                data: ToolCallRunData {
                    turn,
                    step,
                    index,
                    dt,
                    id,
                    name,
                    args,
                },
            }
        }
        DeltaKind::TextDelta | DeltaKind::ReasoningDelta => {
            let texts: Vec<String> = run
                .iter()
                .map(|event| {
                    event
                        .get("data")
                        .and_then(|data| data.get("chunk"))
                        .and_then(|chunk| chunk.get("text"))
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                        .unwrap_or_default()
                })
                .collect();
            let data = TextRunData {
                turn,
                step,
                index,
                dt,
                texts,
            };
            if kind == DeltaKind::TextDelta {
                ChunkRow::TextChunks { seq0, time0, data }
            } else {
                ChunkRow::ReasoningChunks { seq0, time0, data }
            }
        }
    }
}

/// Pack an event batch for storage (TS `packChunkRuns`).
pub fn pack_chunk_runs(events: &[SessionEvent]) -> Vec<StorageRecord> {
    let raw: Vec<JsonValue> = events
        .iter()
        .map(|event| serde_json::to_value(event).unwrap_or(JsonValue::Null))
        .collect();
    let mut out: Vec<StorageRecord> = Vec::new();
    let mut kind: Option<DeltaKind> = None;
    let mut run: Vec<JsonValue> = Vec::new();

    let flush =
        |kind: &mut Option<DeltaKind>, run: &mut Vec<JsonValue>, out: &mut Vec<StorageRecord>| {
            if let Some(kind) = *kind {
                if run.len() >= MIN_RUN {
                    out.push(StorageRecord::Row(build_row(kind, run)));
                } else {
                    out.extend(run.drain(..).map(StorageRecord::Event));
                }
            } else {
                out.extend(run.drain(..).map(StorageRecord::Event));
            }
            *kind = None;
            run.clear();
        };

    for event in &raw {
        let Some(k) = classify(event) else {
            flush(&mut kind, &mut run, &mut out);
            out.push(StorageRecord::Event(event.clone()));
            continue;
        };
        if Some(k) == kind && run.last().is_some_and(|last| continues(last, event, k)) {
            run.push(event.clone());
            continue;
        }
        flush(&mut kind, &mut run, &mut out);
        kind = Some(k);
        run = vec![event.clone()];
    }
    flush(&mut kind, &mut run, &mut out);
    out
}

/// Throw the uniform malformed-row diagnostic.
fn malformed<T>(tag: &str, why: &str) -> Result<T, String> {
    Err(format!("malformed {tag} storage row: {why}"))
}

/// Validate one row-tagged parsed value's envelope and data, throwing on
/// any malformation (TS `validateRow`). Exact-key checks run on the RAW
/// JSON so unknown fields are rejected like the TS decoder.
fn validate_row_value(tag: &str, value: &JsonValue) -> Result<(), String> {
    let Some(record) = value.as_object() else {
        malformed(tag, "envelope must be exactly {type, seq0, time0, data}")?;
        unreachable!();
    };
    if !has_exact_keys(record, &["type", "seq0", "time0", "data"]) {
        malformed(tag, "envelope must be exactly {type, seq0, time0, data}")?;
    }
    let seq0 = match value.get("seq0").and_then(|seq| seq.as_u64()) {
        Some(seq0) => seq0,
        None => malformed(tag, "seq0 must be a non-negative safe integer")?,
    };
    let time0 = match value.get("time0").and_then(|time| time.as_i64()) {
        Some(time0) => time0,
        None => malformed(tag, "time0 must be a safe integer")?,
    };
    let Some(data) = value.get("data").and_then(|data| data.as_object()) else {
        malformed(tag, "data must be an object")?;
        unreachable!();
    };
    let payload: Vec<String> = if tag == "tool-call-chunks" {
        let with_name =
            has_exact_keys(data, &["turn", "step", "index", "id", "name", "dt", "args"]);
        let without_name = has_exact_keys(data, &["turn", "step", "index", "id", "dt", "args"]);
        if !with_name && !without_name {
            malformed(
                tag,
                "data must be exactly {turn, step, index, id, name?, dt, args}",
            )?;
        }
        if data.get("id").and_then(|id| id.as_str()).is_none()
            || (with_name && data.get("name").and_then(|name| name.as_str()).is_none())
        {
            malformed(tag, "id (and name when present) must be strings")?;
        }
        validate_payload(tag, data, "args")?
    } else {
        if !has_exact_keys(data, &["turn", "step", "index", "dt", "texts"]) {
            malformed(tag, "data must be exactly {turn, step, index, dt, texts}")?;
        }
        validate_payload(tag, data, "texts")?
    };
    // turn/step/index must be JSON integers (TS checks `typeof number`; the
    // port requires integers — see cordis-rust-notes).
    for key in ["turn", "step", "index"] {
        if data.get(key).and_then(|field| field.as_u64()).is_none() {
            malformed(tag, "turn/step/index must be numbers")?;
        }
    }
    let Some(dt) = data.get("dt").and_then(|dt| dt.as_array()) else {
        return malformed(tag, "dt must be an array of safe integers");
    };
    let gaps: Vec<i64> = dt.iter().map(|gap| gap.as_i64().unwrap_or(0)).collect();
    if dt.iter().any(|gap| gap.as_i64().is_none()) {
        malformed(tag, "dt must be an array of safe integers")?;
    }
    if gaps.len() != payload.len() - 1 {
        return malformed(
            tag,
            &format!(
                "dt length {} does not match {} members",
                gaps.len(),
                payload.len()
            ),
        );
    }
    // Reconstruction bounds: member seqs and times must stay exact integers.
    if seq0.checked_add(payload.len() as u64 - 1).is_none() {
        malformed(tag, "member seqs must stay safe integers")?;
    }
    let mut time = time0;
    for gap in gaps {
        time = match time.checked_add(gap) {
            Some(time) => time,
            None => malformed(tag, "member times must stay safe integers")?,
        };
    }
    Ok(())
}

/// Validate one payload field: a non-empty string array.
fn validate_payload(
    tag: &str,
    data: &serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<Vec<String>, String> {
    let Some(payload) = data.get(key).and_then(|payload| payload.as_array()) else {
        malformed(tag, &format!("{key} must be a non-empty string array"))?;
        unreachable!();
    };
    if payload.is_empty() || payload.iter().any(|entry| !entry.is_string()) {
        malformed(tag, &format!("{key} must be a non-empty string array"))?;
    }
    Ok(payload
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_string())
        .collect())
}

fn visit_row_events(
    row: &ChunkRow,
    on_event: &mut impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    visit_row_events_from(row, 0, on_event)
}

fn visit_row_events_from(
    row: &ChunkRow,
    start: usize,
    on_event: &mut impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    let (seq0, time0, data) = match row {
        ChunkRow::TextChunks { seq0, time0, data }
        | ChunkRow::ReasoningChunks { seq0, time0, data } => (
            *seq0,
            *time0,
            (
                data.turn,
                data.step,
                data.index,
                &data.dt,
                &data.texts,
                None,
            ),
        ),
        ChunkRow::ToolCallChunks { seq0, time0, data } => (
            *seq0,
            *time0,
            (
                data.turn,
                data.step,
                data.index,
                &data.dt,
                &data.args,
                Some((data.id.as_str(), data.name.as_deref())),
            ),
        ),
    };
    let (turn, step, index, dt, members, tool_call) = data;
    let kind = row.type_tag();
    let start = start.min(members.len());
    let mut time = time0;
    for delta in dt.iter().take(start) {
        time = time
            .checked_add(*delta)
            .ok_or_else(|| format!("{kind} time overflow"))?;
    }
    for (k, member) in members.iter().enumerate().skip(start) {
        if k > 0 && k > start {
            time = time
                .checked_add(dt[k - 1])
                .ok_or_else(|| format!("{kind} time overflow"))?;
        }
        let chunk = match (kind, tool_call) {
            ("text-chunks", _) => serde_json::json!({
                "type": "text-delta", "index": index, "text": member,
            }),
            ("reasoning-chunks", _) => serde_json::json!({
                "type": "reasoning-delta", "index": index, "text": member,
            }),
            ("tool-call-chunks", Some((id, name))) => {
                let mut value = serde_json::json!({
                    "type": "tool-call-delta",
                    "index": index,
                    "id": id,
                    "argumentsDelta": member,
                });
                if let Some(name) = name {
                    value["name"] = serde_json::Value::String(name.to_string());
                }
                value
            }
            _ => unreachable!("validateRow only returns the three row tags"),
        };
        if !on_event(SessionEvent {
            type_: "assistant/chunk".to_string(),
            seq: crate::SessionSeq::new(
                seq0.checked_add(k as u64)
                    .ok_or_else(|| format!("{kind} seq overflow"))?,
            )?,
            time,
            data: serde_json::json!({"turn": turn, "step": step, "chunk": chunk}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        })? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Decode one parsed JSONL line value into the session event(s) it stores
/// (TS `decodeStorageRecord`).
pub fn decode_storage_record(value: &JsonValue) -> Result<Vec<SessionEvent>, String> {
    let mut events = Vec::new();
    visit_storage_record_events(value, |event| {
        events.push(event);
        Ok(true)
    })?;
    Ok(events)
}

/// Decode one storage record while visiting each reconstructed event without
/// retaining a potentially enormous packed chunk run.
pub fn visit_storage_record_events(
    value: &JsonValue,
    on_event: impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    visit_owned_storage_record_events(value.clone(), on_event)
}

/// Owned counterpart used by streaming readers to avoid deep-cloning a large
/// packed row before expanding its members.
pub fn visit_owned_storage_record_events(
    value: JsonValue,
    on_event: impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    visit_decoded_storage_record_events(StorageRecord::from_json(value)?, on_event)
}

/// Visit an already decoded storage record without another JSON tree.
pub fn visit_decoded_storage_record_events(
    record: StorageRecord,
    mut on_event: impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    match record {
        StorageRecord::Row(row) => visit_row_events(&row, &mut on_event),
        StorageRecord::Event(value) => {
            let event: SessionEvent = serde_json::from_value(value)
                .map_err(|error| format!("malformed session event storage record: {error}"))?;
            on_event(event)
        }
    }
}

/// Visit only the tail of a decoded storage record. Packed rows remain fully
/// validated by deserialization, while skipped members never become event JSON.
pub fn visit_decoded_storage_record_tail(
    record: StorageRecord,
    capacity: usize,
    mut on_event: impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<bool, String> {
    match record {
        StorageRecord::Row(row) => {
            let member_count = match &row {
                ChunkRow::TextChunks { data, .. } | ChunkRow::ReasoningChunks { data, .. } => {
                    data.texts.len()
                }
                ChunkRow::ToolCallChunks { data, .. } => data.args.len(),
            };
            visit_row_events_from(&row, member_count.saturating_sub(capacity), &mut on_event)
        }
        StorageRecord::Event(value) => {
            let event: SessionEvent = serde_json::from_value(value)
                .map_err(|error| format!("malformed session event storage record: {error}"))?;
            on_event(event)
        }
    }
}
