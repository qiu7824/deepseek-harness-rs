//! Pure types of the session-stats domain (TS `types.ts`). The TS file also
//! declaration-merges `sessionStats` into the projection table; the Rust
//! projection table is open by key, so only the value type is declared here.

use serde::{Deserialize, Serialize};

/// Whole-log conversation figures, independent of how much history a client
/// has paged in. Every field is 0 until its first contributing event lands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatsProjection {
    /// Distinct turns carrying at least one closed step (`step/end`).
    pub turns: u64,
    /// Closed steps (`step/end` events) — completed, failed, and cancelled
    /// alike.
    pub steps: u64,
    /// Summed model wall time (`step/start` → `assistant/message`) over
    /// steps that assembled a message, ms.
    pub llm_ms: u64,
    /// Summed tool wall time over `tool/call` → `tool/result` pairs matched
    /// by callId, ms.
    pub tool_ms: u64,
    /// Summed first-token latency over `ttftSteps`, ms.
    pub ttft_ms: u64,
    /// Steps carrying a recorded first token.
    pub ttft_steps: u64,
    /// Summed decode wall time over usage-reporting steps, ms.
    pub decode_ms: u64,
    /// Summed provider output tokens over the same steps.
    pub decode_tokens: u64,
}

impl SessionStatsProjection {
    /// The all-zero projection value (TS test helper `totals()`).
    pub fn zero() -> Self {
        Self {
            turns: 0,
            steps: 0,
            llm_ms: 0,
            tool_ms: 0,
            ttft_ms: 0,
            ttft_steps: 0,
            decode_ms: 0,
            decode_tokens: 0,
        }
    }
}

impl SessionStatsProjection {
    /// Deserialize the wire value; camelCase field names (TS wire shape).
    pub fn from_wire(value: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())
    }
}
