//! Durable Tool event vocabulary shared with type-only consumers. Rust
//! port of `packages/core/tools/src/types.ts`.

use dsh_llm::CallId;
use dsh_llm::ContentBlock;
use serde::{Deserialize, Serialize};

/// Payload recorded when one nested Code Mode Tool dispatch starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeDispatchStartEventData {
    pub root_call_id: CallId,
    pub parent_call_id: CallId,
    pub sub_call_id: CallId,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Payload recorded when one nested Code Mode Tool dispatch settles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeDispatchEventData {
    pub root_call_id: CallId,
    pub parent_call_id: CallId,
    pub sub_call_id: CallId,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(rename = "isError")]
    pub is_error: bool,
    pub content: Vec<ContentBlock>,
}
