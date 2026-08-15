//! Conversation call configuration and freeze utilities.
//! Rust port of `packages/llm/llm/src/call-config.ts`.

use crate::types::{GenerateOptions, LlmCallConfig, LlmCallConfigAdapterDefaults};

/// Field-wise equality over [`LlmCallConfig`] — the comparison a caller
/// runs to decide whether a proposed configuration is a real change (worth a
/// logged header snapshot) or the held one restated.
pub fn call_config_equals(a: &LlmCallConfig, b: &LlmCallConfig) -> bool {
    if a.provider != b.provider
        || a.model != b.model
        || a.reasoning_effort != b.reasoning_effort
        || a.temperature != b.temperature
        || a.max_tokens != b.max_tokens
    {
        return false;
    }
    match (&a.stop, &b.stop) {
        (None, None) => true,
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// Deep-freeze a value in place. Rust values are owned and cannot be
/// mutated through shared references, so this is the identity function —
/// the TS `deepFreeze` contract is the type system's default.
pub fn deep_freeze<T>(value: T) -> T {
    value
}

/// Mark one exact request object as assembled by dsh-agent-loop (TS
/// `markAgentLoopRequest`).
///
/// # Deviation
///
/// The TS `WeakSet` identity registry collapses to an explicit flag on the
/// request value: `GenerateOptions` is an owned Rust struct without object
/// identity, so membership rides the value itself.
pub fn mark_agent_loop_request(options: &mut GenerateOptions) {
    options.agent_loop_request = true;
}

/// Whether the request object was assembled by dsh-agent-loop (TS
/// `isAgentLoopRequest`).
pub fn is_agent_loop_request(options: &GenerateOptions) -> bool {
    options.agent_loop_request
}

/// Field-wise equality helper for adapter-default markers (each present
/// marker must be `true`).
pub fn adapter_defaults_equals(
    a: Option<&LlmCallConfigAdapterDefaults>,
    b: Option<&LlmCallConfigAdapterDefaults>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.reasoning_effort == right.reasoning_effort && left.max_tokens == right.max_tokens
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_equality_is_fieldwise() {
        let base = LlmCallConfig {
            provider: "deepseek".to_string(),
            model: "deepseek-chat".to_string(),
            reasoning_effort: None,
            temperature: None,
            max_tokens: None,
            stop: Some(vec!["a".to_string(), "b".to_string()]),
        };
        let same = base.clone();
        assert!(call_config_equals(&base, &same));

        let mut different_model = base.clone();
        different_model.model = "other".to_string();
        assert!(!call_config_equals(&base, &different_model));

        let mut different_stop = base.clone();
        different_stop.stop = Some(vec!["a".to_string()]);
        assert!(!call_config_equals(&base, &different_stop));

        let mut no_stop = base.clone();
        no_stop.stop = None;
        assert!(!call_config_equals(&base, &no_stop));

        let mut temperature = base.clone();
        temperature.temperature = Some(0.5);
        assert!(!call_config_equals(&base, &temperature));
    }
}
