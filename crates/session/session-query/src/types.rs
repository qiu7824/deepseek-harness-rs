//! Public records for exact reads and relationship traces over the
//! live-preferred logical session corpus. Rust port of
//! `packages/session-query/session-query/src/types.ts`.

use dsh_session::{SessionEvent, SessionHeader, SessionId};
use dsh_session_title::SessionTitleSnapshot;

use crate::cursor::SessionSearchCursor;

/// Whether an event is current model context, replaced context, or
/// raw-log-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEventSurface {
    Current,
    Shadowed,
    LogOnly,
}

impl SessionEventSurface {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionEventSurface::Current => "current",
            SessionEventSurface::Shadowed => "shadowed",
            SessionEventSurface::LogOnly => "log-only",
        }
    }
}

/// One current-surface entry: the surface seq and its detached event (the
/// TS `SurfaceEvent`).
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceEvent {
    pub seq: u64,
    pub event: SessionEvent,
}

/// Lightweight identity and source availability for one logical session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    /// Cloned session header selected from the live-preferred corpus.
    pub header: SessionHeader,
    /// Whether the id currently exists in `ctx.sessions`.
    pub live: bool,
    /// Whether the active persistence backend currently materializes the id.
    pub persisted: bool,
}

/// One atomic live-preferred observation of a session's current model
/// surface.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSurfaceSnapshot {
    pub session: SessionHeader,
    /// Highest raw-log seq included in the observation, or `None` for an
    /// empty log.
    pub captured_through_seq: Option<u64>,
    /// Cloned current surface events in model-history order.
    pub events: Vec<SurfaceEvent>,
}

/// One validated detached observation of a logical session's complete raw
/// log.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLogSnapshot {
    pub session: SessionHeader,
    pub events: Vec<SessionEvent>,
}

/// Lightweight metadata for one event within a logical session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventRecord {
    pub session_id: SessionId,
    pub seq: u64,
    pub type_: String,
    pub time: i64,
    pub surface: SessionEventSurface,
}

/// Recursive descendant node in a session-lineage trace.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLineageNode {
    pub session: SessionRecord,
    pub descendants: Vec<SessionLineageNode>,
}

/// Known ancestry and descendants for one logical session (TS closed union;
/// `complete` carries the root, `incomplete` the first unresolved parent).
#[derive(Debug, Clone, PartialEq)]
pub enum SessionLineageTrace {
    Complete {
        target: SessionRecord,
        ancestors: Vec<SessionRecord>,
        descendants: Vec<SessionLineageNode>,
        root: SessionRecord,
    },
    Partial {
        target: SessionRecord,
        ancestors: Vec<SessionRecord>,
        descendants: Vec<SessionLineageNode>,
        unresolved_parent_id: SessionId,
    },
}

/// Request for direct surface replacements and relationships to cited source
/// events around one event.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventTraceRequest {
    pub session_id: SessionId,
    pub seq: u64,
}

/// Direct surface replacements and relationships to cited source events for
/// one event.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventTrace {
    pub target: SessionEventRecord,
    /// Immediate positional replacement event, when the target was shadowed.
    pub replaced_by: Option<u64>,
    /// Positional replacers from the immediate replacement to the final
    /// replacement.
    pub replacement_chain: Vec<u64>,
    /// Surface nodes directly removed when the target itself performed a
    /// replacement.
    pub replaced_event_seqs: Vec<u64>,
    /// Earlier events cited directly as sources, in their recorded order.
    pub source_event_seqs: Vec<u64>,
    /// Later events that directly cite the target as a source, in log order.
    pub derived_event_seqs: Vec<u64>,
}

/// Event relationships bound to the same session-header observation.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventTraceObservation {
    pub session: SessionHeader,
    pub target: SessionEventRecord,
    pub replaced_by: Option<u64>,
    pub replacement_chain: Vec<u64>,
    pub replaced_event_seqs: Vec<u64>,
    pub source_event_seqs: Vec<u64>,
    pub derived_event_seqs: Vec<u64>,
}

/// Request for one event plus raw neighboring log context.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventReadRequest {
    pub session_id: SessionId,
    pub seq: u64,
    pub before: Option<u64>,
    pub after: Option<u64>,
}

/// Full target event and a bounded raw-log window.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventWindow {
    pub session: SessionHeader,
    pub target: SessionEvent,
    pub events: Vec<SessionEvent>,
    pub start_seq: u64,
    pub end_seq: u64,
}

/// Latest folded title bound to the same session-header observation.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTitleObservation {
    pub session: SessionHeader,
    pub title: Option<SessionTitleSnapshot>,
}

/// One ordered result from a batch title observation.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionTitleObservationResult {
    Fulfilled {
        session_id: SessionId,
        value: SessionTitleObservation,
    },
    Rejected {
        session_id: SessionId,
        reason: String,
    },
}

/// Inclusive numeric interval used by time and sequence filters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionResultRange {
    pub from: Option<f64>,
    pub to: Option<f64>,
}

/// Source availability predicates understood by logical-session filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAvailability {
    Live,
    Persisted,
}

impl SessionAvailability {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionAvailability::Live => "live",
            SessionAvailability::Persisted => "persisted",
        }
    }
}

/// One logical-session predicate. A filter array is ANDed; `values` within a
/// clause are ORed.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionResultFilter {
    Id { values: Vec<SessionId> },
    Cwd { values: Vec<Option<String>> },
    CreatedAt { from: Option<f64>, to: Option<f64> },
    Parent { values: Vec<Option<SessionId>> },
    Availability { values: Vec<SessionAvailability> },
}

/// One event predicate. A filter array is ANDed; list-valued clauses are
/// ORed. Text is a literal, case-insensitive, whitespace-flexible
/// semantic-text scan.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEventResultFilter {
    Seq { from: Option<f64>, to: Option<f64> },
    Time { from: Option<f64>, to: Option<f64> },
    Type { values: Vec<String> },
    Surface { values: Vec<SessionEventSurface> },
    Text { text: String },
}

/// Searchable semantic document derived from one session event.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventSearchDocument {
    pub session_id: SessionId,
    pub seq: u64,
    pub type_: String,
    pub time: i64,
    pub surface: SessionEventSurface,
    /// First-party semantic text used by scan filters and full-text indexes.
    pub text: String,
}

/// One cursor-paginated result page.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchPage<T> {
    pub items: Vec<T>,
    /// Opaque continuation cursor, absent on the final page.
    pub next_cursor: Option<SessionSearchCursor>,
}

/// Event-search results bound to the indexed target-session observation.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventSearchPage {
    pub session: SessionHeader,
    pub items: Vec<SessionEventSearchHit>,
    pub next_cursor: Option<SessionSearchCursor>,
}

/// Controls shared by cross-session and within-session search calls.
#[derive(Clone, Default)]
pub struct SessionSearchExecContext {
    pub signal: Option<crate::corpus::SessionQueryAbort>,
}

/// Cross-session full-text search request.
#[derive(Debug, Clone, Default)]
pub struct SessionSearchRequest {
    pub query: String,
    pub session_filters: Option<Vec<SessionResultFilter>>,
    pub event_filters: Option<Vec<SessionEventResultFilter>>,
    pub limit: Option<u64>,
    pub cursor: Option<SessionSearchCursor>,
}

/// Within-session full-text search request.
#[derive(Debug, Clone, Default)]
pub struct SessionEventSearchRequest {
    pub session_id: Option<SessionId>,
    pub query: String,
    pub filters: Option<Vec<SessionEventResultFilter>>,
    pub limit: Option<u64>,
    pub cursor: Option<SessionSearchCursor>,
}

/// One event full-text search hit with a bounded plain-text excerpt.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventSearchHit {
    pub session_id: SessionId,
    pub seq: u64,
    pub type_: String,
    pub time: i64,
    pub surface: SessionEventSurface,
    pub snippet: String,
}

/// One grouped cross-session hit, ranked by its strongest matching event.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchHit {
    pub record: SessionRecord,
    pub best_match: SessionEventSearchHit,
}
