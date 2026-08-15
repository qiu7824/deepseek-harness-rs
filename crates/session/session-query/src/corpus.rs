//! Live/persisted logical-corpus resolution for session-query. Rust port of
//! `packages/session-query/session-query/src/corpus.ts`.
//!
//! # Deviations
//!
//! - The abort seam is a predicate without a reason payload; aborts surface
//!   as `SESSION_QUERY_ABORTED` ("session query was cancelled").
//! - The optional persistence binding observes the ERASED registration style
//!   (`Arc<dyn SessionPersistenceApi>`); a backend that registers its
//!   concrete type (jsonl) needs an erased alias at host assembly time.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cordis::{ArcValue, Context, InjectSpec};
use dsh_session::{Session, SessionEvent, SessionHeader, SessionId, SessionStore};
use dsh_session_persistence::{SessionInspection, SessionPersistenceApi};
use parking_lot::Mutex;

use crate::config::{SessionQueryError, SessionQueryErrorCode};
use crate::sources::assert_session_headers_compatible;
use crate::types::SessionRecord;

/// The cancellation seam (TS `AbortSignal`).
pub type SessionQueryAbort = Arc<dyn Fn() -> bool + Send + Sync>;

fn aborted(signal: Option<&SessionQueryAbort>) -> bool {
    signal.is_some_and(|signal| signal())
}

fn abort_error() -> SessionQueryError {
    SessionQueryError::new(
        SessionQueryErrorCode::SessionQueryAborted,
        "session query was cancelled",
    )
}

fn not_found(session_id: &SessionId) -> SessionQueryError {
    SessionQueryError::new(
        SessionQueryErrorCode::SessionQuerySessionNotFound,
        format!("session \"{session_id}\" not found"),
    )
}

/// Detached source selected for one exact read.
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalSession {
    pub header: SessionHeader,
    pub events: Vec<SessionEvent>,
}

/// Borrowed source visible only during one synchronous batch projection.
pub struct LogicalSessionSource<'a> {
    pub header: &'a SessionHeader,
    pub events: &'a [SessionEvent],
}

/// One source-projection result in a batch logical-corpus observation.
#[derive(Debug, Clone)]
pub enum LogicalProjectionResult<Value> {
    Fulfilled {
        session_id: SessionId,
        value: Value,
    },
    Rejected {
        session_id: SessionId,
        reason: String,
    },
}

/// Resolves a live-preferred corpus against the persistence service mounted
/// now (TS `SessionCorpus`).
pub struct SessionCorpus {
    ctx: Context,
    persistence: Arc<Mutex<Option<Arc<dyn SessionPersistenceApi>>>>,
    persisted_inspect_concurrency: usize,
    /// The optional-persistence binding fiber, disposed with the corpus
    /// owner (kept alive for the process lifetime in practice).
    _binding: parking_lot::Mutex<Option<Arc<cordis::FiberCore>>>,
}

impl SessionCorpus {
    /// Bind the optional persistence service and own the binding fiber.
    pub fn new(ctx: &Context, persisted_inspect_concurrency: usize) -> Self {
        let persistence = Arc::new(Mutex::new(None::<Arc<dyn SessionPersistenceApi>>));
        let binding = ctx.inject(
            InjectSpec::new(["sessionPersistence"]),
            Arc::new({
                let persistence = persistence.clone();
                move |type_ctx: &Context, _config: ArcValue| {
                    let type_ctx = type_ctx.clone();
                    let persistence = persistence.clone();
                    Box::pin(async move {
                        if let Some(service) = type_ctx
                            .get_typed::<Arc<dyn SessionPersistenceApi>>(
                                "sessionPersistence",
                                false,
                            )
                            .map(|slot| slot.as_ref().clone())
                        {
                            *persistence.lock() = Some(service);
                        }
                        Ok(())
                    })
                }
            }),
        );
        Self {
            ctx: ctx.clone(),
            persistence,
            persisted_inspect_concurrency,
            _binding: parking_lot::Mutex::new(Some(binding)),
        }
    }

    /// List the complete logical corpus with live precedence and cloned
    /// headers, newest-first.
    pub async fn list_sessions(
        &self,
        signal: Option<&SessionQueryAbort>,
    ) -> Result<Vec<SessionRecord>, SessionQueryError> {
        if aborted(signal) {
            return Err(abort_error());
        }
        let persisted: Vec<SessionHeader> = match self.persistence.lock().clone() {
            Some(persistence) => list_persisted(&*persistence, signal).await?,
            None => Vec::new(),
        };
        if aborted(signal) {
            return Err(abort_error());
        }
        let mut records: HashMap<String, SessionRecord> = HashMap::new();
        for header in persisted {
            records.insert(
                header.id.as_str().to_string(),
                SessionRecord {
                    header: header.clone(),
                    live: false,
                    persisted: true,
                },
            );
        }
        let sessions = self
            .ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
            .expect("sessions service");
        for session in sessions.list() {
            let durable = records.get(session.id().as_str()).cloned();
            if let Some(durable) = &durable {
                assert_session_headers_compatible(session.header(), &durable.header)?;
            }
            records.insert(
                session.id().as_str().to_string(),
                SessionRecord {
                    header: session.header().clone(),
                    live: true,
                    persisted: durable.is_some(),
                },
            );
        }
        let mut records: Vec<SessionRecord> = records.into_values().collect();
        records.sort_by(compare_sessions);
        Ok(records)
    }

    /// Load one logical source, preferring a detached live snapshot.
    pub async fn load(
        &self,
        session_id: &SessionId,
        signal: Option<&SessionQueryAbort>,
    ) -> Result<LogicalSession, SessionQueryError> {
        if aborted(signal) {
            return Err(abort_error());
        }
        let sessions = self
            .ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
            .expect("sessions service");
        if let Some(live) = sessions.get(session_id) {
            let snapshot = snapshot_live(&live);
            if aborted(signal) {
                return Err(abort_error());
            }
            return Ok(snapshot);
        }
        let persistence = self
            .persistence
            .lock()
            .clone()
            .ok_or_else(|| not_found(session_id))?;
        let listed = (list_persisted(&*persistence, signal).await?)
            .into_iter()
            .find(|header| header.id == *session_id);
        if aborted(signal) {
            return Err(abort_error());
        }
        let Some(listed) = listed else {
            return Err(not_found(session_id));
        };
        let loaded = inspect_persisted(&*persistence, session_id, signal).await?;
        if aborted(signal) {
            return Err(abort_error());
        }
        if let Some(attached) = sessions.get(session_id) {
            let snapshot = snapshot_live(&attached);
            if aborted(signal) {
                return Err(abort_error());
            }
            return Ok(snapshot);
        }
        assert_session_headers_compatible(&loaded.meta, &listed)?;
        let snapshot = LogicalSession {
            header: loaded.meta.clone(),
            events: loaded.events.clone(),
        };
        if aborted(signal) {
            return Err(abort_error());
        }
        Ok(snapshot)
    }

    /// Project unique logical sources immediately from one persistence
    /// listing (TS `projectMany`).
    pub async fn project_many<Value: Clone + Send + 'static>(
        &self,
        session_ids: &[SessionId],
        project: Arc<dyn for<'a> Fn(&LogicalSessionSource<'a>) -> Value + Send + Sync>,
        signal: Option<&SessionQueryAbort>,
    ) -> Result<Vec<LogicalProjectionResult<Value>>, SessionQueryError> {
        let mut ids: Vec<SessionId> = Vec::new();
        for id in session_ids {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        if aborted(signal) {
            return Err(abort_error());
        }
        let sessions = self
            .ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
            .expect("sessions service");
        let resolved: Arc<std::sync::Mutex<HashMap<String, LogicalProjectionResult<Value>>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let mut unresolved: Vec<SessionId> = Vec::new();
        for id in &ids {
            match sessions.get(id) {
                Some(session) => {
                    resolved.lock().expect("resolved").insert(
                        id.as_str().to_string(),
                        project_source(id, &session, &project, signal),
                    );
                }
                None => unresolved.push(id.clone()),
            }
        }
        if unresolved.is_empty() {
            return Ok(ordered_results(&ids, &resolved));
        }
        let Some(persistence) = self.persistence.lock().clone() else {
            for id in unresolved {
                let reason = not_found(&id).message;
                resolved.lock().expect("resolved").insert(
                    id.as_str().to_string(),
                    LogicalProjectionResult::Rejected {
                        session_id: id,
                        reason,
                    },
                );
            }
            return Ok(ordered_results(&ids, &resolved));
        };

        let persisted = match list_persisted(&*persistence, signal).await {
            Ok(persisted) => {
                if aborted(signal) {
                    return Err(abort_error());
                }
                persisted
            }
            Err(error) => {
                if aborted(signal) {
                    return Err(abort_error());
                }
                for id in unresolved {
                    resolved.lock().expect("resolved").insert(
                        id.as_str().to_string(),
                        LogicalProjectionResult::Rejected {
                            session_id: id,
                            reason: error.message.clone(),
                        },
                    );
                }
                return Ok(ordered_results(&ids, &resolved));
            }
        };
        let persisted_by_id: HashMap<String, SessionHeader> = persisted
            .into_iter()
            .map(|header| (header.id.as_str().to_string(), header))
            .collect();
        let cursor = Arc::new(AtomicUsize::new(0));
        let worker_count = self.persisted_inspect_concurrency.min(unresolved.len());
        let mut workers = Vec::new();
        for _ in 0..worker_count {
            let persistence = persistence.clone();
            let sessions = sessions.clone();
            let resolved = resolved.clone();
            let persisted_by_id = persisted_by_id.clone();
            let cursor = cursor.clone();
            let project = project.clone();
            let unresolved = unresolved.clone();
            let signal = signal.cloned();
            workers.push(tokio::spawn(async move {
                loop {
                    if aborted(signal.as_ref()) {
                        return;
                    }
                    let index = cursor.fetch_add(1, Ordering::Relaxed);
                    if index >= unresolved.len() {
                        return;
                    }
                    let session_id = unresolved[index].clone();
                    let result = match persisted_by_id.get(session_id.as_str()) {
                        None => {
                            let attached = sessions.get(&session_id);
                            match attached {
                                Some(attached) => {
                                    project_source(&session_id, &attached, &project, signal.as_ref())
                                }
                                None => LogicalProjectionResult::Rejected {
                                    session_id: session_id.clone(),
                                    reason: not_found(&session_id).message,
                                },
                            }
                        }
                        Some(listed) => {
                            match inspect_persisted(&*persistence, &session_id, signal.as_ref())
                                .await
                            {
                                Ok(loaded) => {
                                    if aborted(signal.as_ref()) {
                                        return;
                                    }
                                    let attached = sessions.get(&session_id);
                                    if let Some(attached) = attached {
                                        project_source(
                                            &session_id,
                                            &attached,
                                            &project,
                                            signal.as_ref(),
                                        )
                                    } else {
                                        match assert_session_headers_compatible(
                                            &loaded.meta,
                                            listed,
                                        ) {
                                            Ok(()) => project_source_with(
                                                &session_id,
                                                LogicalSessionSource {
                                                    header: &loaded.meta,
                                                    events: &loaded.events,
                                                },
                                                &project,
                                                signal.as_ref(),
                                            ),
                                            Err(error) => LogicalProjectionResult::Rejected {
                                                session_id: session_id.clone(),
                                                reason: error.message,
                                            },
                                        }
                                    }
                                }
                                Err(error) => LogicalProjectionResult::Rejected {
                                    session_id: session_id.clone(),
                                    reason: error.message,
                                },
                            }
                        }
                    };
                    resolved
                        .lock()
                        .expect("resolved")
                        .insert(session_id.as_str().to_string(), result);
                }
            }));
        }
        for worker in workers {
            worker.await.map_err(|error| {
                SessionQueryError::new(
                    SessionQueryErrorCode::SessionQueryIndexFailed,
                    format!("persisted inspection worker failed: {error}"),
                )
            })?;
        }
        if aborted(signal) {
            return Err(abort_error());
        }
        let resolved = Arc::try_unwrap(resolved)
            .map_err(|_| {
                SessionQueryError::new(
                    SessionQueryErrorCode::SessionQueryIndexFailed,
                    "persisted inspection workers still hold the result map",
                )
            })?
            .into_inner()
            .expect("unlocked");
        Ok(ordered_results(&ids, &Arc::new(std::sync::Mutex::new(resolved))))
    }
}

/// Project one LIVE source synchronously (the TS `projectSource` + live
/// branch).
fn project_source<Value>(
    session_id: &SessionId,
    session: &Session,
    project: &Arc<dyn for<'a> Fn(&LogicalSessionSource<'a>) -> Value + Send + Sync>,
    signal: Option<&SessionQueryAbort>,
) -> LogicalProjectionResult<Value> {
    project_source_with(
        session_id,
        LogicalSessionSource {
            header: session.header(),
            events: &session.events(),
        },
        project,
        signal,
    )
}

fn project_source_with<Value>(
    session_id: &SessionId,
    source: LogicalSessionSource<'_>,
    project: &Arc<dyn for<'a> Fn(&LogicalSessionSource<'a>) -> Value + Send + Sync>,
    signal: Option<&SessionQueryAbort>,
) -> LogicalProjectionResult<Value> {
    if aborted(signal) {
        return LogicalProjectionResult::Rejected {
            session_id: session_id.clone(),
            reason: abort_error().message,
        };
    }
    let value = (project)(&source);
    if aborted(signal) {
        return LogicalProjectionResult::Rejected {
            session_id: session_id.clone(),
            reason: abort_error().message,
        };
    }
    LogicalProjectionResult::Fulfilled {
        session_id: session_id.clone(),
        value,
    }
}

fn ordered_results<Value: Clone>(
    ids: &[SessionId],
    resolved: &Arc<std::sync::Mutex<HashMap<String, LogicalProjectionResult<Value>>>>,
) -> Vec<LogicalProjectionResult<Value>> {
    let map = resolved.lock().expect("resolved");
    ids.iter()
        .filter_map(|id| map.get(id.as_str()).cloned())
        .collect()
}

async fn list_persisted(
    persistence: &dyn SessionPersistenceApi,
    signal: Option<&SessionQueryAbort>,
) -> Result<Vec<SessionHeader>, SessionQueryError> {
    match persistence.list().await {
        Ok(headers) => Ok(headers),
        Err(error) => {
            if aborted(signal) {
                return Err(abort_error());
            }
            Err(SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryPersistenceFailed,
                format!("session persistence listing failed: {error}"),
            ))
        }
    }
}

async fn inspect_persisted(
    persistence: &dyn SessionPersistenceApi,
    session_id: &SessionId,
    signal: Option<&SessionQueryAbort>,
) -> Result<SessionInspection, SessionQueryError> {
    match persistence.inspect(session_id).await {
        Ok(inspection) => Ok(inspection),
        Err(error) => {
            if aborted(signal) {
                return Err(abort_error());
            }
            Err(SessionQueryError::new(
                SessionQueryErrorCode::SessionQueryPersistenceFailed,
                format!("failed to inspect session \"{session_id}\": {error}"),
            ))
        }
    }
}

fn snapshot_live(session: &Session) -> LogicalSession {
    LogicalSession {
        header: session.header().clone(),
        events: session.events().iter().cloned().collect(),
    }
}

fn compare_sessions(a: &SessionRecord, b: &SessionRecord) -> std::cmp::Ordering {
    b.header
        .created_at
        .cmp(&a.header.created_at)
        .then_with(|| a.header.id.as_str().cmp(b.header.id.as_str()))
}
