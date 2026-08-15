//! Durable retry event payloads. Rust port of
//! `packages/llm/llm-retry/src/types.ts`.

use serde::{Deserialize, Serialize};

use crate::brand::RetryId;
use dsh_llm::LlmFailure;

/// Durable payload recorded before one provider-routed model-request retry
/// wait.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum LlmRetryEventData {
    #[serde(rename = "normal")]
    Normal {
        #[serde(rename = "retryId")]
        retry_id: RetryId,
        turn: u64,
        step: u64,
        provider: String,
        #[serde(rename = "policyKey")]
        policy_key: String,
        retry: u64,
        #[serde(rename = "maxRetries")]
        max_retries: u64,
        #[serde(rename = "delayMs")]
        delay_ms: u64,
        failure: LlmFailure,
    },
    #[serde(rename = "always")]
    Always {
        #[serde(rename = "retryId")]
        retry_id: RetryId,
        turn: u64,
        step: u64,
        provider: String,
        #[serde(rename = "policyKey")]
        policy_key: String,
        retry: u64,
        #[serde(rename = "delayMs")]
        delay_ms: u64,
        failure: LlmFailure,
    },
}

impl LlmRetryEventData {
    pub fn retry_id(&self) -> &RetryId {
        match self {
            LlmRetryEventData::Normal { retry_id, .. } => retry_id,
            LlmRetryEventData::Always { retry_id, .. } => retry_id,
        }
    }

    pub fn retry(&self) -> u64 {
        match self {
            LlmRetryEventData::Normal { retry, .. } => *retry,
            LlmRetryEventData::Always { retry, .. } => *retry,
        }
    }
}

/// Durable transition recorded after one retry delay completes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRetryStartedEventData {
    pub retry_id: RetryId,
    pub turn: u64,
    pub step: u64,
    pub retry: u64,
}
