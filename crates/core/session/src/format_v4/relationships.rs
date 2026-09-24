use super::{
    admission::{canonical_suffix, validate_v4_row_fields},
    catalog,
    vocabulary::V4Vocabulary,
    wire::{MAX_SAFE_INTEGER, count, object, validate_header},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

const SURFACE: &[&str] = &[
    "system/message",
    "developer/message",
    "user/message",
    "assistant/message",
    "tool/result",
];
const REPAIR_TEXT: &str = "The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed.";

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{field} requires a nonempty string"))
}
fn array<'a>(value: &'a Value, field: &str) -> Result<&'a Vec<Value>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{field} requires an array"))
}
fn earlier(value: &Value, seq: u64, field: &str) -> Result<u64, String> {
    let value = count(value, field)?;
    if value >= seq {
        Err(format!("{field} must name an earlier event"))
    } else {
        Ok(value)
    }
}

fn identity(value: &Value) -> [u8; 32] {
    fn hash(state: &mut Sha256, value: &Value) {
        match value {
            Value::Null => state.update([0]),
            Value::Bool(v) => state.update([1, *v as u8]),
            Value::Number(v) => {
                state.update([2]);
                let rendered = if let Some(n) = v.as_f64().filter(|n| {
                    n.is_finite() && n.abs() <= MAX_SAFE_INTEGER as f64 && n.fract() == 0.0
                }) {
                    format!("{n:.0}")
                } else {
                    v.to_string()
                };
                state.update((rendered.len() as u64).to_le_bytes());
                state.update(rendered);
            }
            Value::String(v) => {
                state.update([3]);
                state.update((v.len() as u64).to_le_bytes());
                state.update(v.as_bytes());
            }
            Value::Array(values) => {
                state.update([4]);
                state.update((values.len() as u64).to_le_bytes());
                for value in values {
                    hash(state, value);
                }
            }
            Value::Object(values) => {
                state.update([5]);
                let mut fields: Vec<_> = values.iter().collect();
                fields.sort_by(|a, b| a.0.cmp(b.0));
                state.update((fields.len() as u64).to_le_bytes());
                for (key, value) in fields {
                    state.update((key.len() as u64).to_le_bytes());
                    state.update(key.as_bytes());
                    hash(state, value);
                }
            }
        }
    }
    let mut state = Sha256::new();
    hash(&mut state, value);
    state.finalize().into()
}
fn optional_identity(value: Option<&Value>) -> Option<[u8; 32]> {
    value.map(identity)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReferenceKind {
    Other,
    HumanUser,
    Command,
    Header,
}
#[derive(Clone, PartialEq, Eq)]
struct ToolShape {
    count: u64,
    complete: bool,
}
type ToolIndex = BTreeMap<String, ToolShape>;
struct PendingTool {
    name: String,
    arguments: [u8; 32],
    started: bool,
}
struct Dispatch {
    root: String,
    parent: String,
    name: Option<[u8; 32]>,
    arguments: Option<[u8; 32]>,
    settled: bool,
}
struct Compaction {
    id: String,
    command: Option<Value>,
    turn: Option<u64>,
    summarized: bool,
    crossed_turn: bool,
}

/// Successful validation of one complete logical generation. Open transactions
/// may remain at EOF; crossed lifecycle boundaries and invalid references may not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V4ValidationSummary {
    pub event_count: u64,
    pub inherited_event_count: u64,
    pub open_turn: Option<u64>,
    pub open_step: Option<u64>,
    pub pending_tools: usize,
    pub open_compaction: bool,
}

/// One-pass native V4 relationship validation. Only reference tags, tool schema
/// name/shape indexes and unresolved lifecycle identities are retained; message
/// text, tool output and historical schema bodies are not cloned into this state.
pub struct V4Validator {
    vocabulary: V4Vocabulary,
    header: Value,
    cut: u64,
    seq: u64,
    last_marker: Option<u64>,
    failed: bool,
    turn: Option<u64>,
    step: Option<u64>,
    next_turn: u64,
    next_step: u64,
    provider: Option<String>,
    references: Vec<ReferenceKind>,
    headers: HashMap<u64, Arc<ToolIndex>>,
    header_shapes: Vec<Arc<ToolIndex>>,
    surface: Vec<u64>,
    protected: Option<u64>,
    tools: HashMap<String, PendingTool>,
    dispatches: HashMap<String, Dispatch>,
    retry_chains: HashMap<[u8; 32], (u64, String)>,
    retry_ids: HashMap<String, [u8; 32]>,
    scheduled: HashMap<(String, u64), (u64, u64)>,
    started_retries: HashSet<(String, u64)>,
    commands: HashSet<String>,
    catalogs: HashSet<String>,
    compaction: Option<Compaction>,
}

impl V4Validator {
    pub fn new(header: Value, inherited_event_count: u64) -> Result<Self, String> {
        Self::with_vocabulary(header, inherited_event_count, V4Vocabulary::default())
    }

    pub fn with_vocabulary(
        header: Value,
        inherited_event_count: u64,
        vocabulary: V4Vocabulary,
    ) -> Result<Self, String> {
        validate_header(&header, 4)?;
        if inherited_event_count > MAX_SAFE_INTEGER
            || header["isSeeded"] != true && inherited_event_count != 0
        {
            return Err("invalid V4 inherited cut".into());
        }
        Ok(Self {
            vocabulary,
            header,
            cut: inherited_event_count,
            seq: 0,
            last_marker: None,
            failed: false,
            turn: None,
            step: None,
            next_turn: 1,
            next_step: 1,
            provider: None,
            references: vec![],
            headers: HashMap::new(),
            header_shapes: vec![],
            surface: vec![],
            protected: None,
            tools: HashMap::new(),
            dispatches: HashMap::new(),
            retry_chains: HashMap::new(),
            retry_ids: HashMap::new(),
            scheduled: HashMap::new(),
            started_retries: HashSet::new(),
            commands: HashSet::new(),
            catalogs: HashSet::new(),
            compaction: None,
        })
    }
    pub fn push(&mut self, row: &Value) -> Result<(), String> {
        if self.failed {
            return Err(
                "V4 validation already failed; staged generation cannot be published".into(),
            );
        }
        let result = self.accept(row);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn finish(self) -> Result<V4ValidationSummary, String> {
        if self.failed {
            return Err("V4 validation failed; staged generation cannot be published".into());
        }
        if self.cut > self.seq
            || self.header["isSeeded"] == true && self.last_marker != Some(self.cut)
            || self.header["isSeeded"] != true && self.last_marker.is_some()
        {
            return Err("V4 inherited marker and header disagree".into());
        }
        if self.compaction.as_ref().is_some_and(|c| c.crossed_turn) {
            return Err("turn boundary crosses an unfinished compaction".into());
        }
        Ok(V4ValidationSummary {
            event_count: self.seq,
            inherited_event_count: self.cut,
            open_turn: self.turn,
            open_step: self.step,
            pending_tools: self.tools.len(),
            open_compaction: self.compaction.is_some(),
        })
    }
    fn require_turn(&self, kind: &str) -> Result<(), String> {
        if self.turn.is_none() {
            Err(format!("{kind} is outside an open turn"))
        } else {
            Ok(())
        }
    }
    fn require_step(&self, kind: &str, data: &Value) -> Result<(), String> {
        if self.turn.is_none()
            || self.step.is_none()
            || Some(count(&data["turn"], "turn")?) != self.turn
            || Some(count(&data["step"], "step")?) != self.step
        {
            return Err(format!("{kind} does not match an open turn and step"));
        }
        Ok(())
    }
    fn close_tools(&self, kind: &str) -> Result<(), String> {
        if let Some(id) = self.tools.keys().next() {
            Err(format!("{kind} leaves unresolved tool call {id}"))
        } else {
            Ok(())
        }
    }
    fn source_kind(&self, seq: u64) -> Result<ReferenceKind, String> {
        self.references
            .get(usize::try_from(seq).map_err(|_| "sequence overflow")?)
            .copied()
            .ok_or_else(|| "reference has no accepted earlier event".into())
    }

    fn envelope<'a>(&self, row: &'a Value) -> Result<(&'a str, bool), String> {
        let value = object(row, "V4 event")?;
        let kind = text(&row["type"], "event type")?;
        if count(&row["seq"], "event seq")? != self.seq {
            return Err("V4 event sequence is not dense".into());
        }
        if !row["time"]
            .as_i64()
            .is_some_and(|time| time.unsigned_abs() <= MAX_SAFE_INTEGER)
            || !value.contains_key("data")
            || value.keys().any(|key| {
                ![
                    "type",
                    "seq",
                    "time",
                    "data",
                    "ignorable",
                    "surfaceOp",
                    "sourceEventSeqs",
                ]
                .contains(&key.as_str())
            })
            || value.get("ignorable").is_some_and(|v| v != true)
        {
            return Err("invalid V4 event envelope".into());
        }
        let known = self.vocabulary.contains(kind);
        if !known && row["ignorable"] != true {
            return Err(format!("unknown required V4 event {kind}"));
        }
        if known {
            if !SURFACE.contains(&kind)
                && (row.get("surfaceOp").is_some() || row.get("sourceEventSeqs").is_some())
            {
                return Err("surface metadata on a non-surface event".into());
            }
            if let Some(refs) = row.get("sourceEventSeqs") {
                let refs = array(refs, "sourceEventSeqs")?;
                let mut seen = HashSet::new();
                if refs.is_empty() && kind != "assistant/message" {
                    return Err("empty sourceEventSeqs on a non-assistant message".into());
                }
                for value in refs {
                    if !seen.insert(earlier(value, self.seq, "sourceEventSeqs")?) {
                        return Err("duplicate source event reference".into());
                    }
                }
            }
        }
        Ok((kind, known))
    }

    fn fold_surface(&mut self, row: &Value, kind: &str) -> Result<(), String> {
        if !SURFACE.contains(&kind) {
            return Ok(());
        }
        if kind == "system/message" && !self.surface.is_empty() && self.protected.is_none() {
            return Err("system message requires a protected first surface head".into());
        }
        if row["surfaceOp"] == "append" {
            if kind == "system/message" && self.surface.is_empty() {
                self.protected = Some(self.seq);
            }
            self.surface.push(self.seq);
            return Ok(());
        }
        let op = object(&row["surfaceOp"], "surfaceOp")?;
        if op.len() != 3 || op.get("op").and_then(Value::as_str) != Some("replace") {
            return Err("invalid V4 surface replacement".into());
        }
        let start = earlier(&row["surfaceOp"]["startSeq"], self.seq, "replacement start")?;
        let end = earlier(&row["surfaceOp"]["endSeq"], self.seq, "replacement end")?;
        let first = self
            .surface
            .iter()
            .position(|s| *s == start)
            .ok_or("replacement start is not on current surface")?;
        let last = self
            .surface
            .iter()
            .position(|s| *s == end)
            .filter(|last| *last >= first)
            .ok_or("replacement end is not on current surface")?;
        let refs = array(&row["sourceEventSeqs"], "replacement sources")?
            .iter()
            .map(|v| count(v, "replacement source"))
            .collect::<Result<HashSet<_>, _>>()?;
        if self.surface[first..=last].iter().any(|s| !refs.contains(s)) {
            return Err("replacement omits a shadowed surface node".into());
        }
        if self
            .protected
            .is_some_and(|head| self.surface[first..=last].contains(&head))
        {
            if kind != "system/message" || first != last {
                return Err("replacement shadows the protected system head".into());
            }
            self.protected = Some(self.seq);
        }
        self.surface.splice(first..=last, [self.seq]);
        Ok(())
    }

    fn tool(&mut self, row: &Value, kind: &str, data: &Value) -> Result<(), String> {
        if kind == "tool/result" && row["surfaceOp"] != "append" {
            return self.require_turn(kind);
        }
        self.require_step(kind, data)?;
        if kind == "assistant/message" {
            for block in array(&data["message"]["content"], "assistant content")? {
                if block["type"] != "tool-call" {
                    continue;
                }
                let id = text(&block["id"], "tool id")?;
                if self.tools.contains_key(id) {
                    return Err(format!("assistant repeats advertised tool call {id}"));
                }
                self.tools.insert(
                    id.into(),
                    PendingTool {
                        name: text(&block["name"], "tool name")?.into(),
                        arguments: identity(&block["arguments"]),
                        started: false,
                    },
                );
            }
            return Ok(());
        }
        let message = &data["message"];
        let id = text(
            if kind == "tool/call" {
                &data["callId"]
            } else {
                &message["toolCallId"]
            },
            "tool call id",
        )?;
        let pending = self
            .tools
            .get_mut(id)
            .ok_or_else(|| format!("{kind} {id} has no advertised lifecycle"))?;
        if kind == "tool/call" {
            if pending.started
                || data["name"] != pending.name
                || identity(&data["arguments"]) != pending.arguments
            {
                return Err("tool/call does not match its advertised call".into());
            }
            pending.started = true;
        } else {
            if !pending.started && !Self::not_started(row, data, message, id)? {
                return Err("unstarted result is not an exact TOOL_NOT_STARTED repair".into());
            }
            self.tools.remove(id);
        }
        Ok(())
    }
    fn not_started(row: &Value, data: &Value, message: &Value, id: &str) -> Result<bool, String> {
        object(&data["error"], "not-started error")?;
        if data["error"]["name"] != "ToolNotStartedError"
            || data["error"]["code"] != "TOOL_NOT_STARTED"
            || message["isError"] != true
            || row.get("sourceEventSeqs").is_some()
        {
            return Ok(false);
        }
        let message_id = text(&message["id"], "not-started message id")?;
        if message_id.starts_with(&format!("forked-tool-result-{id}-")) {
            return Ok(true);
        }
        let valid_suffix = message_id
            .strip_prefix(&format!("interrupted-tool-result-{id}-"))
            .and_then(canonical_suffix)
            .is_some();
        let content = array(&message["content"], "not-started content")?;
        Ok(valid_suffix
            && content.len() == 1
            && content[0]["type"] == "text"
            && content[0]["text"] == REPAIR_TEXT)
    }

    fn dispatch(&mut self, kind: &str, data: &Value) -> Result<(), String> {
        self.require_turn(kind)?;
        let id = text(&data["subCallId"], "PTC subCallId")?;
        let root = text(&data["rootCallId"], "PTC rootCallId")?;
        let parent = text(&data["parentCallId"], "PTC parentCallId")?;
        if parent != root && self.dispatches.get(parent).is_none_or(|d| d.root != root) {
            return Err("PTC parent does not belong to its root".into());
        }
        let name = optional_identity(data.get("name"));
        let arguments = optional_identity(data.get("arguments"));
        if kind == "tool/ptc-dispatch-start" {
            if self.dispatches.contains_key(id) {
                return Err("PTC dispatch repeats subCallId".into());
            }
            self.dispatches.insert(
                id.into(),
                Dispatch {
                    root: root.into(),
                    parent: parent.into(),
                    name,
                    arguments,
                    settled: false,
                },
            );
        } else {
            let old = self
                .dispatches
                .get_mut(id)
                .filter(|d| !d.settled)
                .ok_or("PTC dispatch has no unique start")?;
            if old.root != root
                || old.parent != parent
                || old.name != name
                || old.arguments != arguments
            {
                return Err("PTC result does not match its start".into());
            }
            old.settled = true;
        }
        Ok(())
    }

    fn retry(&mut self, kind: &str, data: &Value) -> Result<(), String> {
        let id = text(&data["retryId"], "retryId")?.to_string();
        let attempt = count(&data["retry"], "retry")?;
        let coordinates = (
            count(&data["turn"], "retry turn")?,
            count(&data["step"], "retry step")?,
        );
        let key = (id.clone(), attempt);
        if kind == "llm/retry-started" {
            if self.scheduled.get(&key) != Some(&coordinates) || !self.started_retries.insert(key) {
                return Err("retry-started has no unique matching scheduled attempt".into());
            }
            return Ok(());
        }
        if self.turn != Some(coordinates.0)
            || coordinates.1 != self.step.unwrap_or(self.next_step - 1)
        {
            return Err("retry does not match current turn and step".into());
        }
        let provider = text(&data["provider"], "retry provider")?;
        let policy = text(&data["policyKey"], "retry policyKey")?;
        if self.provider.as_deref() != Some(provider) {
            return Err("retry provider does not match request header".into());
        }
        let chain = identity(&serde_json::json!([
            coordinates.0,
            coordinates.1,
            provider,
            policy
        ]));
        if let Some((prior, owner)) = self.retry_chains.get(&chain) {
            if attempt != prior + 1 || owner != &id {
                return Err("retry skips its policy sequence or changes retryId".into());
            }
        } else if attempt != 1 || self.retry_ids.contains_key(&id) {
            return Err("retry must start a unique policy chain at one".into());
        }
        self.retry_ids.insert(id.clone(), chain);
        self.retry_chains.insert(chain, (attempt, id));
        self.scheduled.insert(key, coordinates);
        Ok(())
    }

    fn span(&self, data: &Value) -> Result<(), String> {
        let start = earlier(
            &data["shadowedRange"]["start"],
            self.seq,
            "compaction range start",
        )?;
        let end = earlier(
            &data["shadowedRange"]["end"],
            self.seq,
            "compaction range end",
        )?;
        let first = self
            .surface
            .iter()
            .position(|s| *s == start)
            .ok_or("compaction starts outside current surface")?;
        let last = self
            .surface
            .iter()
            .position(|s| *s == end)
            .filter(|i| *i >= first)
            .ok_or("compaction ends outside current surface")?;
        let seqs = array(&data["shadowedSeqs"], "shadowedSeqs")?;
        if seqs.len() != last - first + 1
            || seqs
                .iter()
                .zip(&self.surface[first..=last])
                .any(|(value, seq)| count(value, "shadowed seq").ok() != Some(*seq))
        {
            return Err("compaction does not name an exact current surface span".into());
        }
        if self
            .protected
            .is_some_and(|head| self.surface[first..=last].contains(&head))
        {
            return Err("compaction shadows the protected system head".into());
        }
        Ok(())
    }
    fn compaction_owner(&self, data: &Value) -> Result<&Compaction, String> {
        self.compaction
            .as_ref()
            .filter(|c| {
                data["compactionId"] == c.id && data.get("sourceCommandId") == c.command.as_ref()
            })
            .ok_or_else(|| "compaction record has no matching start and command owner".into())
    }
    fn compact(&mut self, kind: &str, data: &Value) -> Result<(), String> {
        if matches!(kind, "compaction/summary" | "compaction/prune") {
            self.span(data)?;
        }
        if kind == "compaction/prune" {
            return Ok(());
        }
        if kind == "compaction/start" {
            if self.compaction.is_some() {
                return Err("compaction start overlaps an open compaction".into());
            }
            let owner = data
                .get("turn")
                .ok_or("compaction start requires an explicit owner turn")?;
            let turn = if owner.is_null() {
                None
            } else {
                Some(count(owner, "compaction turn")?)
            };
            if turn != self.turn {
                return Err("compaction start does not match open turn".into());
            }
            self.compaction = Some(Compaction {
                id: text(&data["compactionId"], "compactionId")?.into(),
                command: data.get("sourceCommandId").cloned(),
                turn,
                summarized: false,
                crossed_turn: false,
            });
            return Ok(());
        }
        let current = self.compaction_owner(data)?;
        if current.turn != self.turn {
            return Err("compaction does not match open turn".into());
        }
        if kind == "compaction/summary" {
            if current.summarized {
                return Err("compaction repeats its summary".into());
            }
            self.compaction.as_mut().unwrap().summarized = true;
        } else {
            let owner = data
                .get("turn")
                .ok_or("compaction end requires an explicit owner turn")?;
            let turn = if owner.is_null() {
                None
            } else {
                Some(count(owner, "compaction turn")?)
            };
            if turn != current.turn {
                return Err("compaction end changes owner turn".into());
            }
            if current.crossed_turn {
                return Err("compaction crossed a turn boundary before settling".into());
            }
            if data.get("error").is_none() && !current.summarized {
                return Err("successful compaction end requires one summary".into());
            }
            self.compaction = None;
        }
        Ok(())
    }

    fn index_header(&mut self, data: &Value) -> Result<(), String> {
        let header = &data["header"];
        object(header, "request header")?;
        object(&header["config"], "request config")?;
        self.provider = Some(text(&header["config"]["provider"], "request provider")?.into());
        let mut definitions = ToolIndex::new();
        if let Some(tools) = header.get("tools") {
            for tool in array(tools, "request tools")? {
                let name = text(&tool["name"], "tool definition name")?;
                let complete = tool["description"].is_string() && tool["parameters"].is_object();
                definitions
                    .entry(name.into())
                    .and_modify(|d| {
                        d.count += 1;
                        d.complete &= complete;
                    })
                    .or_insert(ToolShape { count: 1, complete });
            }
        }
        let shared = if let Some(index) = self
            .header_shapes
            .iter()
            .find(|i| i.as_ref() == &definitions)
        {
            index.clone()
        } else {
            let index = Arc::new(definitions);
            self.header_shapes.push(index.clone());
            index
        };
        self.headers.insert(self.seq, shared);
        Ok(())
    }
    fn developer(&self, data: &Value) -> Result<(), String> {
        let Some(reference) = data.get("headerSeq") else {
            return Ok(());
        };
        let seq = earlier(reference, self.seq, "developer headerSeq")?;
        if self.source_kind(seq)? != ReferenceKind::Header {
            return Err("developer headerSeq does not reference a request header".into());
        }
        let header = self
            .headers
            .get(&seq)
            .ok_or("missing developer header index")?;
        for block in array(&data["message"]["content"], "developer content")? {
            if block["type"] == "tool-addition" {
                let name = text(&block["toolName"], "toolName")?;
                if header
                    .get(name)
                    .is_none_or(|definition| definition.count != 1 || !definition.complete)
                {
                    return Err(format!(
                        "tool-addition {name} does not name one complete definition in headerSeq {seq}"
                    ));
                }
            }
        }
        Ok(())
    }
    fn title(&self, kind: &str, data: &Value) -> Result<(), String> {
        let refs = array(&data["messageSeqs"], "title messageSeqs")?;
        if kind == "session/title" && (refs.is_empty() != (data["source"]["kind"] == "user")) {
            return Err("title references must be empty exactly for a user title".into());
        }
        let mut seen = HashSet::new();
        for value in refs {
            let seq = earlier(value, self.seq, "title source")?;
            if !seen.insert(seq) || self.source_kind(seq)? != ReferenceKind::HumanUser {
                return Err("title must reference distinct earlier human user messages".into());
            }
        }
        if kind == "session/title-llm-request" {
            let messages = array(&data["messages"], "title messages")?;
            if refs.is_empty()
                || messages.len() != 1
                || messages[0]["role"] != "user"
                || messages[0]["source"]["kind"] != "dsh-session-title-llm"
            {
                return Err("title request messages do not represent their references".into());
            }
            let content = array(&messages[0]["content"], "title content")?;
            if content.len() != 1 || content[0]["type"] != "text" {
                return Err("title request requires one text block".into());
            }
        }
        Ok(())
    }

    fn accept(&mut self, row: &Value) -> Result<(), String> {
        let (kind, known) = self.envelope(row)?;
        let kind = kind.to_owned();
        if !known {
            self.references.push(ReferenceKind::Other);
            self.seq += 1;
            return Ok(());
        }
        validate_v4_row_fields(row)?;
        let data = &row["data"];
        if SURFACE.contains(&kind.as_str())
            || matches!(
                kind.as_str(),
                "turn/start"
                    | "turn/end"
                    | "step/start"
                    | "step/end"
                    | "tool/call"
                    | "assistant/attempt"
                    | "request/header"
                    | "request/context"
                    | "tool/ptc-dispatch-start"
                    | "tool/ptc-dispatch"
                    | "llm/retry"
                    | "llm/retry-started"
                    | "session/title"
                    | "session/title-llm-request"
                    | "command/run"
                    | "command/done"
                    | "compaction/start"
                    | "compaction/summary"
                    | "compaction/end"
                    | "compaction/prune"
                    | "session/end-seed"
                    | "session-log-deepseek/delivery-accepted"
            )
        {
            object(data, &kind)?;
        }
        if matches!(
            kind.as_str(),
            "system/message" | "developer/message" | "assistant/attempt"
        ) {
            self.require_step(&kind, data)?;
        }
        if kind.starts_with("turn/") {
            if let Some(compaction) = self.compaction.as_mut() {
                compaction.crossed_turn = true;
            }
        }
        self.fold_surface(row, &kind)?;
        let mut reference = ReferenceKind::Other;
        match kind.as_str() {
            "turn/start" => {
                if self.turn.is_some() || count(&data["turn"], "turn")? != self.next_turn {
                    return Err("turn/start does not open the expected turn".into());
                }
                self.turn = Some(self.next_turn);
                self.next_step = 1;
                self.tools.clear();
            }
            "turn/end" => {
                if self.turn != Some(count(&data["turn"], "turn")?) || self.step.is_some() {
                    return Err("turn/end does not match open turn without an open step".into());
                }
                self.close_tools(&kind)?;
                self.turn = None;
                self.next_turn += 1;
            }
            "step/start" => {
                if self.turn != Some(count(&data["turn"], "turn")?)
                    || self.step.is_some()
                    || count(&data["step"], "step")? != self.next_step
                {
                    return Err("step/start does not match open turn and next step".into());
                }
                self.step = Some(self.next_step);
            }
            "step/end" => {
                self.require_step(&kind, data)?;
                self.close_tools(&kind)?;
                self.step = None;
                self.next_step += 1;
            }
            "assistant/message" | "tool/call" | "tool/result" => self.tool(row, &kind, data)?,
            "developer/message" => self.developer(data)?,
            "request/header" => {
                self.require_turn(&kind)?;
                self.index_header(data)?;
                reference = ReferenceKind::Header;
            }
            "request/context" => self.require_turn(&kind)?,
            "tool/ptc-dispatch-start" | "tool/ptc-dispatch" => self.dispatch(&kind, data)?,
            "llm/retry" | "llm/retry-started" => self.retry(&kind, data)?,
            "session/title" | "session/title-llm-request" => self.title(&kind, data)?,
            "command/run" => {
                let id = text(&data["commandId"], "commandId")?;
                if !self.commands.insert(id.into()) {
                    return Err("command/run repeats commandId".into());
                }
                reference = ReferenceKind::Command;
            }
            "command/done" => {
                if !self
                    .commands
                    .contains(text(&data["commandId"], "commandId")?)
                {
                    return Err("command/done has no prior command/run".into());
                }
                if let Some(source) = data.get("sourceEventSeq") {
                    let source = earlier(source, self.seq, "command sourceEventSeq")?;
                    if data["kind"] != "success"
                        || self.source_kind(source)? == ReferenceKind::Command
                    {
                        return Err("invalid command/done sourceEventSeq".into());
                    }
                }
                reference = ReferenceKind::Command;
            }
            "compaction/start" | "compaction/summary" | "compaction/end" | "compaction/prune" => {
                self.compact(&kind, data)?
            }
            "user/message" => {
                if data["source"]["kind"] == "user" {
                    reference = ReferenceKind::HumanUser;
                }
                if row["surfaceOp"] != "append" && data["source"]["kind"] == "compact-checkpoint" {
                    self.compaction_owner(&data["source"])?;
                }
            }
            "session/end-seed" => {
                // An own initialization marker has an empty payload; only
                // inherited boundaries carry the optional literal true flag.
                if data.get("inherited").is_some_and(|value| value != true) {
                    return Err("end-seed inherited must be true when present".into());
                }
                if data["inherited"] == true {
                    self.last_marker = Some(self.seq);
                }
                self.compaction = None;
            }
            "subagent/catalog" if self.seq >= self.cut => {
                catalog::validate_catalog(data)?;
                if !self
                    .catalogs
                    .insert(data["childId"].as_str().unwrap().into())
                {
                    return Err("duplicate own catalog child".into());
                }
            }
            "session-log-deepseek/delivery-accepted" => {
                let version = data
                    .get("sessionFormatVersion")
                    .map(|v| count(v, "delivery generation"))
                    .transpose()?
                    .unwrap_or(0);
                if version == 4 {
                    earlier(&data["throughSeq"], self.seq, "delivery throughSeq")?;
                    let id = text(&data["sessionId"], "delivery Session id")?;
                    if !(self.header.get("parentSession").is_some() && self.seq < self.cut)
                        && self.header["id"] != id
                    {
                        return Err("current delivery names another Session".into());
                    }
                }
            }
            _ => {}
        }
        self.references.push(reference);
        self.seq += 1;
        Ok(())
    }
}
