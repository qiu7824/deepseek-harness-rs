//! Default model selection for an Agent without a session-specific
//! selection. Rust port of `@deepseek-ai/dsh-agent-default-model`.

pub mod index;
pub mod invariant;

pub use index::{
    AgentDefaultModelConfig, AgentDefaultModelConfigService, AgentDefaultModelSettings,
    agent_default_model_settings_namespace, agent_default_model_settings_schema,
    selection_from_data,
};
