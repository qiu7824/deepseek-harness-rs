use super::{
    catalog, convert_v3_event_messages, references,
    wire::{count, object, validate_header},
};
use crate::{SessionEvent, SessionSeq};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Source authority is selected by the storage/import boundary. It must not be
/// inferred from untrusted tool arguments or silently changed after a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V3Dialect {
    Released,
    Rust,
}

// These historical lists deliberately do not read the current writer vocabulary.
const RELEASED: &[&str] = &[
    "agent-preset/selected",
    "agent/inbox/spliced",
    "approval/asked",
    "approval/decided",
    "approval/policy",
    "assistant/attempt",
    "assistant/message",
    "command/done",
    "command/run",
    "compaction/end",
    "compaction/prune",
    "compaction/start",
    "compaction/summary",
    "deliverables/presented",
    "feedback/message-delete",
    "feedback/message-put",
    "feedback/record",
    "goal/change",
    "hook/invoked",
    "hook/result",
    "image/offload",
    "llm/retry",
    "llm/retry-started",
    "model/selection",
    "permission/preset",
    "plan/mode",
    "request/context",
    "request/header",
    "sandbox/mode",
    "schedule/change",
    "session-log-deepseek/delivery-accepted",
    "session/end-seed",
    "session/title",
    "session/title-llm-request",
    "step/end",
    "step/start",
    "subagent/catalog",
    "subagent/descriptor",
    "subagent/model-selection-policy",
    "system/message",
    "team/member",
    "team/message/delivered",
    "team/message/queued",
    "team/task",
    "todo/write",
    "tool-workflow/agent-end",
    "tool-workflow/agent-start",
    "tool-workflow/run-end",
    "tool-workflow/run-start",
    "tool/call",
    "tool/ptc-dispatch",
    "tool/ptc-dispatch-start",
    "tool/result",
    "turn/end",
    "turn/start",
    "user/message",
    "web/deepseek-search-llm-request",
    "workspace/changes",
];
const RUST_ADDITIONS: &[&str] = &[
    "assistant/chunk",
    "compaction/error",
    "compaction/recovery",
    "compaction/retry",
    "computer-use/activity",
    "execution/ultra-child",
    "execution/ultra-admitted",
    "execution/ultra-settled",
    "execution/ultra-budget-exhausted",
    "request/phase",
    "sandbox/roots-revoked",
    "team/config",
    "team/message/cancelled",
    "terminal/permissions-revoked",
    "tool/code-dispatch",
    "tool/code-dispatch-start",
    "tools/discovery",
    "web/hosted-search-request",
];

pub(super) fn known_v4_event(kind: &str) -> bool {
    matches!(kind, "developer/message" | "permission/options")
        || RELEASED.contains(&kind)
        || RUST_ADDITIONS.contains(&kind)
            && !["tool/code-dispatch", "tool/code-dispatch-start"].contains(&kind)
}

#[derive(Debug, Clone, PartialEq)]
pub struct V4TransformSummary {
    pub header: Value,
    pub inherited_event_count: u64,
    /// Target position of each original event, excluding appended catalog facts.
    pub source_offsets: Vec<u64>,
    /// Boundary after each source prefix; inserted restart ends belong to the
    /// following source prefix, and appended own catalog facts stay outside EOF.
    pub source_cuts: Vec<u64>,
    pub event_count: u64,
}

/// Streaming adjacent conversion. Returned rows must remain private until
/// `finish` and the complete V4 relationship validator both succeed.
pub struct V3ToV4Transform {
    header: Value,
    dialect: V3Dialect,
    children: Vec<Value>,
    catalogs: Vec<Value>,
    mapping: Vec<u64>,
    cuts: Vec<u64>,
    next_seq: u64,
    time: i64,
    turn: Option<u64>,
    step_open: bool,
    next_turn_spliced: bool,
    source_cut: Option<u64>,
    cut: Option<u64>,
    expected_cut: Option<u64>,
    foreign_delivery: Option<u64>,
    failed: bool,
}

impl V3ToV4Transform {
    pub fn new(
        mut header: Value,
        children: Option<Vec<Value>>,
        inherited: Option<u64>,
        dialect: V3Dialect,
    ) -> Result<Self, String> {
        object(&header, "Session header")?;
        if dialect == V3Dialect::Rust && header.get("delegationDepth").is_none() {
            header["delegationDepth"] = json!(0);
        }
        validate_header(&header, 3)?;
        let mut children = children.ok_or(
            "V3 migration requires explicit historical child facts, including an empty list",
        )?;
        for child in &children {
            catalog::validate_source(child)?;
        }
        children.sort_by(|a, b| {
            count(&a["childCreatedAt"], "childCreatedAt")
                .unwrap()
                .cmp(&count(&b["childCreatedAt"], "childCreatedAt").unwrap())
                .then_with(|| {
                    a["childId"]
                        .as_str()
                        .unwrap()
                        .encode_utf16()
                        .cmp(b["childId"].as_str().unwrap().encode_utf16())
                })
        });
        let cut = if header["isSeeded"] == true {
            None
        } else {
            Some(0)
        };
        let time = count(&header["createdAt"], "createdAt")? as i64;
        Ok(Self {
            header,
            dialect,
            children,
            catalogs: vec![],
            mapping: vec![],
            cuts: vec![0],
            next_seq: 0,
            time,
            turn: None,
            step_open: false,
            next_turn_spliced: false,
            source_cut: cut,
            cut,
            expected_cut: inherited,
            foreign_delivery: None,
            failed: false,
        })
    }

    pub fn push(&mut self, row: Value) -> Result<Vec<Value>, String> {
        if self.failed {
            return Err("migration already failed; discard its staged output".into());
        }
        let result = self.push_inner(row);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn push_inner(&mut self, mut row: Value) -> Result<Vec<Value>, String> {
        let source_seq = count(&row["seq"], "event seq")?;
        if source_seq != self.mapping.len() as u64 {
            return Err("V3 source events must be dense".into());
        }
        let kind = row["type"]
            .as_str()
            .ok_or("event type must be a string")?
            .to_string();
        let time = row["time"]
            .as_i64()
            .ok_or("event time must be an integer")?;
        let envelope = object(&row, "event")?;
        if !envelope.contains_key("data")
            || envelope.keys().any(|k| {
                ![
                    "type",
                    "seq",
                    "time",
                    "data",
                    "ignorable",
                    "surfaceOp",
                    "sourceEventSeqs",
                ]
                .contains(&k.as_str())
            })
            || envelope.get("ignorable").is_some_and(|v| v != true)
        {
            return Err("invalid V3 event envelope".into());
        }
        let known = RELEASED.contains(&kind.as_str())
            || self.dialect == V3Dialect::Rust && RUST_ADDITIONS.contains(&kind.as_str());
        if !known && row["ignorable"] != true {
            return Err(format!(
                "unknown required V3 event {kind}; original generation must be retained"
            ));
        }
        let mut out = Vec::with_capacity(2);
        if known
            && kind == "turn/start"
            && self.turn.is_some()
            && !self.step_open
            && self.next_turn_spliced
            && count(&row["data"]["turn"], "turn")? == self.turn.unwrap() + 1
        {
            out.push(json!({"type":"turn/end","seq":self.next_seq,"time":time,"data":{"turn":self.turn,"reason":{"kind":"interrupted"}}}));
            self.next_seq += 1;
        }
        let target_seq = self.next_seq;
        self.next_seq += 1;
        self.time = time;
        self.next_turn_spliced = known
            && kind == "agent/inbox/spliced"
            && row["data"]["target"] == "next-turn"
            && row["data"]["inserted"]
                .as_array()
                .is_some_and(|a| !a.is_empty());
        if known {
            // Rust stored the inherited cut in its physical header and left
            // end-seed payloads empty, including later restore markers.
            if self.dialect == V3Dialect::Rust && kind == "session/end-seed"
                && self.header["isSeeded"] == true && self.expected_cut == Some(source_seq)
                && row["data"].as_object().is_some_and(|data| data.is_empty()) {
                row["data"]["inherited"] = json!(true);
            }
            match kind.as_str() {
                "turn/start" => self.turn = Some(count(&row["data"]["turn"], "turn")?),
                "turn/end" => self.turn = None,
                "step/start" => self.step_open = true,
                "step/end" => self.step_open = false,
                "session/end-seed" if row["data"]["inherited"] == true => {
                    if self.header["isSeeded"] != true {
                        return Err("unseeded V3 Session has an inherited marker".into());
                    }
                    self.source_cut = Some(source_seq);
                    self.cut = Some(target_seq);
                    self.catalogs.clear();
                }
                "subagent/catalog" => self.catalogs.push(row["data"].clone()),
                "session-log-deepseek/delivery-accepted" => self.delivery(&row)?,
                _ => {}
            }
        }
        if !known {
            row["type"] = json!(format!("plugin:{kind}"));
            row["seq"] = json!(target_seq);
        } else {
            references::remap(&mut row, target_seq, &self.mapping, self.dialect)?;
            let target_kind = match (self.dialect, kind.as_str()) {
                (V3Dialect::Rust, "tool/code-dispatch") => "tool/ptc-dispatch",
                (V3Dialect::Rust, "tool/code-dispatch-start") => "tool/ptc-dispatch-start",
                _ => kind.as_str(),
            };
            let data = row.as_object_mut().unwrap().remove("data").unwrap();
            let converted = convert_v3_event_messages(SessionEvent {
                type_: target_kind.into(),
                seq: SessionSeq::new(target_seq)?,
                time,
                data,
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            })?;
            row["type"] = json!(target_kind);
            row["data"] = converted.data;
            if self.dialect == V3Dialect::Rust {
                normalize_rust_compaction_owner(&mut row);
            }
        }
        self.mapping.push(target_seq);
        self.cuts.push(self.next_seq);
        out.push(row);
        Ok(out)
    }

    fn delivery(&mut self, row: &Value) -> Result<(), String> {
        object(&row["data"], "delivery-accepted data")?;
        let version = row["data"]
            .get("sessionFormatVersion")
            .map(|v| count(v, "delivery generation"))
            .transpose()?
            .unwrap_or(0);
        if version == 4 {
            return Err("V3 delivery marker claims target V4 generation".into());
        }
        if version == 3 {
            if count(&row["data"]["throughSeq"], "delivery throughSeq")?
                >= count(&row["seq"], "delivery seq")?
            {
                return Err("delivery throughSeq must precede its marker".into());
            }
            let id = row["data"]["sessionId"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("active delivery requires Session identity")?;
            if id != self.header["id"].as_str().unwrap() {
                self.foreign_delivery = Some(count(&row["seq"], "delivery seq")?);
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(V4TransformSummary, Vec<Value>), String> {
        if self.failed {
            return Err("migration failed; no successor may be published".into());
        }
        let source_cut = self
            .source_cut
            .ok_or("seeded V3 Session lacks its inherited marker")?;
        let cut = self.cut.ok_or("missing inherited cut")?;
        if self.expected_cut.is_some_and(|value| value != source_cut) {
            return Err("source inherited cut disagrees with its marker".into());
        }
        if self
            .foreign_delivery
            .is_some_and(|seq| self.header.get("parentSession").is_none() || seq >= source_cut)
        {
            return Err("current-generation delivery marker names the wrong Session".into());
        }
        let mut existing = BTreeMap::new();
        for fact in self.catalogs {
            catalog::validate_catalog(&fact)?;
            if existing
                .insert(fact["childId"].as_str().unwrap().to_string(), fact)
                .is_some()
            {
                return Err("duplicate own catalog child".into());
            }
        }
        let mut output = Vec::new();
        for source in self.children {
            let fact = catalog::child_fact(&source)?;
            let id = source["childId"].as_str().unwrap();
            if let Some(old) = existing.get(id) {
                if count(&old["childCreatedAt"], "childCreatedAt")?
                    != count(&source["childCreatedAt"], "childCreatedAt")?
                    || fact.as_ref().is_some_and(|fact| {
                        old["mode"] != fact["mode"] || old.get("label") != fact.get("label")
                    })
                {
                    return Err(format!(
                        "historical child {id} conflicts with parent catalog"
                    ));
                }
                continue;
            }
            let fact = fact.unwrap_or_else(||json!({"version":1,"childId":id,"childCreatedAt":source["childCreatedAt"],"mode":"unknown"}));
            existing.insert(id.to_owned(), fact.clone());
            output.push(
                json!({"type":"subagent/catalog","seq":self.next_seq,"time":self.time,"data":fact}),
            );
            self.next_seq += 1;
        }
        self.header["version"] = json!(4);
        validate_header(&self.header, 4)?;
        Ok((
            V4TransformSummary {
                header: self.header,
                inherited_event_count: cut,
                source_offsets: self.mapping,
                source_cuts: self.cuts,
                event_count: self.next_seq,
            },
            output,
        ))
    }
}

fn preserve_null_command(owner: &mut serde_json::Map<String, Value>) {
    if owner.get("sourceCommandId") != Some(&Value::Null) {
        return;
    }
    let mut key = "plugin:rust-v3:sourceCommandId".to_string();
    while owner.contains_key(&key) {
        key = format!("plugin:rust-v3:{key}");
    }
    let original = owner.remove("sourceCommandId").unwrap();
    owner.insert(key, original);
}

// Rust V3 emitted JSON null for automatic-compaction lifecycle ownership but
// omitted that optional field on its checkpoint source. Preserve the original
// null as extension data while making the known no-command representation agree.
fn normalize_rust_compaction_owner(row: &mut Value) {
    if matches!(
        row["type"].as_str(),
        Some("compaction/start" | "compaction/summary" | "compaction/end")
    ) {
        if let Some(data) = row["data"].as_object_mut() {
            preserve_null_command(data);
        }
    }
    if row["type"] == "user/message" && row["data"]["source"]["kind"] == "compact-checkpoint" {
        if let Some(source) = row["data"]["source"].as_object_mut() {
            preserve_null_command(source);
        }
    }
}
