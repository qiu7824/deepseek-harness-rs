//! Provider-owned request-retry policy configuration and resolution. Rust
//! port of `packages/llm/llm/src/retry-policy.ts`.
//!
//! # Deviations
//!
//! - The schemastery `RetryPolicySchema` is replaced by a manual JSON
//!   validation in [`resolve_retry_policy`] (config arrives as
//!   `serde_json::Value`); the checks and error paths mirror the TS
//!   validation pair (schema + `resolveRetryPolicy`).

use dsh_timeout::MAX_TIMER_DELAY_MS;
use serde::{Deserialize, Serialize};

use crate::error::EMPTY_RESPONSE_CODE;

pub const DEFAULT_MAX_RETRIES: u64 = 2;
pub const DEFAULT_INITIAL_DELAY_MS: u64 = 500;
pub const DEFAULT_MAX_DELAY_MS: u64 = 10_000;
pub const DEFAULT_JITTER_RATIO: f64 = 0.1;

/// The default stable failure codes eligible for the normal policy.
pub const DEFAULT_RETRYABLE_CODES: [&str; 5] = [
    EMPTY_RESPONSE_CODE,
    "RATE_LIMIT",
    "SERVER",
    "TIMEOUT",
    "TRANSPORT",
];

/// Fully resolved backoff shared by both retry modes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRetryBackoff {
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub jitter_ratio: f64,
}

/// Fully resolved bounded transient retry policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedNormalRetryPolicy {
    pub mode: &'static str,
    pub max_retries: u64,
    pub retryable_codes: Vec<String>,
    #[serde(flatten)]
    pub backoff: ResolvedRetryBackoff,
}

/// Fully resolved unbounded retry policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAlwaysRetryPolicy {
    pub mode: &'static str,
    #[serde(flatten)]
    pub backoff: ResolvedRetryBackoff,
}

/// Immutable provider policy captured when its adapter route is registered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum ResolvedRetryPolicy {
    #[serde(rename = "normal")]
    Normal {
        #[serde(rename = "maxRetries")]
        max_retries: u64,
        #[serde(rename = "retryableCodes")]
        retryable_codes: Vec<String>,
        #[serde(flatten)]
        backoff: ResolvedRetryBackoff,
    },
    #[serde(rename = "always")]
    Always {
        #[serde(flatten)]
        backoff: ResolvedRetryBackoff,
    },
}

impl ResolvedRetryPolicy {
    pub fn mode(&self) -> &'static str {
        match self {
            ResolvedRetryPolicy::Normal { .. } => "normal",
            ResolvedRetryPolicy::Always { .. } => "always",
        }
    }

    pub fn backoff(&self) -> ResolvedRetryBackoff {
        match self {
            ResolvedRetryPolicy::Normal { backoff, .. } => *backoff,
            ResolvedRetryPolicy::Always { backoff } => *backoff,
        }
    }

    pub fn max_retries(&self) -> Option<u64> {
        match self {
            ResolvedRetryPolicy::Normal { max_retries, .. } => Some(*max_retries),
            ResolvedRetryPolicy::Always { .. } => None,
        }
    }

    pub fn retryable_codes(&self) -> Option<&[String]> {
        match self {
            ResolvedRetryPolicy::Normal { retryable_codes, .. } => Some(retryable_codes),
            ResolvedRetryPolicy::Always { .. } => None,
        }
    }
}

const NORMAL_POLICY_KEYS: [&str; 4] = ["mode", "maxRetries", "retryableCodes", "backoff"];
const ALWAYS_POLICY_KEYS: [&str; 2] = ["mode", "backoff"];
const BACKOFF_KEYS: [&str; 3] = ["initialDelayMs", "maxDelayMs", "jitterRatio"];

fn validate_keys(object: &serde_json::Map<String, serde_json::Value>, allowed: &[&str], path: &str) -> Result<(), String> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{path}: unknown key \"{key}\""));
        }
    }
    Ok(())
}

fn resolve_backoff(config: Option<&serde_json::Value>, path: &str) -> Result<ResolvedRetryBackoff, String> {
    if let Some(serde_json::Value::Object(config)) = config {
        validate_keys(config, &BACKOFF_KEYS, path)?;
    }
    let initial = config
        .and_then(|value| value.get("initialDelayMs"))
        .and_then(|value| value.as_u64())
        .unwrap_or(DEFAULT_INITIAL_DELAY_MS);
    let max = config
        .and_then(|value| value.get("maxDelayMs"))
        .and_then(|value| value.as_u64())
        .unwrap_or(DEFAULT_MAX_DELAY_MS);
    let jitter = config
        .and_then(|value| value.get("jitterRatio"))
        .and_then(|value| value.as_f64())
        .unwrap_or(DEFAULT_JITTER_RATIO);
    if initial == 0 || initial > MAX_TIMER_DELAY_MS {
        return Err(format!(
            "{path}.initialDelayMs must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
        ));
    }
    if max == 0 || max > MAX_TIMER_DELAY_MS {
        return Err(format!(
            "{path}.maxDelayMs must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
        ));
    }
    if initial > max {
        return Err(format!("{path}.initialDelayMs must be less than or equal to maxDelayMs"));
    }
    if !jitter.is_finite() || !(0.0..=1.0).contains(&jitter) {
        return Err(format!("{path}.jitterRatio must be between 0 and 1"));
    }
    Ok(ResolvedRetryBackoff {
        initial_delay_ms: initial,
        max_delay_ms: max,
        jitter_ratio: jitter,
    })
}

/// Validate, default, and detach one provider-owned retry policy (TS
/// `resolveRetryPolicy`).
pub fn resolve_retry_policy(
    config: Option<&serde_json::Value>,
    path: &str,
) -> Result<ResolvedRetryPolicy, String> {
    let Some(config) = config else {
        return Ok(ResolvedRetryPolicy::Normal {
            max_retries: DEFAULT_MAX_RETRIES,
            retryable_codes: DEFAULT_RETRYABLE_CODES.iter().map(|code| code.to_string()).collect(),
            backoff: resolve_backoff(None, &format!("{path}.backoff"))?,
        });
    };
    let serde_json::Value::Object(object) = config else {
        return Err(format!("{path} must be an object"));
    };
    let mode = object.get("mode").and_then(|value| value.as_str());
    match mode {
        Some("normal") => {
            validate_keys(object, &NORMAL_POLICY_KEYS, path)?;
            let max_retries = object
                .get("maxRetries")
                .and_then(|value| value.as_u64())
                .unwrap_or(DEFAULT_MAX_RETRIES);
            let retryable_codes: Vec<String> = object
                .get("retryableCodes")
                .and_then(|value| value.as_array())
                .map(|codes| {
                    codes
                        .iter()
                        .filter_map(|code| code.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_else(|| DEFAULT_RETRYABLE_CODES.iter().map(|code| code.to_string()).collect());
            if retryable_codes.is_empty() {
                return Err(format!("{path}.retryableCodes must not be empty"));
            }
            if retryable_codes.iter().any(|code| code.is_empty()) {
                return Err(format!("{path}.retryableCodes must contain only non-empty strings"));
            }
            if retryable_codes.iter().collect::<std::collections::HashSet<_>>().len() != retryable_codes.len() {
                return Err(format!("{path}.retryableCodes must not contain duplicates"));
            }
            Ok(ResolvedRetryPolicy::Normal {
                max_retries,
                retryable_codes,
                backoff: resolve_backoff(object.get("backoff"), &format!("{path}.backoff"))?,
            })
        }
        Some("always") => {
            validate_keys(object, &ALWAYS_POLICY_KEYS, path)?;
            Ok(ResolvedRetryPolicy::Always {
                backoff: resolve_backoff(object.get("backoff"), &format!("{path}.backoff"))?,
            })
        }
        _ => Err(format!("{path}.mode must be \"normal\" or \"always\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_normal_defaults() {
        let policy = resolve_retry_policy(None, "llm: provider \"p\" retryPolicy").unwrap();
        assert_eq!(policy.mode(), "normal");
        assert_eq!(policy.max_retries(), Some(2));
        assert_eq!(
            policy.retryable_codes().unwrap(),
            DEFAULT_RETRYABLE_CODES.iter().map(|code| code.to_string()).collect::<Vec<_>>()
        );
        assert_eq!(policy.backoff(), ResolvedRetryBackoff {
            initial_delay_ms: 500,
            max_delay_ms: 10_000,
            jitter_ratio: 0.1,
        });
    }

    #[test]
    fn resolves_explicit_policies_and_rejects_bad_ones() {
        let normal = resolve_retry_policy(
            Some(&serde_json::json!({
                "mode": "normal", "maxRetries": 5,
                "retryableCodes": ["SERVER", "RATE_LIMIT"],
                "backoff": {"initialDelayMs": 100, "maxDelayMs": 900, "jitterRatio": 0.25},
            })),
            "p",
        )
        .unwrap();
        assert_eq!(normal.max_retries(), Some(5));
        assert_eq!(normal.backoff().initial_delay_ms, 100);
        assert_eq!(normal.backoff().jitter_ratio, 0.25);

        let always = resolve_retry_policy(Some(&serde_json::json!({"mode": "always"})), "p").unwrap();
        assert_eq!(always.mode(), "always");

        assert!(resolve_retry_policy(Some(&serde_json::json!({"mode": "never"})), "p").is_err());
        assert!(resolve_retry_policy(Some(&serde_json::json!({"mode": "normal", "retryableCodes": []})), "p").is_err());
        assert!(resolve_retry_policy(Some(&serde_json::json!({"mode": "normal", "retryableCodes": ["a", "a"]})), "p").is_err());
        assert!(resolve_retry_policy(
            Some(&serde_json::json!({"mode": "normal", "backoff": {"initialDelayMs": 900, "maxDelayMs": 100}})),
            "p",
        )
        .is_err());
        assert!(resolve_retry_policy(
            Some(&serde_json::json!({"mode": "normal", "backoff": {"jitterRatio": 2}})),
            "p",
        )
        .is_err());
        assert!(resolve_retry_policy(Some(&serde_json::json!({"mode": "normal", "extra": 1})), "p").is_err());
    }

    #[test]
    fn wire_shapes_round_trip() {
        let policy = resolve_retry_policy(None, "p").unwrap();
        let json = serde_json::to_value(&policy).unwrap();
        assert_eq!(json.get("mode").and_then(|v| v.as_str()), Some("normal"));
        assert_eq!(json.get("maxRetries").and_then(|v| v.as_u64()), Some(2));
        let decoded: ResolvedRetryPolicy = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, policy);
    }
}
