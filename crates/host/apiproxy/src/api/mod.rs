//! api/ contract layer: zero cordis dependencies, importable from any
//! carrier (the Rust counterpart of the TS browser-importable contract
//! layer).

pub mod agent_presets;
pub mod approvals;
pub mod credentials;
pub mod downloads;
pub mod events;
pub mod goals;
pub mod host;
pub mod jobs;
pub mod llm;
pub mod questions;
pub mod rpc;
pub mod rpc_map;
pub mod sessions;
pub mod settings;
pub mod skills;
pub mod subagents;
pub mod workspace;
