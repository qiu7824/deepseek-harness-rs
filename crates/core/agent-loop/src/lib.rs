//! Agent loop scheduler, request reconstruction, and runtime-context
//! projection: Rust port of `@deepseek-ai/dsh-agent-loop` (constants,
//! runtime-context projection, the invariant companion, the parallel
//! tool-call scheduler, and the concrete loop agent; the AgentLoop
//! service/registry wiring arrives next).

pub mod agent;
pub mod constants;
pub mod index;
pub mod invariant;
pub mod runtime_context;
pub mod tool_calls;

pub use agent::{LoopCancelled, ReactLoopAgent};
pub use constants::DEFAULT_MAX_PARALLEL_TOOL_CALLS;
pub use index::{
    CONFIGURED_AGENT_IDENTITIES_KEY, AgentLoop, Config, ConfiguredAgent,
    agent_loop_settings_namespace, agent_loop_settings_schema,
};
pub use invariant::{
    LlmLoopInvariantPlugin, NAME as AGENT_LOOP_INVARIANT_NAME,
    PACKAGE_NAME as AGENT_LOOP_INVARIANT_PACKAGE_NAME, apply as apply_agent_loop_invariant,
    installer as agent_loop_invariant_installer,
};
pub use runtime_context::RuntimeContextProjection;
pub use tool_calls::{ContextAcceptor, execute_tool_calls};
