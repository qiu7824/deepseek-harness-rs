//! Pure provider-independent predicates for logical sessions and event text.
//! Rust port of `packages/session-query/session-query/src/filters.ts`.

use crate::config::{SessionQueryError, SessionQueryErrorCode};
use crate::types::{
    SessionEventResultFilter, SessionEventSearchDocument, SessionEventSurface, SessionRecord,
    SessionResultFilter, SessionResultRange,
};

fn invalid_filter(detail: &str) -> SessionQueryError {
    SessionQueryError::new(
        SessionQueryErrorCode::SessionQueryInvalidFilter,
        format!("session {detail}"),
    )
}

/// Apply ANDed logical-session filters while preserving input order (TS
/// `filterSessionResults`).
pub fn filter_session_results(
    records: &[SessionRecord],
    filters: &[SessionResultFilter],
) -> Vec<SessionRecord> {
    records
        .iter()
        .filter(|record| {
            filters
                .iter()
                .all(|filter| session_predicate(filter, record))
        })
        .cloned()
        .collect()
}

/// Apply ANDed event filters to extracted semantic documents (TS
/// `filterSessionEventDocuments`).
pub fn filter_session_event_documents(
    documents: &[SessionEventSearchDocument],
    filters: &[SessionEventResultFilter],
) -> Vec<SessionEventSearchDocument> {
    documents
        .iter()
        .filter(|document| {
            filters
                .iter()
                .all(|filter| event_predicate(filter, document))
        })
        .cloned()
        .collect()
}

/// Copy and validate logical-session filters (TS
/// `materializeSessionResultFilters`).
pub fn materialize_session_result_filters(
    filters: &[SessionResultFilter],
) -> Result<Vec<SessionResultFilter>, SessionQueryError> {
    filters
        .iter()
        .map(|filter| match filter {
            SessionResultFilter::Id { values } => Ok(SessionResultFilter::Id {
                values: values.clone(),
            }),
            SessionResultFilter::Cwd { values } => Ok(SessionResultFilter::Cwd {
                values: values.clone(),
            }),
            SessionResultFilter::CreatedAt { from, to } => Ok(SessionResultFilter::CreatedAt {
                from: *from,
                to: *to,
            }),
            SessionResultFilter::Parent { values } => Ok(SessionResultFilter::Parent {
                values: values.clone(),
            }),
            SessionResultFilter::Availability { values } => Ok(SessionResultFilter::Availability {
                values: values.clone(),
            }),
        })
        .collect()
}

/// Copy and validate event filters (TS
/// `materializeSessionEventResultFilters`).
pub fn materialize_session_event_result_filters(
    filters: &[SessionEventResultFilter],
) -> Result<Vec<SessionEventResultFilter>, SessionQueryError> {
    filters
        .iter()
        .map(|filter| match filter {
            SessionEventResultFilter::Seq { from, to } => Ok(SessionEventResultFilter::Seq {
                from: *from,
                to: *to,
            }),
            SessionEventResultFilter::Time { from, to } => Ok(SessionEventResultFilter::Time {
                from: *from,
                to: *to,
            }),
            SessionEventResultFilter::Type { values } => Ok(SessionEventResultFilter::Type {
                values: values.clone(),
            }),
            SessionEventResultFilter::Surface { values } => Ok(SessionEventResultFilter::Surface {
                values: values.clone(),
            }),
            SessionEventResultFilter::Text { text } => {
                Ok(SessionEventResultFilter::Text { text: text.clone() })
            }
        })
        .collect()
}

/// Compile a literal case-insensitive, whitespace-flexible semantic-text
/// match, safe from regex injection (TS `compileSessionTextFilter`).
pub fn compile_session_text_filter(text: &str) -> Result<regex::Regex, SessionQueryError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryInvalidFilter,
            "session text filter must contain non-whitespace text",
        ));
    }
    let parts: Vec<String> = trimmed
        .split_whitespace()
        .map(|part| regex::escape(part))
        .collect();
    let pattern = format!("(?i){}", parts.join(r"\s+"));
    regex::Regex::new(&pattern)
        .map_err(|error| invalid_filter(&format!("text filter failed to compile: {error}")))
}

fn session_predicate(filter: &SessionResultFilter, record: &SessionRecord) -> bool {
    match filter {
        SessionResultFilter::Id { values } => values.contains(&record.header.id),
        SessionResultFilter::Cwd { values } => values.contains(&record.header.cwd),
        SessionResultFilter::CreatedAt { from, to } => {
            matches_range(record.header.created_at as f64, *from, *to)
        }
        SessionResultFilter::Parent { values } => values.contains(&record.header.parent_session),
        SessionResultFilter::Availability { values } => values.iter().any(|value| match value {
            crate::types::SessionAvailability::Live => record.live,
            crate::types::SessionAvailability::Persisted => record.persisted,
        }),
    }
}

fn event_predicate(
    filter: &SessionEventResultFilter,
    document: &SessionEventSearchDocument,
) -> bool {
    match filter {
        SessionEventResultFilter::Seq { from, to } => {
            matches_range(document.seq as f64, *from, *to)
        }
        SessionEventResultFilter::Time { from, to } => {
            matches_range(document.time as f64, *from, *to)
        }
        SessionEventResultFilter::Type { values } => values.contains(&document.type_),
        SessionEventResultFilter::Surface { values } => values.contains(&document.surface),
        SessionEventResultFilter::Text { text } => match compile_session_text_filter(text) {
            Ok(pattern) => pattern.is_match(&document.text),
            Err(_) => false,
        },
    }
}

fn matches_range(value: f64, from: Option<f64>, to: Option<f64>) -> bool {
    (from.is_none_or(|from| value >= from)) && (to.is_none_or(|to| value <= to))
}

/// The TS surface-value strings for materialization diagnostics.
pub fn surface_from_str(value: &str) -> Option<SessionEventSurface> {
    match value {
        "current" => Some(SessionEventSurface::Current),
        "shadowed" => Some(SessionEventSurface::Shadowed),
        "log-only" => Some(SessionEventSurface::LogOnly),
        _ => None,
    }
}

/// Validate one raw range clause (TS `validateRange`).
pub fn validate_range(name: &str, range: SessionResultRange) -> Result<(), SessionQueryError> {
    if range.from.is_some_and(|from| !from.is_finite()) {
        return Err(invalid_filter(&format!(
            "{name} filter from must be finite"
        )));
    }
    if range.to.is_some_and(|to| !to.is_finite()) {
        return Err(invalid_filter(&format!("{name} filter to must be finite")));
    }
    if let (Some(from), Some(to)) = (range.from, range.to) {
        if from > to {
            return Err(invalid_filter(&format!(
                "{name} filter from must be less than or equal to to"
            )));
        }
    }
    Ok(())
}
