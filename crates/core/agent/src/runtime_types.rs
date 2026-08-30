//! Public agent types and the live-runtime event vocabulary. Rust port of
//! `packages/core/agent/src/runtime-types.ts`.
//!
//! # Deviations
//!
//! - `Agent` is a trait object handle (`Arc<dyn Agent>`) instead of the TS
//!   interface; `scope_key()` exposes the agent's dsh-scope identity.
//! - `runMaintenance` erases its generic result (Rust `BoxFuture<'static,
//!   ()>`); the loop re-types at its call sites.
//! - `AbortSignal` parameters are omitted until the cancellation-signal
//!   wiring lands with the loop.
//! - `AssembleContext.agent` (the TS declaration merge) is represented by
//!   scalar `sessionId`/`provider`/`model`/`cwd` fields plus the Agent scope,
//!   avoiding a live trait object inside the prompt assembly value.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{BoxFuture, Context};
use dsh_llm::{LlmCallConfig, LlmFailure, ResolvedRetryPolicy};
use dsh_scope::ScopeKey;
use dsh_session::{AgentCancelCause, Session, SessionId, UserMessage};
use parking_lot::Mutex;

use crate::inbox::Inbox;
use crate::types::InboxTarget;

/// Cancellation signal standing in for TS `AbortSignal`. The loop carries an
/// abort reason (the agent cancel cause) beside the raw flag; `llm-retry`
/// consumes only the flag.
pub struct CancellationSignal {
    aborted: AtomicBool,
    reason: Mutex<Option<AgentCancelCause>>,
    notify: tokio::sync::Notify,
}

impl Default for CancellationSignal {
    fn default() -> Self {
        Self {
            aborted: AtomicBool::new(false),
            reason: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        }
    }
}

impl CancellationSignal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Abort without a durable reason (plain cancellation).
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Abort carrying the loop's cancel cause.
    pub fn abort_with(&self, reason: AgentCancelCause) {
        *self.reason.lock() = Some(reason);
        self.aborted.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }

    /// The durable abort reason, when one was supplied.
    pub fn reason(&self) -> Option<AgentCancelCause> {
        self.reason.lock().clone()
    }

    /// Resolve when the signal aborts.
    pub async fn cancelled(&self) {
        loop {
            if self.aborted() {
                return;
            }
            self.notify.notified().await;
        }
    }
}

/// The `agent/request-error` waterfall payload (the TS `Events` parameter;
/// published by dsh-agent-loop and consumed by dsh-llm-retry).

/// Merge-extensible agent creation options. Persona belongs to system-prompt
/// sections.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentOptions {
    /// Provider route (must have a registered adapter at call time).
    pub provider: Option<String>,
    /// Model id interpreted by the selected provider adapter.
    pub model: Option<String>,
    /// Maximum output tokens for each conversation-model request.
    pub max_tokens: Option<u64>,
    /// Adapter-owned reasoning effort applied to this Agent's requests.
    pub reasoning_effort: Option<dsh_llm::ReasoningEffortId>,
    /// Delegation depth: zero for a top-level agent and parent depth + 1 for
    /// a child (the TS module augmentation on `AgentOptions`).
    pub subagent_depth: Option<u64>,
}

/// Options for [`Agent::cancel`].
#[derive(Debug, Clone, Default)]
pub struct CancelOptions {
    /// Preserve queued and steering inbox items instead of discarding them.
    pub keep_inbox: bool,
}

/// An agent's lifecycle state (`idle` / `running`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Running,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Running => "running",
        }
    }
}

/// Whether and with which messages the loop enters a proposed step.
#[derive(Debug, Clone, PartialEq)]
pub enum PreStepDecision {
    Reject,
    Enter { messages: Vec<UserMessage> },
}

/// Action returned by a listener that owns model-request recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestErrorAction {
    Retry,
}

/// Why a session lifecycle began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStartSource {
    Startup,
    Resume,
    Clear,
    Compact,
}

/// Public live-agent handle.
pub trait Agent: Send + Sync + 'static {
    /// The single identity shared with [`Agent::session`].
    fn id(&self) -> &SessionId;
    /// The provider route and model this agent's requests use.
    fn options(&self) -> &AgentOptions;
    /// The live session this agent drives.
    fn session(&self) -> &Session;
    /// The agent-owned projection of durable pending work.
    fn inbox(&self) -> &Inbox;
    /// The current lifecycle state.
    fn status(&self) -> AgentStatus;
    /// Agent-scoped context; its contributions are agent-local.
    fn ctx(&self) -> &Context;
    /// The agent's dsh-scope identity (TS uses the agent object itself as
    /// its `scopeTarget(agent, agent)` key).
    fn scope_key(&self) -> &ScopeKey;

    /// Clear queued and steering work — unless `keepInbox` — and abort the
    /// active turn or between-turn task.
    fn cancel(&self, cause: AgentCancelCause, options: Option<&CancelOptions>);

    /// Resolve after the current whole-agent activity reaches quiescence.
    fn when_idle(&self) -> BoxFuture<'static, ()>;

    /// Run one non-turn maintenance task from the true idle phase (result
    /// type erased — see the module deviation notes).
    fn run_maintenance(
        &self,
        task: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()>;

    /// Route identified input to an inbox boundary and optionally wake the
    /// driver.
    fn send(&self, message: UserMessage, target: InboxTarget, wakeup: bool);

    /// Queue an ordinary follow-up turn and wake the driver.
    fn followup(&self, message: UserMessage);

    /// Submit steering for the nearest step.
    fn steer(&self, message: UserMessage);

    /// Queue model-facing context for the next pre-step without waking the
    /// driver.
    fn inject(&self, message: UserMessage);
}

// ---- event payloads ----

/// `agent/created` / `agent/disposed` payload.
#[derive(Clone)]
pub struct AgentLifecyclePayload {
    pub agent: Arc<dyn Agent>,
}

/// `agent/status` payload.
#[derive(Clone)]
pub struct AgentStatusPayload {
    pub agent: Arc<dyn Agent>,
    pub status: AgentStatus,
}

/// `agent/inbox/inserted` / `agent/inbox/discarded` payload.
#[derive(Clone)]
pub struct AgentInboxMessagePayload {
    pub agent: Arc<dyn Agent>,
    pub message: UserMessage,
}

/// `agent/inbox/claimed` payload.
#[derive(Clone)]
pub struct AgentInboxClaimedPayload {
    pub agent: Arc<dyn Agent>,
    pub message: UserMessage,
    pub turn: u64,
}

/// `agent/session-start` payload.
#[derive(Clone)]
pub struct AgentSessionStartPayload {
    pub agent: Arc<dyn Agent>,
    pub source: SessionStartSource,
}

/// `agent/pre-step` payload.
#[derive(Clone)]
pub struct AgentPreStepPayload {
    pub agent: Arc<dyn Agent>,
    pub messages: Vec<UserMessage>,
    pub turn: u64,
    pub step: u64,
}

/// `agent/request` payload.
#[derive(Clone)]
pub struct AgentRequestPayload {
    pub agent: Arc<dyn Agent>,
    pub turn: u64,
    pub step: u64,
}

/// The `agent/request-error` waterfall payload (the TS `Events` parameter).
#[derive(Clone)]
pub struct AgentRequestErrorPayload {
    pub agent: Arc<dyn Agent>,
    pub turn: u64,
    pub step: u64,
    pub provider: String,
    pub failure: LlmFailure,
    pub retry_policy: Option<ResolvedRetryPolicy>,
    pub signal: Arc<CancellationSignal>,
}

/// `agent/turn-stopping` payload.
#[derive(Clone)]
pub struct AgentTurnStoppingPayload {
    pub agent: Arc<dyn Agent>,
    pub turn: u64,
}

/// `agent/error` payload.
#[derive(Clone)]
pub struct AgentErrorPayload {
    pub agent: Arc<dyn Agent>,
    pub turn: u64,
    pub step: u64,
    pub error: serde_json::Value,
}

/// Synchronous finalizer returned by unpublished Agent setup.
pub trait AgentSetupCommit: Send + Sync {
    /// Validate and commit the prepared setup immediately before
    /// publication.
    fn commit(&self);
}

/// Compose an unpublished Agent scope (TS `AgentSetup`).
pub type AgentSetup = Arc<
    dyn Fn(
            &Context,
            Arc<dyn Agent>,
        ) -> BoxFuture<'static, Result<Option<Arc<dyn AgentSetupCommit>>, String>>
        + Send
        + Sync,
>;

/// Options for programmatically creating an agent through the registry
/// factory.
#[derive(Default)]
pub struct CreateAgentOptions {
    /// The live agent/session identity.
    pub session_id: Option<SessionId>,
    /// Session creation metadata (durable session data, validated by the
    /// session boundary).
    pub meta: Option<dsh_session::CreateSessionMeta>,
    /// Initial replay/fork history.
    pub seed: Option<Vec<dsh_session::SessionEvent>>,
    /// Per-agent options (model, …).
    pub agent_options: Option<AgentOptions>,
    /// Creation-time composition of the agent's scoped world.
    pub setup: Option<AgentSetup>,
}

/// Options for resuming an agent on a persisted session.
#[derive(Default)]
pub struct ResumeAgentOptions {
    /// The persisted session id to load and use as the live agent/session
    /// identity.
    pub resume_session_id: Option<SessionId>,
    /// Per-agent options (model, …).
    pub agent_options: Option<AgentOptions>,
    /// Resume-time composition of the agent's fresh scoped world.
    pub setup: Option<AgentSetup>,
}

/// An owned agent plus its disposer (TS `AgentHandle`).
pub struct AgentHandle {
    pub agent: Arc<dyn Agent>,
    /// The owner-only teardown capability.
    pub dispose: BoxFuture<'static, ()>,
}

impl std::fmt::Debug for AgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHandle")
            .field("agent", &self.agent.id())
            .finish_non_exhaustive()
    }
}

/// The agent-creation factory the loop implementation provides to the
/// registry.
#[async_trait::async_trait]
pub trait AgentFactory: Send + Sync + 'static {
    /// Whether this factory structurally owns the exact live Agent lifecycle.
    fn can_retire(&self, _agent: &Arc<dyn Agent>) -> bool {
        false
    }

    /// Retire one exact Agent through its structural factory owner.
    async fn retire(&self, _agent: Arc<dyn Agent>) -> Result<bool, String> {
        Ok(false)
    }

    /// Create a new agent on a caller-supplied session id.
    async fn create_agent(
        &self,
        owner_ctx: &Context,
        options: CreateAgentOptions,
    ) -> Result<AgentHandle, String>;

    /// Prepare a persisted session and resume an agent on it.
    async fn resume(
        &self,
        owner_ctx: &Context,
        options: ResumeAgentOptions,
    ) -> Result<AgentHandle, String>;
}

/// The request config the `agent/request` waterfall resolves (TS
/// `LlmCallConfig`; re-exported for the model-selection listener).
pub type AgentRequestConfig = LlmCallConfig;
