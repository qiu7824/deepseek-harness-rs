//! The subagent seam's consumer-facing contracts: request, result, and
//! capability types for the provider trait, plus the `subagent/start` /
//! `subagent/end` observation payloads. Rust port of
//! `packages/subagent/subagent/src/types.ts` (the contract layer; the
//! runtime/continuation/registry implementation arrives in a later round).

use std::sync::Arc;

use dsh_agent::{Agent, AgentOptions};
use dsh_brand::Branded;
use dsh_llm::ContentBlock;
use dsh_session::{SessionEvent, SessionId};
use dsh_tools::ToolRestriction;

use crate::descriptor::SubagentDescriptorData;

/// The brand marker for [`SubagentRunId`].
#[doc(hidden)]
pub enum SubagentRunIdTag {}

/// Identifies one accepted subagent run across its lifecycle event pair.
pub type SubagentRunId = Branded<SubagentRunIdTag>;

/// Brand a string as a [`SubagentRunId`] (TS `SubagentRunId(id)`).
pub fn subagent_run_id(id: impl Into<String>) -> SubagentRunId {
    SubagentRunId::new(id)
}

/// Observe-only identifying detail for a published subagent run, carried by
/// `subagent/start`.
#[derive(Debug, Clone)]
pub struct SubagentRunInfo {
    /// Unique identity shared with the paired terminal event.
    pub run_id: SubagentRunId,
    /// Provider name recorded when the child was first created.
    pub provider: String,
    /// The child agent's id.
    pub id: SessionId,
    /// Snapshot of whether the run's local agent was present when start
    /// fulfilled.
    pub local: bool,
}

/// Observe-only outcome detail for a settled subagent run, carried by
/// `subagent/end`.
#[derive(Debug, Clone)]
pub struct SubagentRunEndInfo {
    pub run_id: SubagentRunId,
    pub provider: String,
    pub id: SessionId,
    pub local: bool,
    /// The terminal stop reason.
    pub stop_reason: SubagentStopReason,
    /// The child's final assistant output, absent on infrastructure
    /// rejection or when the child produced none.
    pub last_assistant_message: Option<Vec<ContentBlock>>,
}

/// Which START-TIME features a provider supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubagentCapabilities {
    pub output_schema: bool,
    pub depth_limit: bool,
    pub tool_filter: bool,
    pub persona: bool,
}

/// Cancellation from the spawning context (the TS `AbortSignal` seam).
pub type SubagentSignal = Arc<dyn Fn() -> bool + Send + Sync>;

/// What a caller asks for when starting a ONE-SHOT subagent.
#[derive(Clone)]
pub struct SubagentStartRequest {
    /// Optional short display label persisted with a session-backed child.
    pub label: Option<String>,
    /// Content delivered as the child's user message.
    pub prompt: Vec<ContentBlock>,
    /// The spawning agent.
    pub parent: Arc<dyn Agent>,
    /// Cancellation signal from the spawning context.
    pub signal: SubagentSignal,
    /// Optional child agent options.
    pub agent_options: Option<AgentOptions>,
    /// Object-rooted JSON Schema within the enforced subset.
    pub output_schema: Option<serde_json::Value>,
    /// Optional absolute delegation-depth cap.
    pub max_depth: Option<u64>,
    /// Optional child tool scoping.
    pub tool_filter: Option<ToolRestriction>,
    /// Optional per-child persona.
    pub persona: Option<String>,
}

/// Provider-facing one-shot request after the runtime resolves the durable
/// child descriptor.
#[derive(Clone)]
pub struct ResolvedSubagentStartRequest {
    pub request: SubagentStartRequest,
    /// Detached descriptor a session-backed provider persists in the child
    /// log.
    pub descriptor: SubagentDescriptorData,
}

/// What the continuation manager asks a provider for while materializing
/// one continuable child's FIRST activation.
#[derive(Clone)]
pub struct ContinuableCreateRequest {
    /// The reserved durable child session id, for provider diagnostics.
    pub session_id: SessionId,
    /// The delegating parent agent whose history a seeding provider reads.
    pub parent: Arc<dyn Agent>,
    /// Caller cancellation.
    pub signal: SubagentSignal,
}

/// A provider's detached contribution to one continuable child's creation.
#[derive(Debug, Clone, Default)]
pub struct ContinuableCreateSpec {
    /// Completed-turn prefix of the parent's log to seed the child session
    /// with, or absent for a fresh child.
    pub seed: Option<Vec<SessionEvent>>,
}

/// Why a subagent run ended (the TS merge-extensible map; the Rust enum
/// widens through future variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentStopReason {
    #[default]
    Completed,
    Aborted,
    Error,
    MaxTokens,
    Refusal,
}

impl SubagentStopReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubagentStopReason::Completed => "completed",
            SubagentStopReason::Aborted => "aborted",
            SubagentStopReason::Error => "error",
            SubagentStopReason::MaxTokens => "max-tokens",
            SubagentStopReason::Refusal => "refusal",
        }
    }
}

/// The terminal outcome of a subagent run.
#[derive(Clone)]
pub struct SubagentResult {
    /// The child's final assistant output (last non-empty assistant message
    /// content, or the accumulated assistant text stream, or `[]`).
    pub output: Vec<ContentBlock>,
    /// The structured result after a requested `outputSchema` was
    /// successfully satisfied.
    pub structured: Option<serde_json::Value>,
    /// Why the run ended.
    pub stop_reason: SubagentStopReason,
}

/// ONE-SHOT child handle returned after publication (the Rust trait form of
/// the TS `SubagentRun`).
#[async_trait::async_trait]
pub trait SubagentRun: Send + Sync + 'static {
    /// Parent-scoped run id.
    fn id(&self) -> &SessionId;
    /// The exact published in-process child, or `None` for a remote run.
    fn local_agent(&self) -> Option<Arc<dyn Agent>>;
    /// Resolves with the child's terminal result when the run settles.
    async fn result(&self) -> Result<SubagentResult, String>;
    /// Cancel remaining work, reach child quiescence, and release resources.
    async fn dispose(&self) -> Result<(), String>;
}

/// One registered transport for running child agents (the TS
/// `SubagentProvider`).
#[async_trait::async_trait]
pub trait SubagentProvider: Send + Sync + 'static {
    /// Unique registry name (e.g. `spawn`, `fork`, `acp`).
    fn name(&self) -> &str;
    /// The start-time features this provider supports.
    fn capabilities(&self) -> SubagentCapabilities;
    /// Whether the child sees the parent's completed-turn prefix.
    fn inherits_parent_context(&self) -> bool;
    /// Establish a ONE-SHOT child and return its handle after publication.
    async fn start(
        &self,
        request: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, crate::error::SubagentError>;
    /// OPTIONAL (continuable-creation capability): contribute the detached
    /// creation inputs that distinguish this provider's continuable children.
    /// Method presence IS the capability in TS; the Rust trait defaults the
    /// method to a typed rejection and a backend opting in overrides it.
    async fn prepare_continuable(
        &self,
        _request: ContinuableCreateRequest,
    ) -> Result<ContinuableCreateSpec, crate::error::SubagentError> {
        Err(crate::error::SubagentError::new(
            "SUBAGENT_NOT_CONTINUABLE",
            format!(
                "provider \"{}\" does not support continuable children",
                self.name()
            ),
        ))
    }
}
