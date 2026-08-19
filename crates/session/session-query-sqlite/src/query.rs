//! Request normalization, parameterized predicates, and result presentation.
//! Rust port of `packages/session-query/session-query-sqlite/src/query.ts`.

use dsh_session::SessionId;
use dsh_session_query::filters::validate_range;
use dsh_session_query::{
    SessionEventResultFilter, SessionEventSearchRequest, SessionQueryError, SessionQueryErrorCode,
    SessionResultFilter, SessionResultRange, SessionSearchCursor, SessionSearchRequest,
    materialize_session_event_result_filters, materialize_session_result_filters,
    session_search_cursor,
};
use sha2::{Digest, Sha256};

use crate::ResolvedConfig;

/// Collision-free marker inserted before an FTS5 match by `highlight()`.
pub const FTS_HIGHLIGHT_START: char = '\u{FDD0}';
/// Collision-free marker inserted after an FTS5 match by `highlight()`.
pub const FTS_HIGHLIGHT_END: char = '\u{FDD1}';

/// Largest page size whose internal lookahead remains an exact SQLite integer
/// binding (`Number.MAX_SAFE_INTEGER - 1`).
pub const SQLITE_MAX_PAGE_LIMIT: u64 = 9_007_199_254_740_990;

/// Portable host-parameter ceiling shared by predicate and statement builders.
pub const SQLITE_PORTABLE_VARIABLE_LIMIT: usize = 32_766;

/// Supported outer-predicate budget that keeps SQLite FTS5 MATCH usable.
pub const SQLITE_FTS5_OUTER_PREDICATE_LIMIT: usize = 14;

fn invalid_filter(detail: &str) -> SessionQueryError {
    SessionQueryError::new(
        SessionQueryErrorCode::SessionQueryInvalidFilter,
        detail.to_string(),
    )
}

/// Reject prospective SQLite binding growth beyond the portable ceiling.
pub fn assert_portable_binding_count(count: usize) -> Result<(), SessionQueryError> {
    if count > SQLITE_PORTABLE_VARIABLE_LIMIT {
        return Err(invalid_filter(&format!(
            "session-search request exceeds SQLite's portable {SQLITE_PORTABLE_VARIABLE_LIMIT}-variable limit; reduce filter values"
        )));
    }
    Ok(())
}

/// Reject compiled outer predicates beyond the supported FTS5 planner budget.
pub fn assert_fts5_outer_predicate_count(count: usize) -> Result<(), SessionQueryError> {
    if count > SQLITE_FTS5_OUTER_PREDICATE_LIMIT {
        return Err(invalid_filter(&format!(
            "session-search request exceeds the supported SQLite FTS5 outer-predicate budget of {SQLITE_FTS5_OUTER_PREDICATE_LIMIT}; reduce filters"
        )));
    }
    Ok(())
}

/// One portable SQLite binding value (TS `string | number`).
#[derive(Debug, Clone, PartialEq)]
pub enum Binding {
    Text(String),
    Integer(i64),
    Float(f64),
}

impl Binding {
    pub fn to_sql_value(&self) -> rusqlite::types::Value {
        match self {
            Binding::Text(text) => rusqlite::types::Value::Text(text.clone()),
            Binding::Integer(integer) => rusqlite::types::Value::Integer(*integer),
            Binding::Float(float) => rusqlite::types::Value::Real(*float),
        }
    }
}

/// Normalized cross-session request.
#[derive(Debug, Clone)]
pub struct NormalizedSessionRequest {
    pub query: String,
    pub session_filters: Vec<SessionResultFilter>,
    pub event_filters: Vec<SessionEventResultFilter>,
    pub limit: u64,
    pub cursor: Option<SessionSearchCursor>,
}

/// Normalized within-session request.
#[derive(Debug, Clone)]
pub struct NormalizedEventRequest {
    pub session_id: SessionId,
    pub query: String,
    pub filters: Vec<SessionEventResultFilter>,
    pub limit: u64,
    pub cursor: Option<SessionSearchCursor>,
}

/// Parameterized SQL predicate fragment.
#[derive(Debug, Clone, Default)]
pub struct SqlWhere {
    /// SQL without the leading `WHERE`.
    pub sql: String,
    /// Bindings in placeholder order.
    pub params: Vec<Binding>,
    /// Number of compiled predicates in `sql`.
    pub predicate_count: usize,
}

/// Validate and canonicalize a cross-session request.
pub fn normalize_session_request(
    request: &SessionSearchRequest,
    config: &ResolvedConfig,
) -> Result<NormalizedSessionRequest, SessionQueryError> {
    let session_filters =
        materialize_session_result_filters(request.session_filters.as_deref().unwrap_or(&[]))?;
    for filter in &session_filters {
        if let SessionResultFilter::CreatedAt { from, to } = filter {
            validate_range(
                "created-at",
                SessionResultRange {
                    from: *from,
                    to: *to,
                },
            )?;
        }
    }
    let event_filters =
        materialize_metadata_filters(request.event_filters.as_deref().unwrap_or(&[]))?;
    let cursor = materialize_cursor(request.cursor.clone());
    Ok(NormalizedSessionRequest {
        query: normalize_query(&request.query)?,
        session_filters,
        event_filters,
        limit: normalize_limit(request.limit, config)?,
        cursor,
    })
}

/// Validate and canonicalize a within-session request.
pub fn normalize_event_request(
    request: &SessionEventSearchRequest,
    config: &ResolvedConfig,
) -> Result<NormalizedEventRequest, SessionQueryError> {
    let Some(session_id) = &request.session_id else {
        return Err(invalid_filter("session-search session id must be text"));
    };
    let filters = materialize_metadata_filters(request.filters.as_deref().unwrap_or(&[]))?;
    let cursor = materialize_cursor(request.cursor.clone());
    Ok(NormalizedEventRequest {
        session_id: session_id.clone(),
        query: normalize_query(&request.query)?,
        filters,
        limit: normalize_limit(request.limit, config)?,
        cursor,
    })
}

/// Compile logical-session predicates against selected-document columns.
pub fn build_session_where(filters: &[SessionResultFilter]) -> Result<SqlWhere, SessionQueryError> {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Binding> = Vec::new();
    for filter in filters {
        match filter {
            SessionResultFilter::Id { values } => {
                add_list(
                    &mut clauses,
                    &mut params,
                    "session_id",
                    &values
                        .iter()
                        .map(|id| Binding::Text(id.as_str().to_string()))
                        .collect::<Vec<_>>(),
                )?;
            }
            SessionResultFilter::Cwd { values } => {
                add_nullable_list(
                    &mut clauses,
                    &mut params,
                    "cwd",
                    &values
                        .iter()
                        .map(|value| value.clone().map(Binding::Text))
                        .collect::<Vec<_>>(),
                )?;
            }
            SessionResultFilter::CreatedAt { from, to } => {
                add_range(&mut clauses, &mut params, "created_at", *from, *to)?;
            }
            SessionResultFilter::Parent { values } => {
                add_nullable_list(
                    &mut clauses,
                    &mut params,
                    "parent_session",
                    &values
                        .iter()
                        .map(|value| {
                            value
                                .as_ref()
                                .map(|id| Binding::Text(id.as_str().to_string()))
                        })
                        .collect::<Vec<_>>(),
                )?;
            }
            SessionResultFilter::Availability { values } => {
                let mut availability: Vec<dsh_session_query::SessionAvailability> = Vec::new();
                for value in values {
                    if !availability.contains(value) {
                        availability.push(*value);
                    }
                }
                if availability.is_empty() {
                    clauses.push("0".to_string());
                } else if availability.len() == 1 {
                    match availability[0] {
                        dsh_session_query::SessionAvailability::Live => {
                            clauses.push("live = 1".to_string())
                        }
                        dsh_session_query::SessionAvailability::Persisted => {
                            clauses.push("persisted = 1".to_string())
                        }
                    }
                }
            }
        }
    }
    assert_fts5_outer_predicate_count(clauses.len())?;
    Ok(SqlWhere {
        sql: clauses.join(" AND "),
        params,
        predicate_count: clauses.len(),
    })
}

/// Compile event metadata predicates against selected-document columns.
pub fn build_event_where(
    filters: &[SessionEventResultFilter],
) -> Result<SqlWhere, SessionQueryError> {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Binding> = Vec::new();
    for filter in filters {
        match filter {
            SessionEventResultFilter::Seq { from, to } => {
                add_range(&mut clauses, &mut params, "seq", *from, *to)?;
            }
            SessionEventResultFilter::Time { from, to } => {
                add_range(&mut clauses, &mut params, "time", *from, *to)?;
            }
            SessionEventResultFilter::Type { values } => {
                add_list(
                    &mut clauses,
                    &mut params,
                    "type",
                    &values
                        .iter()
                        .map(|value| Binding::Text(value.clone()))
                        .collect::<Vec<_>>(),
                )?;
            }
            SessionEventResultFilter::Surface { values } => {
                add_list(
                    &mut clauses,
                    &mut params,
                    "surface",
                    &values
                        .iter()
                        .map(|value| Binding::Text(value.as_str().to_string()))
                        .collect::<Vec<_>>(),
                )?;
            }
            SessionEventResultFilter::Text { .. } => {
                return Err(invalid_filter(
                    "session-search metadata filters do not accept text clauses",
                ));
            }
        }
    }
    assert_fts5_outer_predicate_count(clauses.len())?;
    Ok(SqlWhere {
        sql: clauses.join(" AND "),
        params,
        predicate_count: clauses.len(),
    })
}

/// Quote caller text as one FTS5 phrase so query syntax remains inert data.
pub fn quote_fts_data(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

/// Remove reserved marker collisions before text enters FTS5 or MATCH.
pub fn sanitize_fts_text(text: &str) -> String {
    text.replace('\0', "\u{FFFD}")
        .replace(FTS_HIGHLIGHT_START, "\u{FFFD}")
        .replace(FTS_HIGHLIGHT_END, "\u{FFFD}")
}

/// Build the stable normalized request identity stored in opaque cursors.
pub fn request_fingerprint(request: &RequestFingerprint) -> String {
    let json = match request {
        RequestFingerprint::Sessions {
            query,
            session_filters,
            event_filters,
            limit,
        } => serde_json::json!({
            "scope": "sessions",
            "query": query,
            "sessionFilters": canonical_session_filters(session_filters),
            "eventFilters": canonical_event_filters(event_filters),
            "limit": limit,
        }),
        RequestFingerprint::Events {
            session_id,
            query,
            filters,
            limit,
        } => serde_json::json!({
            "scope": "events",
            "sessionId": session_id.as_str(),
            "query": query,
            "filters": canonical_event_filters(filters),
            "limit": limit,
        }),
    };
    let encoded = serde_json::to_string(&json).expect("fingerprint json");
    let digest = Sha256::digest(encoded.as_bytes());
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// The two normalized request shapes folded into one fingerprint input.
pub enum RequestFingerprint<'a> {
    Sessions {
        query: &'a str,
        session_filters: &'a [SessionResultFilter],
        event_filters: &'a [SessionEventResultFilter],
        limit: u64,
    },
    Events {
        session_id: &'a SessionId,
        query: &'a str,
        filters: &'a [SessionEventResultFilter],
        limit: u64,
    },
}

/// Build a whitespace-normalized excerpt no longer than `max_chars` Unicode
/// code points.
pub fn make_snippet(marked_text: &str, max_chars: usize) -> String {
    let (clean, match_start) = normalize_marked_text(marked_text);
    let characters: Vec<char> = clean.chars().collect();
    if characters.len() <= max_chars {
        return clean;
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let matched_index = match_start.min(characters.len() - 1);
    let mut start = matched_index.saturating_sub(max_chars / 3);
    let prefix = if start > 0 { "…" } else { "" };
    let mut suffix = "…";
    let mut content_length =
        max_chars.saturating_sub(prefix.chars().count() + suffix.chars().count());
    if content_length < 1 {
        start = matched_index;
        suffix = "";
        content_length = max_chars.saturating_sub(prefix.chars().count() + suffix.chars().count());
    } else if matched_index >= start + content_length {
        start = matched_index - content_length + 1;
    }
    let mut end = characters.len().min(start + content_length);
    if end == characters.len() {
        suffix = "";
        content_length = max_chars.saturating_sub(prefix.chars().count());
        start = end.saturating_sub(content_length);
    }
    end = characters.len().min(start + content_length);
    let mut out = String::new();
    out.push_str(prefix);
    out.extend(characters[start..end].iter());
    out.push_str(suffix);
    out
}

fn normalize_marked_text(marked_text: &str) -> (String, usize) {
    let mut characters: Vec<char> = Vec::new();
    let mut match_start: Option<usize> = None;
    for character in marked_text.chars() {
        if character == FTS_HIGHLIGHT_START {
            match_start.get_or_insert(characters.len());
            continue;
        }
        if character == FTS_HIGHLIGHT_END {
            continue;
        }
        if character.is_whitespace() {
            if !characters.is_empty() && *characters.last().expect("non-empty") != ' ' {
                characters.push(' ');
            }
        } else {
            characters.push(character);
        }
    }
    if characters.last() == Some(&' ') {
        characters.pop();
    }
    (characters.into_iter().collect(), match_start.unwrap_or(0))
}

/// One cursor payload (TS `CursorPayload`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CursorPayload {
    pub version: u32,
    pub instance: String,
    pub scope: String,
    pub fingerprint: String,
    pub generation: String,
    pub offset: u64,
}

/// Encode an opaque continuation cursor for the public search contract.
pub fn encode_cursor(payload: &CursorPayload) -> SessionSearchCursor {
    let json = serde_json::to_string(payload).expect("cursor json");
    use base64::Engine;
    session_search_cursor(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
}

/// Decode and bind a cursor to one normalized request identity.
pub fn decode_cursor(
    cursor: &SessionSearchCursor,
    instance: &str,
    scope: &str,
    fingerprint: &str,
    generation: &str,
) -> Result<u64, SessionQueryError> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor.as_str())
        .map_err(|_| invalid_cursor())?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).map_err(|_| invalid_cursor())?;
    let valid = json.get("version").and_then(serde_json::Value::as_u64) == Some(1)
        && json.get("instance").and_then(serde_json::Value::as_str) == Some(instance)
        && json.get("scope").and_then(serde_json::Value::as_str) == Some(scope)
        && json.get("fingerprint").and_then(serde_json::Value::as_str) == Some(fingerprint)
        && json
            .get("offset")
            .and_then(serde_json::Value::as_u64)
            .is_some();
    if !valid {
        return Err(invalid_cursor());
    }
    let generation_matches =
        json.get("generation").and_then(serde_json::Value::as_str) == Some(generation);
    if !generation_matches {
        return Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryStaleCursor,
            "session-search cursor is stale because its relevant corpus changed",
        ));
    }
    Ok(json
        .get("offset")
        .and_then(serde_json::Value::as_u64)
        .expect("checked"))
}

fn invalid_cursor() -> SessionQueryError {
    SessionQueryError::new(
        SessionQueryErrorCode::SessionQueryInvalidCursor,
        "session-search cursor is invalid",
    )
}

fn normalize_query(value: &str) -> Result<String, SessionQueryError> {
    let query = value
        .trim()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if query.is_empty() {
        return Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryInvalidQuery,
            "session-search query must contain non-whitespace text",
        ));
    }
    if query.contains('\0') {
        return Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryInvalidQuery,
            "session-search query must not contain NUL",
        ));
    }
    Ok(sanitize_fts_text(&query))
}

fn materialize_cursor(cursor: Option<SessionSearchCursor>) -> Option<SessionSearchCursor> {
    cursor
}

/// Validate event metadata filters (the `Text` clause is rejected).
fn materialize_metadata_filters(
    filters: &[SessionEventResultFilter],
) -> Result<Vec<SessionEventResultFilter>, SessionQueryError> {
    for filter in filters {
        match filter {
            SessionEventResultFilter::Seq { from, to } => validate_range(
                "seq",
                SessionResultRange {
                    from: *from,
                    to: *to,
                },
            )?,
            SessionEventResultFilter::Time { from, to } => validate_range(
                "time",
                SessionResultRange {
                    from: *from,
                    to: *to,
                },
            )?,
            SessionEventResultFilter::Type { .. } => {}
            SessionEventResultFilter::Surface { .. } => {}
            SessionEventResultFilter::Text { .. } => {
                return Err(invalid_filter(
                    "session-search metadata filters do not accept text clauses",
                ));
            }
        }
    }
    materialize_session_event_result_filters(filters)
}

fn normalize_limit(value: Option<u64>, config: &ResolvedConfig) -> Result<u64, SessionQueryError> {
    let limit = value.unwrap_or(config.default_limit);
    let max_limit = config.max_limit.min(SQLITE_MAX_PAGE_LIMIT);
    if limit < 1 || limit > max_limit {
        return Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryInvalidLimit,
            format!("session-search limit must be an integer between 1 and {max_limit}"),
        ));
    }
    Ok(limit)
}

fn add_list(
    clauses: &mut Vec<String>,
    params: &mut Vec<Binding>,
    column: &str,
    values: &[Binding],
) -> Result<(), SessionQueryError> {
    if values.is_empty() {
        clauses.push("0".to_string());
        return Ok(());
    }
    clauses.push(format!(
        "{column} IN ({})",
        append_list_bindings(params, values)?
    ));
    Ok(())
}

fn add_nullable_list(
    clauses: &mut Vec<String>,
    params: &mut Vec<Binding>,
    column: &str,
    values: &[Option<Binding>],
) -> Result<(), SessionQueryError> {
    if values.is_empty() {
        clauses.push("0".to_string());
        return Ok(());
    }
    let concrete: Vec<Binding> = values
        .iter()
        .filter_map(|value| value.as_ref().cloned())
        .collect();
    let mut parts: Vec<String> = Vec::new();
    if !concrete.is_empty() {
        parts.push(format!(
            "{column} IN ({})",
            append_list_bindings(params, &concrete)?
        ));
    }
    if values.iter().any(Option::is_none) {
        parts.push(format!("{column} IS NULL"));
    }
    clauses.push(format!("({})", parts.join(" OR ")));
    Ok(())
}

fn add_range(
    clauses: &mut Vec<String>,
    params: &mut Vec<Binding>,
    column: &str,
    from: Option<f64>,
    to: Option<f64>,
) -> Result<(), SessionQueryError> {
    if let Some(from) = from {
        assert_portable_binding_count(params.len() + 1)?;
        clauses.push(format!("CAST({column} AS INTEGER) >= ?"));
        params.push(Binding::Float(from));
    }
    if let Some(to) = to {
        assert_portable_binding_count(params.len() + 1)?;
        clauses.push(format!("CAST({column} AS INTEGER) <= ?"));
        params.push(Binding::Float(to));
    }
    Ok(())
}

fn append_list_bindings(
    params: &mut Vec<Binding>,
    values: &[Binding],
) -> Result<String, SessionQueryError> {
    assert_portable_binding_count(params.len() + values.len())?;
    params.extend(values.iter().cloned());
    Ok(values.iter().map(|_| "?").collect::<Vec<_>>().join(", "))
}

fn canonical_filters(filters: &[SessionResultFilter]) -> Vec<serde_json::Value> {
    filters.iter().map(canonical_session_filter).collect()
}

fn canonical_session_filter(filter: &SessionResultFilter) -> serde_json::Value {
    match filter {
        SessionResultFilter::Id { values } => serde_json::json!({
            "kind": "id",
            "values": sorted_texts(&values.iter().map(|id| Some(id.as_str())).collect::<Vec<_>>()),
        }),
        SessionResultFilter::Cwd { values } => serde_json::json!({
            "kind": "cwd",
            "values": sorted_texts(&values.iter().map(|value| value.as_deref()).collect::<Vec<_>>()),
        }),
        SessionResultFilter::CreatedAt { from, to } => serde_json::json!({
            "kind": "created-at",
            "from": from,
            "to": to,
        }),
        SessionResultFilter::Parent { values } => serde_json::json!({
            "kind": "parent",
            "values": sorted_texts(&values.iter().map(|value| value.as_ref().map(|id| id.as_str())).collect::<Vec<_>>()),
        }),
        SessionResultFilter::Availability { values } => serde_json::json!({
            "kind": "availability",
            "values": sorted_texts(&values.iter().map(|value| Some(value.as_str())).collect::<Vec<_>>()),
        }),
    }
}

fn canonical_event_filters_unsorted(
    filters: &[SessionEventResultFilter],
) -> Vec<serde_json::Value> {
    filters.iter().map(canonical_event_filter).collect()
}

fn canonical_event_filter(filter: &SessionEventResultFilter) -> serde_json::Value {
    match filter {
        SessionEventResultFilter::Seq { from, to } => serde_json::json!({
            "kind": "seq",
            "from": from,
            "to": to,
        }),
        SessionEventResultFilter::Time { from, to } => serde_json::json!({
            "kind": "time",
            "from": from,
            "to": to,
        }),
        SessionEventResultFilter::Type { values } => serde_json::json!({
            "kind": "type",
            "values": sorted_texts(&values.iter().map(|value| Some(value.as_str())).collect::<Vec<_>>()),
        }),
        SessionEventResultFilter::Surface { values } => serde_json::json!({
            "kind": "surface",
            "values": sorted_texts(&values.iter().map(|value| Some(value.as_str())).collect::<Vec<_>>()),
        }),
        SessionEventResultFilter::Text { text } => serde_json::json!({
            "kind": "text",
            "text": text,
        }),
    }
}

/// JSON-string sort mirroring the TS `localeCompare` over ASCII JSON text.
fn sorted_texts(values: &[Option<&str>]) -> Vec<serde_json::Value> {
    let mut copies: Vec<Option<&str>> = values.to_vec();
    copies.sort_by(|a, b| match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) => a.cmp(b),
    });
    copies
        .into_iter()
        .map(|value| match value {
            Some(text) => serde_json::Value::String(text.to_string()),
            None => serde_json::Value::Null,
        })
        .collect()
}

/// The canonicalized filter identity used by the cursor fingerprint.
pub fn canonical_session_filters(filters: &[SessionResultFilter]) -> Vec<serde_json::Value> {
    let mut values = canonical_filters(filters);
    sort_json(&mut values);
    values
}

/// The canonicalized event filter identity used by the cursor fingerprint.
pub fn canonical_event_filters(filters: &[SessionEventResultFilter]) -> Vec<serde_json::Value> {
    let mut values = canonical_event_filters_unsorted(filters);
    sort_json(&mut values);
    values
}

fn sort_json(values: &mut [serde_json::Value]) {
    values.sort_by(|a, b| {
        serde_json::to_string(a)
            .expect("json")
            .cmp(&serde_json::to_string(b).expect("json"))
    });
}
