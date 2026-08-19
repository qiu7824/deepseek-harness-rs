//! One-shot session-lineage and event-relationship tracing helpers. Rust
//! port of `packages/session-query/session-query/src/tracing.ts`.

use std::collections::{HashMap, HashSet};

use dsh_session::{SessionEvent, SessionId, fold_surface};

use crate::config::{SessionQueryError, SessionQueryErrorCode};
use crate::types::{
    SessionEventRecord, SessionEventSurface, SessionEventTrace, SessionLineageNode,
    SessionLineageTrace, SessionRecord, SurfaceEvent,
};

struct EventLogAnalysis {
    records: Vec<SessionEventRecord>,
    replaced_by: HashMap<u64, u64>,
    replaced_event_seqs: HashMap<u64, Vec<u64>>,
    current_seqs: Vec<u64>,
}

/// Classify a raw event log with one canonical surface fold (TS
/// `eventRecords`).
pub fn event_records(
    session_id: &SessionId,
    events: &[SessionEvent],
) -> Result<Vec<SessionEventRecord>, SessionQueryError> {
    Ok(analyze_event_log(session_id, events)?.records)
}

/// Fold and return the current model surface after validating the whole log
/// (TS `currentSurfaceEvents`).
pub fn current_surface_events(
    session_id: &SessionId,
    events: &[SessionEvent],
) -> Result<Vec<SurfaceEvent>, SessionQueryError> {
    let analysis = analyze_event_log(session_id, events)?;
    let mut surface = Vec::new();
    for seq in analysis.current_seqs {
        let event = events
            .get(seq as usize)
            .filter(|event| event.seq == seq && event.surface_op.is_some());
        let Some(event) = event else {
            return Err(SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryInvalidSurface,
                format!("invalid session surface: current node {seq} is not a surface event"),
            ));
        };
        surface.push(SurfaceEvent {
            seq,
            event: event.clone(),
        });
    }
    Ok(surface)
}

/// Trace one target after one canonical surface fold and whole-log
/// validation (TS `traceEvent`).
pub fn trace_event(
    session_id: &SessionId,
    events: &[SessionEvent],
    seq: u64,
) -> Result<SessionEventTrace, SessionQueryError> {
    let target = events
        .get(seq as usize)
        .filter(|event| event.seq == seq)
        .ok_or_else(|| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryEventNotFound,
                format!("session \"{session_id}\" has no event at seq {seq}"),
            )
        })?;
    let analysis = analyze_event_log(session_id, events)?;

    let mut replacement_chain: Vec<u64> = Vec::new();
    let mut replacement = analysis.replaced_by.get(&seq).copied();
    while let Some(next) = replacement {
        replacement_chain.push(next);
        replacement = analysis.replaced_by.get(&next).copied();
    }

    let mut derived_event_seqs: Vec<u64> = Vec::new();
    for event in events {
        if event.seq <= seq {
            continue;
        }
        if event_sources(event).contains(&seq) {
            derived_event_seqs.push(event.seq);
        }
    }

    let target_record = analysis.records.get(seq as usize).cloned().ok_or_else(|| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryEventNotFound,
            format!("session \"{session_id}\" has no event at seq {seq}"),
        )
    })?;
    Ok(SessionEventTrace {
        target: target_record,
        replaced_by: analysis.replaced_by.get(&seq).copied(),
        replacement_chain,
        replaced_event_seqs: analysis
            .replaced_event_seqs
            .get(&seq)
            .cloned()
            .unwrap_or_default(),
        source_event_seqs: event_sources(target),
        derived_event_seqs,
    })
}

/// Trace one target's known ancestry and recursively known descendants (TS
/// `traceSession`).
pub fn trace_session(
    records: &[SessionRecord],
    session_id: &SessionId,
) -> Result<SessionLineageTrace, SessionQueryError> {
    let by_id: HashMap<String, SessionRecord> = records
        .iter()
        .map(|record| (record.header.id.as_str().to_string(), record.clone()))
        .collect();
    let target = by_id.get(session_id.as_str()).cloned().ok_or_else(|| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQuerySessionNotFound,
            format!("session \"{session_id}\" not found"),
        )
    })?;

    let mut ancestors: Vec<SessionRecord> = Vec::new();
    let mut ancestry_seen: HashSet<String> = HashSet::from([session_id.as_str().to_string()]);
    let mut unresolved_parent_id: Option<SessionId> = None;
    let mut parent_id = target.header.parent_session.clone();
    while let Some(parent) = parent_id {
        if ancestry_seen.contains(parent.as_str()) {
            return Err(SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryInvalidLineage,
                format!("session lineage contains a cycle at \"{parent}\""),
            ));
        }
        ancestry_seen.insert(parent.as_str().to_string());
        match by_id.get(parent.as_str()) {
            Some(record) => {
                ancestors.push(record.clone());
                parent_id = record.header.parent_session.clone();
            }
            None => {
                unresolved_parent_id = Some(parent);
                break;
            }
        }
    }

    let mut children_by_parent: HashMap<String, Vec<SessionRecord>> = HashMap::new();
    for record in records {
        let Some(parent) = &record.header.parent_session else {
            continue;
        };
        children_by_parent
            .entry(parent.as_str().to_string())
            .or_default()
            .push(record.clone());
    }
    for children in children_by_parent.values_mut() {
        children.sort_by(|a, b| {
            a.header
                .created_at
                .cmp(&b.header.created_at)
                .then_with(|| a.header.id.as_str().cmp(b.header.id.as_str()))
        });
    }

    let descendants = build_descendants(&children_by_parent, session_id);
    let common = (target.clone(), ancestors.clone(), descendants);
    match unresolved_parent_id {
        Some(unresolved_parent_id) => Ok(SessionLineageTrace::Partial {
            target: common.0,
            ancestors: common.1,
            descendants: common.2,
            unresolved_parent_id,
        }),
        None => Ok(SessionLineageTrace::Complete {
            target: common.0,
            ancestors: common.1.clone(),
            descendants: common.2,
            root: ancestors.last().cloned().unwrap_or(target),
        }),
    }
}

fn analyze_event_log(
    session_id: &SessionId,
    events: &[SessionEvent],
) -> Result<EventLogAnalysis, SessionQueryError> {
    let folded = fold_surface(events).map_err(|error| {
        SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryInvalidSurface,
            format!("invalid session surface: {error}"),
        )
    })?;
    let current: HashSet<u64> = folded.nodes.iter().copied().collect();
    let mut replaced_by: HashMap<u64, u64> = HashMap::new();
    let mut replaced_event_seqs: HashMap<u64, Vec<u64>> = HashMap::new();
    for replacement in folded.replacements {
        replaced_event_seqs.insert(replacement.seq, replacement.shadowed_seqs.clone());
        for removed_seq in replacement.shadowed_seqs {
            replaced_by.insert(removed_seq, replacement.seq);
        }
    }
    Ok(EventLogAnalysis {
        records: events
            .iter()
            .map(|event| SessionEventRecord {
                session_id: session_id.clone(),
                seq: event.seq,
                type_: event.type_.clone(),
                time: event.time,
                surface: if current.contains(&event.seq) {
                    SessionEventSurface::Current
                } else if replaced_by.contains_key(&event.seq) {
                    SessionEventSurface::Shadowed
                } else {
                    SessionEventSurface::LogOnly
                },
            })
            .collect(),
        replaced_by,
        replaced_event_seqs,
        current_seqs: folded.nodes,
    })
}

fn event_sources(event: &SessionEvent) -> Vec<u64> {
    event.source_event_seqs.clone().unwrap_or_default()
}

fn build_descendants(
    children_by_parent: &HashMap<String, Vec<SessionRecord>>,
    session_id: &SessionId,
) -> Vec<SessionLineageNode> {
    // The TS version walks iteratively to bound stack depth; the Rust port
    // recurses per lineage level (delegation depth is bounded in practice).
    fn build(
        children_by_parent: &HashMap<String, Vec<SessionRecord>>,
        id: &str,
    ) -> Vec<SessionLineageNode> {
        children_by_parent
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|child| SessionLineageNode {
                session: child.clone(),
                descendants: build(children_by_parent, child.header.id.as_str()),
            })
            .collect()
    }
    build(children_by_parent, session_id.as_str())
}
