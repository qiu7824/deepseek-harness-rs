//! Shared event metadata and semantic-document projection. Rust port of
//! `packages/session-query/session-query/src/documents.ts`.

use std::collections::HashMap;

use dsh_session::{SessionEvent, SessionId, fold_surface};

use crate::config::{SessionQueryError, SessionQueryErrorCode};
use crate::extraction::extract_session_event_text;
use crate::types::{SessionEventRecord, SessionEventSearchDocument, SessionEventSurface};

/// Project a raw log into lightweight surface-aware event records (TS
/// `buildSessionEventRecords`).
pub fn build_session_event_records(
    session_id: &SessionId,
    events: &[SessionEvent],
) -> Result<Vec<SessionEventRecord>, SessionQueryError> {
    let surface_by_seq = classify_surface(events)?;
    Ok(events
        .iter()
        .map(|event| SessionEventRecord {
            session_id: session_id.clone(),
            seq: event.seq.get(),
            type_: event.type_.clone(),
            time: event.time,
            surface: surface_by_seq
                .get(&event.seq.get())
                .copied()
                .unwrap_or(SessionEventSurface::LogOnly),
        })
        .collect())
}

/// Build first-party semantic documents for one complete raw event log (TS
/// `buildSessionEventSearchDocuments`); structural events are omitted.
pub fn build_session_event_search_documents(
    session_id: &SessionId,
    events: &[SessionEvent],
) -> Result<Vec<SessionEventSearchDocument>, SessionQueryError> {
    let surface_by_seq = classify_surface(events)?;
    let mut documents = Vec::new();
    for event in events {
        let text = extract_session_event_text(event);
        if text.is_empty() {
            continue;
        }
        documents.push(SessionEventSearchDocument {
            session_id: session_id.clone(),
            seq: event.seq.get(),
            type_: event.type_.clone(),
            time: event.time,
            surface: surface_by_seq
                .get(&event.seq.get())
                .copied()
                .unwrap_or(SessionEventSurface::LogOnly),
            text,
        });
    }
    Ok(documents)
}

fn classify_surface(
    events: &[SessionEvent],
) -> Result<HashMap<u64, SessionEventSurface>, SessionQueryError> {
    let folded = fold_surface(events).map_err(|error| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryInvalidSurface,
            format!("invalid session surface: {error}"),
        )
    })?;
    let mut result = HashMap::new();
    for seq in folded.nodes {
        result.insert(seq, SessionEventSurface::Current);
    }
    for replacement in folded.replacements {
        for seq in replacement.shadowed_seqs {
            result.insert(seq, SessionEventSurface::Shadowed);
        }
    }
    Ok(result)
}
