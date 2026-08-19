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

    /// Lightweight listing from metadata, without a full-log parse.
    async fn list(&self) -> Result<Vec<SessionHeader>, String>;

    /// List materialized sessions with cheap per-log change tokens.
    async fn list_snapshots(&self) -> Result<Vec<SessionPersistenceSnapshot>, String>;

    /// The service's context (for the default `prepare`).
    fn ctx(&self) -> &Context;
}
