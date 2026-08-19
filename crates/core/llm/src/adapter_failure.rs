//! Normalization for values thrown by a final LLM adapter boundary. Rust
//! port of `packages/llm/llm/src/adapter-failure.ts`.
//!
//! # Deviation
//!
//! - Adapter failures cross the boundary as `String` errors (the port's
//!   uniform error channel); `normalize_llm_failure` validates and detaches
//!   the carried facts the same way, with the `UNKNOWN` code fallback.

use crate::types::LlmFailure;

/// Detach serializable provider facts from a value thrown by an adapter (TS
/// `normalizeLlmFailure`).
pub fn normalize_llm_failure(value: &str) -> LlmFailure {
    LlmFailure {
        message: if value.is_empty() {
            "LLM adapter failed".to_string()
        } else {
            value.to_string()
        },
        code: "UNKNOWN".to_string(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

/// The normalized failure for a value whose own `failure` payload was
/// validated and detached (kept for callers that already hold facts).
pub fn failure_snapshot(
    message: String,
    code: String,
    status: Option<u64>,
    provider_retry_after_ms: Option<u64>,
    request_id: Option<crate::brand::ProviderRequestId>,
) -> Option<LlmFailure> {
    if message.is_empty() || code.is_empty() {
        return None;
    }
    if status.is_some_and(|status| !(100..=599).contains(&status)) {
        return None;
    }
    if provider_retry_after_ms.is_some_and(|delay| delay == 0) {
        return None;
    }
    if request_id.as_ref().is_some_and(|id| id.as_str().is_empty()) {
        return None;
    }
    Some(LlmFailure {
        message,
        code,
        status,
        provider_retry_after_ms,
        request_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_thrown_messages() {
        let failure = normalize_llm_failure("busy");
        assert_eq!(failure.message, "busy");
        assert_eq!(failure.code, "UNKNOWN");
        let empty = normalize_llm_failure("");
        assert_eq!(empty.message, "LLM adapter failed");
    }

    #[test]
    fn snapshots_validated_facts() {
        let snapshot = failure_snapshot(
            "busy".to_string(),
            "RATE_LIMIT".to_string(),
            Some(429),
            None,
            None,
        )
        .expect("valid");
        assert_eq!(snapshot.status, Some(429));
        assert!(
            failure_snapshot("".to_string(), "RATE_LIMIT".to_string(), None, None, None).is_none()
        );
        assert!(
            failure_snapshot("busy".to_string(), "X".to_string(), Some(99), None, None).is_none()
        );
    }
}
