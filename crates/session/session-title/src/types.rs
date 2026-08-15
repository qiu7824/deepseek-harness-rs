//! Pure domain types of the session-title seam. Rust port of
//! `packages/session/session-title/src/types.ts` + the type layer of
//! `src/index.ts`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use dsh_brand::Branded;
use dsh_session::Session;

/// Marker for the session-title provider id brand.
#[doc(hidden)]
#[allow(dead_code)]
pub enum SessionTitleProviderIdTag {}

/// Identifies one session-title provider registration (TS
/// `SessionTitleProviderId`).
pub type SessionTitleProviderId = Branded<SessionTitleProviderIdTag>;

/// Brand a raw provider id (TS `SessionTitleProviderId(id)`).
pub fn session_title_provider_id(id: impl Into<String>) -> SessionTitleProviderId {
    SessionTitleProviderId::new(id)
}

/// Exact auxiliary model route that produced a title (TS
/// `SessionTitleModelProvenance`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTitleModelProvenance {
    pub provider: String,
    pub model: String,
}

/// Durable ownership record for an accepted session title (TS
/// `SessionTitleSource`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SessionTitleSource {
    Fallback,
    Provider {
        provider: SessionTitleProviderId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<SessionTitleModelProvenance>,
    },
    User,
}

impl SessionTitleSource {
    pub fn kind(&self) -> &'static str {
        match self {
            SessionTitleSource::Fallback => "fallback",
            SessionTitleSource::Provider { .. } => "provider",
            SessionTitleSource::User => "user",
        }
    }
}

/// Payload of the log-only `session/title` event (TS
/// `SessionTitleEventData`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTitleEventData {
    /// Normalized non-empty title text.
    pub title: String,
    /// Exact human `user/message` seqs used to derive this title; empty for
    /// an explicit user rename.
    #[serde(rename = "messageSeqs")]
    pub message_seqs: Vec<u64>,
    /// Whether the built-in fallback, a registered provider, or the user
    /// supplied the title.
    pub source: SessionTitleSource,
}

/// Latest folded title plus the title event's durable envelope facts (TS
/// `SessionTitleSnapshot`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTitleSnapshot {
    pub title: String,
    pub message_seqs: Vec<u64>,
    pub source: SessionTitleSource,
    /// Seq of the latest `session/title` event.
    pub event_seq: u64,
    /// Timestamp of the latest `session/title` event.
    pub updated_at: i64,
}

/// Required deterministic fallback and accepted-title limits (TS `Config`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Maximum whitespace-delimited words in the built-in fallback.
    pub fallback_max_words: u64,
    /// Maximum UTF-8 bytes in the built-in fallback.
    pub fallback_max_bytes: u64,
    /// Maximum UTF-8 bytes in any accepted title.
    pub max_title_bytes: u64,
}

/// Rejection of an explicit user title whose text normalizes to empty (TS
/// `SessionTitleInvalidError`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTitleInvalidError {
    pub message: String,
}

impl SessionTitleInvalidError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl fmt::Display for SessionTitleInvalidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessionTitleInvalidError {}

/// One eligible human text message exposed to title providers (TS
/// `SessionTitleUserMessage`; serialized in the JSON-framed model prompt).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTitleUserMessage {
    /// Source `user/message` event seq.
    pub seq: u64,
    /// Exact concatenated text-block content.
    pub text: String,
}

/// Automatic generation cadence owned by a registered provider (TS
/// `SessionTitleAutomaticMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionTitleAutomaticMode {
    FirstPrompt,
    AllPrompts,
}

impl SessionTitleAutomaticMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionTitleAutomaticMode::FirstPrompt => "first-prompt",
            SessionTitleAutomaticMode::AllPrompts => "all-prompts",
        }
    }
}

/// A cancellation signal carrying an optional String abort reason.
///
/// This is the seam-local stand-in for the TS `AbortController` /
/// `AbortSignal.any` composition: `abort()` records the FIRST reason (like
/// `AbortSignal.any` adopting the first abort), and a fused signal's
/// `is_aborted()` predicate scans its upstream sources SYNCHRONOUSLY (the
/// TS `AbortSignal.any` aborted predicate is synchronous, so supersession
/// must be observable without yielding). Fused construction lives in
/// `SessionTitleService::compose_signal`.
pub struct SessionTitleSignal {
    inner: Arc<SignalInner>,
}

struct SignalInner {
    aborted: AtomicBool,
    reason: Mutex<Option<String>>,
    notify: Notify,
    /// Fused upstreams (empty for a plain controller signal); the
    /// synchronous predicate and reason scan these in order.
    sources: Vec<SessionTitleSignal>,
}

impl Clone for SessionTitleSignal {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl SessionTitleSignal {
    /// A signal that never aborts.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SignalInner {
                aborted: AtomicBool::new(false),
                reason: Mutex::new(None),
                notify: Notify::new(),
                sources: Vec::new(),
            }),
        }
    }

    /// Build a fused view over this controller and its upstreams: the
    /// result aborts when this signal or any upstream aborts.
    pub fn fused_with(&self, sources: Vec<SessionTitleSignal>) -> SessionTitleSignal {
        let mut all = Vec::with_capacity(sources.len() + 1);
        all.push(self.clone());
        all.extend(sources);
        SessionTitleSignal {
            inner: Arc::new(SignalInner {
                aborted: AtomicBool::new(false),
                reason: Mutex::new(None),
                notify: Notify::new(),
                sources: all,
            }),
        }
    }

    /// Abort this signal (idempotent); the first reason wins (TS
    /// `AbortSignal.any` semantics).
    pub fn abort(&self, reason: impl Into<String>) {
        if self.is_aborted() {
            return;
        }
        *self.inner.reason.lock() = Some(reason.into());
        self.inner.aborted.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    /// Synchronous aborted predicate over this signal and its fused
    /// upstreams.
    pub fn is_aborted(&self) -> bool {
        if self.inner.aborted.load(Ordering::SeqCst) {
            return true;
        }
        self.inner.sources.iter().any(SessionTitleSignal::is_aborted)
    }

    /// The first abort reason, when aborted (own reason first, then fused
    /// upstreams in order).
    pub fn abort_reason(&self) -> Option<String> {
        if self.inner.aborted.load(Ordering::SeqCst) {
            return self.inner.reason.lock().clone();
        }
        for source in &self.inner.sources {
            if let Some(reason) = source.abort_reason() {
                return Some(reason);
            }
        }
        None
    }

    /// Resolve once aborted (spurious-safe loop).
    pub async fn cancelled(&self) {
        loop {
            if self.is_aborted() {
                return;
            }
            self.inner.notify.notified().await;
        }
    }
}

/// Immutable input supplied to one title-provider call (TS
/// `SessionTitleProviderRequest`).
#[derive(Clone)]
pub struct SessionTitleProviderRequest {
    /// Live session being titled.
    pub session: Session,
    /// All eligible human messages through this generation revision.
    pub messages: Vec<SessionTitleUserMessage>,
    /// Exact current logged main-request route, when one has been recorded.
    pub route: Option<SessionTitleModelProvenance>,
    /// Cancellation for supersession, disposal, timeout composition, or the
    /// explicit caller.
    pub signal: SessionTitleSignal,
}

/// Provider output before service-owned normalization and log acceptance
/// (TS `SessionTitleProviderResult`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTitleProviderResult {
    /// Proposed title text.
    pub title: String,
    /// Exact seqs from `request.messages` used by this result.
    pub message_seqs: Vec<u64>,
    /// Auxiliary LLM route, when generation used a model.
    pub model: Option<SessionTitleModelProvenance>,
}

/// A provider generation failure carrying its human-readable message (the
/// TS thrown `Error`; capability-owned code/timeout fields live in the
/// LLM-backed provider layer and collapse to the message here).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTitleError {
    pub message: String,
}

impl SessionTitleError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl fmt::Display for SessionTitleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessionTitleError {}

/// One optional asynchronous title implementation registered with the
/// service (TS `SessionTitleProvider`).
#[async_trait::async_trait]
pub trait SessionTitleProvider: Send + Sync + 'static {
    /// Stable id of the provider recorded with the title.
    fn id(&self) -> &SessionTitleProviderId;
    /// When new human prompts start automatic generation.
    fn automatic(&self) -> SessionTitleAutomaticMode;
    /// Produce one title revision.
    async fn generate(
        &self,
        request: SessionTitleProviderRequest,
    ) -> Result<SessionTitleProviderResult, SessionTitleError>;
}
