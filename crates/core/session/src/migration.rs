//! Ordered migration of the historical Rust session envelope to V3.
//! Source bytes belong to persistence; this pure conversion publishes nothing.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    LEGACY_SESSION_FORMAT_VERSION, SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionSeq,
    SurfaceOp,
};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionMigrationReport {
    pub from_version: u64,
    pub to_version: u64,
    pub inserted_system_event: bool,
    pub events: Vec<SessionEvent>,
    pub header: SessionHeader,
    /// Target position of every original event, followed by the target EOF.
    pub source_offsets: Vec<usize>,
    /// Target prefix length after each source prefix. Insertions anchored at
    /// the next event belong to the next prefix, not to the committed frame.
    pub source_cuts: Vec<usize>,
}

fn remap(value: &mut Value, offsets: &[usize], field: &str) -> Result<(), String> {
    let source = value
        .as_u64()
        .ok_or_else(|| format!("invalid local sequence at {field}"))?;
    let target = offsets
        .get(usize::try_from(source).map_err(|_| "sequence overflow")?)
        .ok_or_else(|| format!("{field} must reference an earlier event: {source}"))?;
    *value = json!(target);
    Ok(())
}

fn remap_array(value: &mut Value, offsets: &[usize], field: &str) -> Result<(), String> {
    for value in value
        .as_array_mut()
        .ok_or_else(|| format!("invalid sequence array at {field}"))?
    {
        remap(value, offsets, field)?;
    }
    Ok(())
}

fn remap_event(event: &mut SessionEvent, offsets: &[usize]) -> Result<(), String> {
    let one = |seq: &mut u64| -> Result<(), String> {
        *seq = *offsets
            .get(usize::try_from(*seq).map_err(|_| "sequence overflow")?)
            .ok_or("envelope reference must point to an earlier event")? as u64;
        Ok(())
    };
    if let Some(seqs) = &mut event.source_event_seqs {
        for seq in seqs {
            one(seq)?;
        }
    }
    if let Some(SurfaceOp::Replace { start, end }) = &mut event.surface_op {
        one(start)?;
        one(end)?;
    }
    match event.type_.as_str() {
        "command/done" => {
            if let Some(seq) = event.data.get_mut("sourceEventSeq") {
                remap(seq, offsets, "command/done.sourceEventSeq")?;
            }
        }
        "compaction/summary" | "compaction/prune" => {
            if let Some(range) = event.data.get_mut("shadowedRange") {
                for key in ["start", "end"] {
                    remap(
                        range
                            .get_mut(key)
                            .ok_or("invalid compaction shadowedRange")?,
                        offsets,
                        key,
                    )?;
                }
            }
            if let Some(seqs) = event.data.get_mut("shadowedSeqs") {
                remap_array(seqs, offsets, "shadowedSeqs")?;
            }
        }
        "session/title" | "session/title-llm-request" => {
            if let Some(seqs) = event.data.get_mut("messageSeqs") {
                remap_array(seqs, offsets, "messageSeqs")?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn system_event(
    header: &SessionHeader,
    anchor: &SessionEvent,
    target: usize,
    previous: Option<u64>,
    prompt: &str,
    turn: u64,
    step: u64,
) -> Result<SessionEvent, String> {
    let identity = serde_json::to_vec(&json!([
        "rust-v0-to-v3",
        header.id,
        anchor.seq,
        anchor.type_
    ]))
    .map_err(|error| error.to_string())?;
    let id = format!("v0-to-v3-system-{:x}", Sha256::digest(identity));
    let content = if prompt.is_empty() {
        json!([])
    } else {
        json!([{"type":"text", "text":prompt}])
    };
    Ok(SessionEvent {
        type_: "system/message".into(),
        seq: SessionSeq::new(target as u64)?,
        time: anchor.time,
        data: json!({"turn":turn,"step":step,"prefix":previous.is_none(),"message":{"id":id,"role":"system","content":content,"source":{"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt"}}}),
        ignorable: None,
        surface_op: Some(
            previous.map_or(SurfaceOp::Append, |seq| SurfaceOp::Replace {
                start: seq,
                end: seq,
            }),
        ),
        source_event_seqs: previous.map(|seq| vec![seq]),
    })
}

/// Preserve each historical request's exact prompt, including explicit clearing.
/// Only audited local references move. Tool arguments and captured foreign
/// session coordinates remain opaque.
pub fn migrate_v0_to_v3(
    mut header: SessionHeader,
    events: &[SessionEvent],
) -> Result<SessionMigrationReport, String> {
    if header.version != LEGACY_SESSION_FORMAT_VERSION {
        return Err(format!(
            "session migration expects V{LEGACY_SESSION_FORMAT_VERSION}, got V{}",
            header.version
        ));
    }
    let mut migrated = Vec::new();
    let mut offsets = Vec::with_capacity(events.len() + 1);
    let mut source_cuts = Vec::with_capacity(events.len() + 1);
    let mut prompt = String::new();
    let mut head = None;
    let (mut turn, mut step) = (0, 0);
    let mut ids = std::collections::HashSet::new();
    for event in events {
        let messages: Vec<&Value> = match event.type_.as_str() {
            "user/message" => vec![&event.data],
            "assistant/message" | "tool/result" | "system/message" => {
                event.data.get("message").into_iter().collect()
            }
            "agent/inbox/spliced" => event
                .data
                .get("inserted")
                .and_then(Value::as_array)
                .map(|a| a.iter().collect())
                .unwrap_or_default(),
            "session/title-llm-request" => event
                .data
                .get("messages")
                .and_then(Value::as_array)
                .map(|a| a.iter().collect())
                .unwrap_or_default(),
            _ => vec![],
        };
        for message in messages {
            if let Some(id) = message.get("id").and_then(Value::as_str) {
                ids.insert(id.to_owned());
            }
        }
    }
    for (index, original) in events.iter().enumerate() {
        source_cuts.push(migrated.len());
        if !crate::is_known_session_event_type(&original.type_) && original.ignorable != Some(true)
        {
            return Err(format!(
                "unsupported required legacy event {} at seq {}; source log must be retained",
                original.type_, original.seq
            ));
        }
        if original.seq.get() != index as u64 {
            return Err(format!(
                "cannot migrate non-contiguous session log: event at index {index} has seq {}",
                original.seq
            ));
        }
        if original.type_ == "system/message" {
            return Err("legacy log already contains a V3 system node".into());
        }
        if original.type_ == "step/start" {
            turn = original
                .data
                .get("turn")
                .and_then(Value::as_u64)
                .ok_or("invalid step/start.turn")?;
            step = original
                .data
                .get("step")
                .and_then(Value::as_u64)
                .ok_or("invalid step/start.step")?;
        }
        let mut event = original.clone();
        remap_event(&mut event, &offsets)?;
        if event.type_ == "request/header" {
            let epoch = event
                .data
                .get_mut("header")
                .and_then(Value::as_object_mut)
                .ok_or("invalid request/header.header")?;
            let next = match epoch.remove("system") {
                None => String::new(),
                Some(Value::String(text)) => text,
                _ => {
                    return Err("request/header.header.system must be a string when present".into());
                }
            };
            if head.is_none() || next != prompt {
                let system =
                    system_event(&header, original, migrated.len(), head, &next, turn, step)?;
                let id = system.data["message"]["id"].as_str().expect("generated id");
                if !ids.insert(id.to_owned()) {
                    return Err("migration system message identity collision".into());
                }
                head = Some(system.seq.get());
                migrated.push(system);
                prompt = next;
            }
        }
        offsets.push(migrated.len());
        event.seq = SessionSeq::new(migrated.len() as u64)?;
        migrated.push(event);
        if original.type_ == "step/start" && head.is_none() {
            let system = system_event(&header, original, migrated.len(), None, "", turn, step)?;
            let id = system.data["message"]["id"].as_str().expect("generated id");
            if !ids.insert(id.to_owned()) {
                return Err("migration system message identity collision".into());
            }
            head = Some(system.seq.get());
            migrated.push(system);
        }
    }
    offsets.push(migrated.len());
    source_cuts.push(migrated.len());
    header.version = SESSION_FORMAT_VERSION;
    Ok(SessionMigrationReport {
        from_version: LEGACY_SESSION_FORMAT_VERSION,
        to_version: SESSION_FORMAT_VERSION,
        inserted_system_event: head.is_some(),
        events: migrated,
        header,
        source_offsets: offsets,
        source_cuts,
    })
}

#[cfg(test)]
#[path = "migration_tests.rs"]
mod migration_tests;
