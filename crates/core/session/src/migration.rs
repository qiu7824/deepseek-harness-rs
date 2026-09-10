//! Explicit migration of the pre-alpha.1 Rust session envelope to V3.
//!
//! V3 makes the effective system prompt part of the ordered history.  The
//! migration is pure so callers can write the returned log to a new artifact
//! and retain the source file until the new artifact has been fsynced.
use serde_json::{Value, json};

use crate::{
    LEGACY_SESSION_FORMAT_VERSION, SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionSeq,
};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionMigrationReport {
    pub from_version: u64,
    pub to_version: u64,
    pub inserted_system_event: bool,
    pub events: Vec<SessionEvent>,
    pub header: SessionHeader,
}

/// Migrate a V0 header and events to V3.  The operation never mutates its
/// inputs and rejects future or already-migrated logs.
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
    // A migrated log must remain a valid append-only sequence.  Persistence
    // scanners normally enforce this already, but the public migration entry
    // point is also callable directly and should never manufacture duplicate
    // sequence numbers from malformed input.
    for (index, event) in events.iter().enumerate() {
        if event.seq.get() != index as u64 {
            return Err(format!(
                "cannot migrate non-contiguous session log: event at index {index} has seq {}",
                event.seq
            ));
        }
    }
    let prompt = events
        .iter()
        .rev()
        .find(|event| event.type_ == "request/header")
        .and_then(|event| event.data.pointer("/header/system"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let inserted_system_event = prompt.is_some();
    let mut migrated = events.to_vec();
    for event in &mut migrated {
        if event.type_ == "request/header"
            && let Some(header) = event.data.get_mut("header").and_then(Value::as_object_mut)
        {
            header.remove("system");
        }
    }
    // The system node is the first model-visible surface node in V3.
    if let Some(system) = prompt {
        let message = dsh_llm::create_message(
            dsh_llm::Role::System,
            vec![dsh_llm::ContentBlock::Text { text: system }],
            dsh_llm::MessageSource::Plugin {
                plugin: "@deepseek-ai/dsh-system-prompt".into(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        );
        migrated.push(SessionEvent {
            type_: "system/message".into(),
            seq: SessionSeq::new(0)?,
            time: events.first().map(|event| event.time).unwrap_or(0),
            data: json!({"turn": 0, "step": 0, "message": message}),
            ignorable: None,
            surface_op: Some(crate::SurfaceOp::Append),
            source_event_seqs: None,
        });
        let system = migrated.pop().expect("system event");
        for event in &mut migrated {
            event.seq = SessionSeq::new(
                event
                    .seq
                    .get()
                    .checked_add(1)
                    .ok_or("session sequence overflow")?,
            )?;
            if let Some(seqs) = event.source_event_seqs.as_mut() {
                for seq in seqs {
                    *seq = seq.checked_add(1).ok_or("source sequence overflow")?;
                }
            }
            if let Some(crate::SurfaceOp::Replace { start, end }) = event.surface_op.as_mut() {
                *start = start.checked_add(1).ok_or("surface sequence overflow")?;
                *end = end.checked_add(1).ok_or("surface sequence overflow")?;
            }
        }
        migrated.insert(0, system);
    }
    header.version = SESSION_FORMAT_VERSION;
    Ok(SessionMigrationReport {
        from_version: LEGACY_SESSION_FORMAT_VERSION,
        to_version: SESSION_FORMAT_VERSION,
        inserted_system_event,
        events: migrated,
        header,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EpochHeader, RequestHeaderReason, request_header_data, session_id};
    use dsh_llm::LlmCallConfig;

    #[test]
    fn v0_prompt_enters_v3_history_without_changing_source_sequences() {
        let header = SessionHeader {
            version: 0,
            id: session_id("s"),
            created_at: 1,
            cwd: None,
            parent_session: None,
            is_seeded: false,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        let event = SessionEvent {
            type_: "request/header".into(),
            seq: SessionSeq::new(0).unwrap(),
            time: 1,
            data: request_header_data(
                &EpochHeader {
                    config: LlmCallConfig::default(),
                    adapter_defaults: None,
                    system: Some("dynamic".into()),
                    tools: None,
                },
                RequestHeaderReason::Initial,
            ),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        };
        let report = migrate_v0_to_v3(header, &[event]).unwrap();
        assert_eq!(report.header.version, 3);
        assert!(report.inserted_system_event);
        assert_eq!(report.events[0].type_, "system/message");
        assert_eq!(report.events[0].seq, 0);
        assert_eq!(report.events[1].seq, 1);
        assert_eq!(report.events[1].source_event_seqs, None);
        assert!(report.events[1].data.pointer("/header/system").is_none());
    }

    #[test]
    fn migration_rejects_non_contiguous_input_instead_of_reusing_a_sequence() {
        let header = SessionHeader {
            version: LEGACY_SESSION_FORMAT_VERSION,
            id: session_id("gap"),
            created_at: 1,
            cwd: None,
            parent_session: None,
            is_seeded: false,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        let event = SessionEvent {
            type_: "request/header".into(),
            seq: SessionSeq::new(2).unwrap(),
            time: 1,
            data: serde_json::json!({}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        };
        let error = migrate_v0_to_v3(header, &[event]).unwrap_err();
        assert!(error.contains("non-contiguous"));
    }
}
