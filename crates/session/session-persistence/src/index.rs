//! Durable session-persistence Service Definition (`ctx.sessionPersistence`).
//! Rust port of `packages/session/session-persistence/src/index.ts`.
//!
//! # Deviations
//!
//! - `AbortSignal` parameters are omitted until cancellation wiring lands.
//! - The abstract class becomes the [`SessionPersistence`] struct (service
//!   registration) plus the [`SessionPersistenceApi`] trait backends
//!   implement; coordinator-backed backends delegate to
//!   [`crate::coordinator::PersistenceCoordinator`].

use std::sync::Arc;

use cordis::{Context, Service};
use dsh_session::{SessionEvent, SessionHeader, SessionId};

use crate::revision::SessionPersistenceRevision;

/// Lightweight immutable source identity returned without loading a full log.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionPersistenceSnapshot {
    /// Detached metadata for one materialized session.
    pub header: SessionHeader,
    /// Opaque source-qualified token that changes whenever this stored log
    /// changes.
    pub revision: SessionPersistenceRevision,
}

/// Immutable logical session prepared from persistence or a live owner.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInspection {
    /// Validated immutable session metadata.
    pub meta: SessionHeader,
    /// Validated contiguous logical event log.
    pub events: Vec<SessionEvent>,
}

/// A backend's own raw artifact text for one session, verbatim.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRawArtifact {
    /// The session header parsed from the artifact's own first line.
    pub meta: SessionHeader,
    /// The artifact's base filename on disk, without any physical encoding
    /// suffix.
    pub filename: String,
    /// The artifact's full text content, decoded from the backend's physical
    /// encoding.
    pub content: String,
}

/// A backend-resolved, per-session local artifact location.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLocation {
    /// Backend-specific artifact kind, for example `jsonl`.
    pub kind: String,
    /// Absolute path to this session's backend-owned artifact.
    pub path: String,
}

/// The read-from-seq result shape (TS inline return type).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionReadFromResult {
    pub meta: SessionHeader,
    pub events: Vec<SessionEvent>,
}

/// One bounded forward event chunk for streaming consumers.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEventChunk {
    pub events: Vec<SessionEvent>,
    /// Seq to request next; `None` when the stored log ended in this chunk.
    pub next_seq: Option<u64>,
}

/// One bounded, backwards history read request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionReadWindowRequest {
    pub before_seq: Option<u64>,
    pub max_messages: u64,
    pub max_events: usize,
}

/// One bounded forward history read starting at an indexed event sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionReadForwardWindowRequest {
    pub after_seq: u64,
    pub max_messages: u64,
    pub max_events: usize,
}

/// One message-aligned history window. An oversized safe group returns no
/// events and reports the required count instead of silently cutting it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionReadWindowResult {
    pub meta: SessionHeader,
    pub events: Vec<SessionEvent>,
    pub has_more: bool,
    pub oversized_event_count: Option<usize>,
}

/// Fixed-size session-list metadata, folded without retaining event payloads.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionListMetadata {
    pub meta: SessionHeader,
    pub last_seq: i64,
    pub blank: bool,
    pub updated_at: i64,
}

/// User-message projection input and watermark from one immutable read.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionUserMessageEvents {
    pub meta: SessionHeader,
    pub last_seq: i64,
    pub events: Vec<SessionEvent>,
}

/// Durable append-only session storage. Implementations preserve contiguous,
/// losslessly JSON-serializable events; `append` resolves only after
/// durability, and `load` balances a complete interrupted tail without
/// rewriting committed events.
pub struct SessionPersistence {
    pub ctx: Context,
}

impl SessionPersistence {
    /// Register the `sessionPersistence` service base.
    pub fn new(ctx: &Context) -> Arc<Self> {
        let service = Arc::new(Self { ctx: ctx.clone() });
        ctx.register_service(service.clone());
        service
    }
}

impl Service for SessionPersistence {
    fn service_name(&self) -> &'static str {
        "sessionPersistence"
    }
}

impl Service for dyn SessionPersistenceApi {
    fn service_name(&self) -> &'static str {
        "sessionPersistence"
    }
}

/// The backend contract (TS abstract members + defaults).
#[async_trait::async_trait]
pub trait SessionPersistenceApi: Send + Sync {
    /// Resolve this backend's independent local artifact for a session
    /// without reading, creating, flushing, or materializing it.
    fn locate(&self, meta: &SessionHeader) -> Option<SessionLocation>;

    /// Whether this backend exposes one verbatim raw artifact per session.
    fn supports_raw_artifacts(&self) -> bool;

    /// Read a session's backend-owned artifact text verbatim.
    async fn read_raw(&self, id: &SessionId) -> Result<Option<SessionRawArtifact>, String> {
        let _ = id;
        Err("this session persistence backend does not expose raw artifacts".to_string())
    }

    /// Register a new session's metadata (lazy materialization allowed).
    async fn create(&self, meta: SessionHeader) -> Result<(), String>;

    /// Durably persist a batch of events.
    async fn append(&self, id: &SessionId, events: &[SessionEvent]) -> Result<(), String>;

    /// Permanently remove one detached session and all backend-owned artifacts.
    async fn delete(&self, id: &SessionId) -> Result<bool, String> {
        let _ = id;
        Err("this session persistence backend does not support deletion".to_string())
    }

    /// Prepare the exact unpublished Session used by resume (default: load +
    /// `SessionStore::prepare` with `seedSource: 'persistence'`).
    async fn prepare(&self, id: &SessionId) -> Result<dsh_session::SessionPreparation, String> {
        let loaded = self.load(id).await?;
        let sessions: Arc<Arc<dsh_session::SessionStore>> =
            self.ctx().get_typed("sessions", false).ok_or_else(|| {
                "cannot prepare a session: SessionStore is not configured".to_string()
            })?;
        let session = sessions.prepare(
            Some(id.clone()),
            Some(dsh_session::CreateSessionOptions {
                seed: Some(loaded.events.clone()),
                meta: Some(dsh_session::CreateSessionMeta {
                    cwd: loaded.meta.cwd.clone(),
                    parent_session: loaded.meta.parent_session.clone(),
                    created_at: Some(loaded.meta.created_at),
                    seed_length: loaded.meta.seed_length,
                    origin: loaded.meta.origin.clone(),
                    delegation_depth: loaded.meta.delegation_depth,
                    agent_preset: loaded.meta.agent_preset.clone(),
                }),
            }),
        )?;
        Ok(dsh_session::SessionPreparation::create(
            session,
            dsh_session::SessionPreparationOptions::default(),
        ))
    }

    /// Load an immutable balanced logical view and commit any required cold
    /// recovery.
    async fn load(&self, id: &SessionId) -> Result<SessionInspection, String>;

    /// Inspect an immutable logical session without committing recovery or
    /// publishing it.
    async fn inspect(&self, id: &SessionId) -> Result<SessionInspection, String>;

    /// Read the stored events from `fromSeq` onward.
    async fn read_from(
        &self,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<SessionReadFromResult, String>;

    /// Read at most `max_events` contiguous events from `from_seq`.
    async fn read_event_chunk(
        &self,
        id: &SessionId,
        from_seq: u64,
        max_events: usize,
    ) -> Result<SessionEventChunk, String> {
        if max_events == 0 {
            return Err("event chunk max_events must be positive".to_string());
        }
        let mut whole = self.read_from(id, from_seq).await?;
        let has_more = whole.events.len() > max_events;
        whole.events.truncate(max_events);
        let next_seq = has_more.then(|| from_seq.saturating_add(whole.events.len() as u64));
        Ok(SessionEventChunk {
            events: whole.events,
            next_seq,
        })
    }

    /// Read a bounded, message-aligned forward page for indexed jumps.
    /// Backends should override this to stop decoding when the page is full.
    async fn read_forward_window(
        &self,
        id: &SessionId,
        request: SessionReadForwardWindowRequest,
    ) -> Result<SessionReadWindowResult, String> {
        let chunk = self
            .read_event_chunk(id, request.after_seq, request.max_events)
            .await?;
        let mut messages = 0_u64;
        let mut end = chunk.events.len();
        for (index, event) in chunk.events.iter().enumerate() {
            if matches!(event.type_.as_str(), "user/message" | "assistant/message")
                && event.surface_op.as_ref().is_none_or(|op| op.is_append())
            {
                messages += 1;
                if messages >= request.max_messages.max(1) {
                    end = index + 1;
                    break;
                }
            }
        }
        let has_more = end < chunk.events.len() || chunk.next_seq.is_some();
        let meta = self.read_list_metadata(id).await?.meta;
        Ok(SessionReadWindowResult {
            meta,
            events: chunk.events.into_iter().take(end).collect(),
            has_more,
            oversized_event_count: None,
        })
    }

    /// Visit a stored log in bounded forward chunks. Backends may override
    /// this to keep one physical reader open across the complete pass.
    async fn visit_event_chunks(
        &self,
        id: &SessionId,
        max_events: usize,
        visitor: Arc<dyn for<'a> Fn(&'a [SessionEvent]) -> Result<(), String> + Send + Sync>,
    ) -> Result<(), String> {
        let mut from_seq = 0;
        loop {
            let chunk = self.read_event_chunk(id, from_seq, max_events).await?;
            if !chunk.events.is_empty() {
                visitor(&chunk.events)?;
            }
            match chunk.next_seq {
                Some(next) => from_seq = next,
                None => return Ok(()),
            }
        }
    }

    /// Read only human-authored user messages for lightweight navigation
    /// projections. Backends should override this to skip packed assistant runs.
    async fn read_user_message_events(
        &self,
        id: &SessionId,
    ) -> Result<SessionUserMessageEvents, String> {
        let whole = self.read_from(id, 0).await?;
        let last_seq = whole
            .events
            .last()
            .map(|event| event.seq as i64)
            .unwrap_or(-1);
        Ok(SessionUserMessageEvents {
            meta: whole.meta,
            last_seq,
            events: whole
                .events
                .into_iter()
                .filter(|event| event.type_ == "user/message")
                .collect(),
        })
    }

    /// Read a bounded, message-aligned history window. Backends should
    /// override this to avoid materializing the full log; the default keeps
    /// compatibility while preserving the explicit event-budget contract.
    async fn read_window(
        &self,
        id: &SessionId,
        request: SessionReadWindowRequest,
    ) -> Result<SessionReadWindowResult, String> {
        let whole = self.read_from(id, 0).await?;
        let mut messages = request.max_messages.max(1);
        loop {
            match crate::select_history_window(
                &whole.events,
                request.before_seq,
                messages,
                request.max_events,
            ) {
                Ok(selection) => {
                    return Ok(SessionReadWindowResult {
                        meta: whole.meta,
                        events: whole.events[selection.start..selection.end].to_vec(),
                        has_more: selection.has_more,
                        oversized_event_count: None,
                    });
                }
                Err(error) if messages > 1 => {
                    let required = error.selection.event_count().max(1);
                    let proportional = messages
                        .saturating_mul(request.max_events as u64)
                        .checked_div(required as u64)
                        .unwrap_or(1)
                        .max(1);
                    messages = proportional.min(messages - 1);
                }
                Err(error) => {
                    return Ok(SessionReadWindowResult {
                        meta: whole.meta,
                        events: Vec::new(),
                        has_more: error.selection.has_more,
                        oversized_event_count: Some(error.selection.event_count()),
                    });
                }
            }
        }
    }

    /// Fixed-size list metadata. Backends override this to avoid retaining a
    /// full event log; the default preserves compatibility.
    async fn read_list_metadata(&self, id: &SessionId) -> Result<SessionListMetadata, String> {
        let whole = self.read_from(id, 0).await?;
        let blank = !whole.events.iter().any(|event| event.type_ == "turn/start");
        let updated_at = whole
            .events
            .iter()
            .rev()
            .find(|event| event.type_ == "user/message")
            .map(|event| event.time)
            .unwrap_or(whole.meta.created_at as i64);
        Ok(SessionListMetadata {
            last_seq: whole
                .events
                .last()
                .map(|event| event.seq as i64)
                .unwrap_or(-1),
            meta: whole.meta,
            blank,
            updated_at,
        })
    }

    /// Internal fixed-size model-selection projection state. Backends override
    /// this without materializing the full log.
    async fn read_model_selection_state(
        &self,
        id: &SessionId,
    ) -> Result<Option<serde_json::Value>, String> {
        let _ = id;
        Ok(None)
    }

    /// Lightweight listing from metadata, without a full-log parse.
    async fn list(&self) -> Result<Vec<SessionHeader>, String>;

    /// Read one materialized session's cheap change token.
    async fn read_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionPersistenceSnapshot>, String> {
        Ok(self
            .list_snapshots()
            .await?
            .into_iter()
            .find(|snapshot| snapshot.header.id == *id))
    }

    /// List materialized sessions with cheap per-log change tokens.
    async fn list_snapshots(&self) -> Result<Vec<SessionPersistenceSnapshot>, String>;

    /// The service's context (for the default `prepare`).
    fn ctx(&self) -> &Context;
}
