//! Agent service: live registry, factory delegation, and process-local
//! initiator scope. Concrete creation and driving belong to the loop.
//! Rust port of `@deepseek-ai/dsh-agent`.

pub mod consumed_work;
pub mod dispatch;
pub mod inbox;
pub mod invariant;
pub mod model_selection;
pub mod registry;
pub mod runtime_types;
pub mod types;

pub use consumed_work::{ConsumedWork, fold_consumed_work};
pub use dispatch::{
    AgentEventDispatch, agent_carrier, assemble_context_for, emit_agent_event,
};
pub use inbox::{Inbox, InboxNotifications};
pub use model_selection::{
    ModelSelection, ModelSelectionRef, install_model_selection,
};
pub use registry::AgentRegistry;
pub use runtime_types::{
    Agent, AgentErrorPayload, AgentFactory, AgentHandle, AgentInboxClaimedPayload,
    AgentInboxMessagePayload, AgentLifecyclePayload, AgentOptions, AgentPreStepPayload,
    AgentRequestConfig, AgentRequestErrorPayload, AgentRequestPayload, AgentSessionStartPayload,
    AgentSetup, AgentSetupCommit, AgentStatus, AgentStatusPayload, AgentTurnStoppingPayload,
    CancelOptions, CancellationSignal, CreateAgentOptions, PreStepDecision, RequestErrorAction,
    ResumeAgentOptions, SessionStartSource,
};
pub use types::{InboxSplice, InboxSpliceOutcome, InboxTarget, inbox_splice_of};

// The canonical cancel-cause type lives in dsh-session; re-export it for the
// Agent API surface.
pub use dsh_session::AgentCancelCause;
