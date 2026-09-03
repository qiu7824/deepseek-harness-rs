//! Unified live-preferred session query service. Rust port of
//! `packages/session-query/session-query/src/index.ts`.
//!
//! Exact reads, filters, and traces are backend-independent concrete
//! behavior. A backend implements the full-text search face
//! ([`SessionQuerySearch`]) and mounts the engine with it.

pub mod config;
pub mod corpus;
pub mod cursor;
pub mod documents;
pub mod extraction;
pub mod filters;
pub mod invariant;
pub mod sources;
pub mod tracing;
pub mod types;

use std::sync::Arc;

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_session::SessionId;
use dsh_session_title::fold_session_title;

pub use crate::config::{
    Config, SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY, SESSION_QUERY_READ_WINDOW_MAX,
    SessionQueryError, SessionQueryErrorCode,
};
pub use crate::corpus::SessionCorpus;
pub use crate::cursor::{SessionSearchCursor, session_search_cursor};
pub use crate::documents::{build_session_event_records, build_session_event_search_documents};
pub use crate::extraction::extract_session_event_text;
pub use crate::filters::{
    compile_session_text_filter, filter_session_event_documents, filter_session_results,
    materialize_session_event_result_filters, materialize_session_result_filters,
};
pub use crate::sources::assert_session_headers_compatible;
pub use crate::types::*;

/// The provider-side full-text search face (the TS abstract members).
#[async_trait::async_trait]
pub trait SessionQuerySearch: Send + Sync + 'static {
    /// Search the live-preferred logical corpus and group by session.
    async fn search_sessions(
        &self,
        engine: &SessionQueryEngine,
        request: &SessionSearchRequest,
        exec: Option<&SessionSearchExecContext>,
    ) -> Result<SessionSearchPage<SessionSearchHit>, SessionQueryError>;

    /// Search events within one live-preferred logical session.
    async fn search_events(
        &self,
        engine: &SessionQueryEngine,
        request: &SessionEventSearchRequest,
        exec: Option<&SessionSearchExecContext>,
    ) -> Result<SessionEventSearchPage, SessionQueryError>;
}

/// Unified live-preferred session query service (TS `SessionQueryEngine`).
pub struct SessionQueryEngine {
    pub ctx: Context,
    corpus: SessionCorpus,
    read_window_max: u64,
    search: Option<Arc<dyn SessionQuerySearch>>,
}

impl SessionQueryEngine {
    /// Build the engine with a concrete search backend (the sqlite provider)
    /// or none (the disabled search face).
    pub fn build(
        ctx: &Context,
        config: &Config,
        search: Option<Arc<dyn SessionQuerySearch>>,
    ) -> Result<Arc<Self>, SessionQueryError> {
        let read_window_max = config
            .read_window_max
            .unwrap_or(SESSION_QUERY_READ_WINDOW_MAX);
        let persisted_inspect_concurrency = config
            .persisted_inspect_concurrency
            .unwrap_or(SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY);
        if persisted_inspect_concurrency == 0 {
            return Err(SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryInvalidConfig,
                "session-query: persistedInspectConcurrency must be a positive safe integer",
            ));
        }
        let corpus = SessionCorpus::new(ctx, persisted_inspect_concurrency);
        Ok(Arc::new(Self {
            ctx: ctx.clone(),
            corpus,
            read_window_max,
            search,
        }))
    }

    /// Build the engine and register it as the `sessionQuery` service.
    pub fn install(
        ctx: &Context,
        config: &Config,
        search: Option<Arc<dyn SessionQuerySearch>>,
    ) -> Result<Arc<Self>, SessionQueryError> {
        let engine = Self::build(ctx, config, search)?;
        ctx.register_service(engine.clone());
        Ok(engine)
    }

    /// Search the live-preferred logical corpus and group by session.
    pub async fn search_sessions(
        &self,
        request: &SessionSearchRequest,
        exec: Option<&SessionSearchExecContext>,
    ) -> Result<SessionSearchPage<SessionSearchHit>, SessionQueryError> {
        let search = self.search.as_ref().ok_or_else(|| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQuerySearchDisabled,
                "session search is disabled: no search backend is mounted",
            )
        })?;
        search.search_sessions(self, request, exec).await
    }

    /// Search events within one live-preferred logical session.
    pub async fn search_events(
        &self,
        request: &SessionEventSearchRequest,
        exec: Option<&SessionSearchExecContext>,
    ) -> Result<SessionEventSearchPage, SessionQueryError> {
        let search = self.search.as_ref().ok_or_else(|| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQuerySearchDisabled,
                "session search is disabled: no search backend is mounted",
            )
        })?;
        search.search_events(self, request, exec).await
    }

    /// List the complete logical corpus using live-preferred records.
    pub async fn list_sessions(
        &self,
        signal: Option<&crate::corpus::SessionQueryAbort>,
    ) -> Result<Vec<SessionRecord>, SessionQueryError> {
        self.corpus.list_sessions(signal).await
    }

    /// Read and replay-validate one complete logical session log without
    /// making it live.
    pub async fn read_session(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionLogSnapshot, SessionQueryError> {
        let loaded = self.corpus.load(session_id, None).await?;
        dsh_session::Session::create(
            session_id.clone(),
            Some(loaded.events.clone()),
            Some(&loaded.header),
            loaded.header.is_seeded.then_some(
                dsh_session::SessionLogOffset::new(0).expect("zero is a valid Session log offset"),
            ),
        )
        .map_err(|error| {
            SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryCorruptSession,
                format!("stored session \"{session_id}\" is corrupt: {error}"),
            )
        })?;
        Ok(SessionLogSnapshot {
            session: loaded.header,
            events: loaded.events.iter().cloned().collect(),
        })
    }

    /// Filter the complete logical corpus with provider-independent
    /// predicates.
    pub async fn filter_sessions(
        &self,
        filters: &[SessionResultFilter],
        signal: Option<&crate::corpus::SessionQueryAbort>,
    ) -> Result<Vec<SessionRecord>, SessionQueryError> {
        let owned = materialize_session_result_filters(filters)?;
        let records = self.corpus.list_sessions(signal).await?;
        Ok(filter_session_results(&records, &owned))
    }

    /// Fold the latest log-backed title from one live-preferred logical
    /// session.
    pub async fn read_title(
        &self,
        session_id: &SessionId,
        signal: Option<&crate::corpus::SessionQueryAbort>,
    ) -> Result<Option<dsh_session_title::SessionTitleSnapshot>, SessionQueryError> {
        Ok(self.read_title_snapshot(session_id, signal).await?.title)
    }

    /// Fold the latest title and return its source header.
    pub async fn read_title_snapshot(
        &self,
        session_id: &SessionId,
        signal: Option<&crate::corpus::SessionQueryAbort>,
    ) -> Result<SessionTitleObservation, SessionQueryError> {
        match self
            .read_title_snapshots(&[session_id.clone()], signal)
            .await?
            .into_iter()
            .next()
            .expect("one result")
        {
            SessionTitleObservationResult::Fulfilled { value, .. } => Ok(value),
            SessionTitleObservationResult::Rejected { reason, .. } => Err(SessionQueryError::new(
                SessionQueryErrorCode::SessionQuerySessionNotFound,
                reason,
            )),
        }
    }

    /// Fold titles for unique sessions from one cancellable corpus
    /// observation.
    pub async fn read_title_snapshots(
        &self,
        session_ids: &[SessionId],
        signal: Option<&crate::corpus::SessionQueryAbort>,
    ) -> Result<Vec<SessionTitleObservationResult>, SessionQueryError> {
        let project: Arc<
            dyn for<'a> Fn(&crate::corpus::LogicalSessionSource<'a>) -> SessionTitleObservation
                + Send
                + Sync,
        > = Arc::new(|source| SessionTitleObservation {
            session: source.header.clone(),
            title: fold_session_title(source.events),
        });
        Ok(self
            .corpus
            .project_many(session_ids, project, signal)
            .await?
            .into_iter()
            .map(|result| match result {
                crate::corpus::LogicalProjectionResult::Fulfilled { session_id, value } => {
                    SessionTitleObservationResult::Fulfilled { session_id, value }
                }
                crate::corpus::LogicalProjectionResult::Rejected { session_id, reason } => {
                    SessionTitleObservationResult::Rejected { session_id, reason }
                }
            })
            .collect())
    }

    /// List lightweight raw-log event records for one logical session.
    pub async fn list_events(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SessionEventRecord>, SessionQueryError> {
        let loaded = self.corpus.load(session_id, None).await?;
        Ok(build_session_event_records(session_id, &loaded.events)?)
    }

    /// Scan first-party semantic event documents with provider-independent
    /// filters.
    pub async fn filter_events(
        &self,
        session_id: &SessionId,
        filters: &[crate::types::SessionEventResultFilter],
    ) -> Result<Vec<SessionEventSearchDocument>, SessionQueryError> {
        let owned = materialize_session_event_result_filters(filters)?;
        let loaded = self.corpus.load(session_id, None).await?;
        let documents = build_session_event_search_documents(session_id, &loaded.events)?;
        Ok(filter_session_event_documents(&documents, &owned))
    }

    /// Read one session's complete current model surface from one corpus
    /// observation.
    pub async fn read_surface(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionSurfaceSnapshot, SessionQueryError> {
        let loaded = self.corpus.load(session_id, None).await?;
        Ok(SessionSurfaceSnapshot {
            session: loaded.header.clone(),
            captured_through_seq: loaded.events.last().map(|event| event.seq.get()),
            events: crate::tracing::current_surface_events(session_id, &loaded.events)?,
        })
    }

    /// Trace known ancestry and descendants from one corpus observation.
    pub async fn trace_session(
        &self,
        session_id: &SessionId,
        signal: Option<&crate::corpus::SessionQueryAbort>,
    ) -> Result<SessionLineageTrace, SessionQueryError> {
        let records = self.corpus.list_sessions(signal).await?;
        if signal.is_some_and(|signal| signal()) {
            return Err(SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryAborted,
                "session query was cancelled",
            ));
        }
        crate::tracing::trace_session(&records, session_id)
    }

    /// Trace one event's direct positional replacements and cited source
    /// events.
    pub async fn trace_event(
        &self,
        request: &SessionEventTraceRequest,
        signal: Option<&crate::corpus::SessionQueryAbort>,
    ) -> Result<SessionEventTraceObservation, SessionQueryError> {
        let loaded = self.corpus.load(&request.session_id, signal).await?;
        if signal.is_some_and(|signal| signal()) {
            return Err(SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryAborted,
                "session query was cancelled",
            ));
        }
        let trace = crate::tracing::trace_event(&request.session_id, &loaded.events, request.seq)?;
        Ok(SessionEventTraceObservation {
            session: loaded.header,
            target: trace.target,
            replaced_by: trace.replaced_by,
            replacement_chain: trace.replacement_chain,
            replaced_event_seqs: trace.replaced_event_seqs,
            source_event_seqs: trace.source_event_seqs,
            derived_event_seqs: trace.derived_event_seqs,
        })
    }

    /// Read one full event plus a bounded raw-log context window.
    pub async fn read_event(
        &self,
        request: &SessionEventReadRequest,
        signal: Option<&crate::corpus::SessionQueryAbort>,
    ) -> Result<SessionEventWindow, SessionQueryError> {
        let before = read_window("before", request.before, self.read_window_max)?;
        let after = read_window("after", request.after, self.read_window_max)?;
        let loaded = self.corpus.load(&request.session_id, signal).await?;
        if signal.is_some_and(|signal| signal()) {
            return Err(SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryAborted,
                "session query was cancelled",
            ));
        }
        let seq = request.seq;
        let target = loaded
            .events
            .get(seq as usize)
            .filter(|event| event.seq == seq)
            .ok_or_else(|| {
                SessionQueryError::new(
                    SessionQueryErrorCode::SessionQueryEventNotFound,
                    format!(
                        "session \"{}\" has no event at seq {seq}",
                        request.session_id
                    ),
                )
            })?;
        let start_seq = seq.saturating_sub(before).max(0);
        let end_seq = seq
            .saturating_add(after)
            .min(loaded.events.len().saturating_sub(1) as u64);
        let target_snapshot = target.clone();
        let events: Vec<dsh_session::SessionEvent> = loaded.events
            [start_seq as usize..=end_seq as usize]
            .iter()
            .map(|event| {
                if event.seq == seq {
                    target_snapshot.clone()
                } else {
                    event.clone()
                }
            })
            .collect();
        Ok(SessionEventWindow {
            session: loaded.header,
            target: target_snapshot,
            events,
            start_seq,
            end_seq,
        })
    }
}

fn read_window(name: &str, value: Option<u64>, max: u64) -> Result<u64, SessionQueryError> {
    let value = value.unwrap_or(0);
    if value > max {
        return Err(SessionQueryError::new(
            SessionQueryErrorCode::SessionQueryInvalidWindow,
            format!("{name} must be an integer between 0 and {max}"),
        ));
    }
    Ok(value)
}

/// The Cordis plugin form (the TS loader mounts the abstract class through
/// a concrete provider; this zero-backend plugin mounts the disabled-search
/// engine for headless assemblies).
pub struct SessionQueryPlugin;

#[async_trait::async_trait]
impl Plugin for SessionQueryPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("session-query")
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(["sessions"])
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config.downcast_ref::<Config>().cloned().unwrap_or_default();
        SessionQueryEngine::install(ctx, &config, None)
            .map_err(|error| PluginError::from(anyhow::anyhow!(error.message)))?;
        Ok(())
    }
}

impl cordis::Service for SessionQueryEngine {
    fn service_name(&self) -> &'static str {
        "sessionQuery"
    }
}

// Re-exported for the engine's internal use in search backends.
pub use crate::config::Config as SessionQueryConfig;
pub use crate::types::SessionEventSearchHit as SessionEventSearchHitType;
