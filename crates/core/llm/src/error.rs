//! Harness error base with a stable machine-routable code and chained cause.
//! Rust port of `packages/llm/llm/src/error.ts`.

use std::fmt;

/// Base class for all harness errors. Carries a `code` (stable,
/// programmatic — e.g. `NO_ADAPTER`, `INVALID_ARGS`, `INVARIANT`) distinct
/// from the human-readable `message`, and supports `cause` chaining.
#[derive(Debug)]
pub struct HarnessError {
    /// Stable machine-routable failure class.
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Chained cause, when present.
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl HarnessError {
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_cause(
        message: impl Into<String>,
        code: impl Into<String>,
        cause: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            cause: Some(cause),
        }
    }
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for HarnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Canonical provider-neutral code for a model request rejected because its
/// context window was exceeded.
pub const CONTEXT_WINDOW_EXCEEDED_CODE: &str = "CONTEXT_WINDOW_EXCEEDED";

/// Canonical provider-neutral code for an exhausted account quota or balance.
pub const QUOTA_EXCEEDED_CODE: &str = "QUOTA";

/// Canonical provider-neutral code for a response that completed normally but
/// carried no content blocks at all.
pub const EMPTY_RESPONSE_CODE: &str = "EMPTY_RESPONSE";

/// Canonical provider-neutral code for a credential that was supplied but
/// cannot be used — malformed rather than absent.
pub const INVALID_CREDENTIAL_CODE: &str = "INVALID_CREDENTIAL";

/// Match explicit model-context overflow facts rather than arbitrary invalid
/// input or configuration errors mentioning a context window.
pub fn is_context_window_exceeded_error(detail: &str) -> bool {
    static PATTERNS: std::sync::OnceLock<regex::RegexSet> = std::sync::OnceLock::new();
    PATTERNS.get_or_init(|| regex::RegexSet::new([
        r"(?i)(?:^|[^a-z0-9])context[\s_-](?:length|window)[\s_-](?:exceed(?:ed|s)?|overflow(?:ed)?|limit[\s_-]exceeded)(?:$|[^a-z0-9])",
        r"(?i)\b(?:maximum|max)(?:\s+(?:allowed|supported))?\s+context\s+(?:length|window)\b",
        r"(?i)\b(?:request|prompt|input|messages?)\s+(?:is\s+|are\s+)?too\s+(?:large|long)\s+for\s+(?:(?:this|the)\s+)?(?:model(?:'s)?\s+)?context(?:\s+window)?\b",
        r"(?i)\b(?:input|prompt|request)\s+(?:is\s+)?too\s+(?:long|large)\s+for\s+(?:this|the)\s+model\b",
        r"(?i)\b(?:input|prompt|request|messages?)\b.{0,40}\b(?:exceed(?:s|ed)?|overflows?|is\s+larger\s+than)\b.{0,40}\b(?:the\s+)?(?:model(?:'s)?\s+)?context(?:\s+(?:length|window))?\b",
    ]).expect("static context-overflow patterns")).is_match(detail)
}

#[cfg(test)]
mod context_overflow_tests {
    use super::is_context_window_exceeded_error;

    #[test]
    fn recognizes_explicit_provider_context_overflow() {
        for detail in [
            "context_length_exceeded",
            "context-window-overflowed",
            "This model maximum context length is 128000 tokens",
            "input is too long for this model",
            "request too large for model context",
            "input exceeds the model context window limit",
        ] {
            assert!(is_context_window_exceeded_error(detail), "{detail}");
        }
    }

    #[test]
    fn unrelated_invalid_requests_do_not_trigger_compaction() {
        for detail in [
            "invalid request: malformed tool arguments",
            "invalid input: temperature exceeds maximum allowed value",
            "input exceeds maximum allowed value",
            "context window size must be positive",
            "context length is not a supported parameter",
        ] {
            assert!(!is_context_window_exceeded_error(detail), "{detail}");
        }
    }
}

/// Render an error with its full `cause` chain: the outermost message first,
/// each cause appended with `: `.
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = Vec::new();
    let mut cursor: Option<&dyn std::error::Error> = Some(error);
    while let Some(current) = cursor {
        parts.push(current.to_string());
        cursor = current.source();
    }
    parts.join(": ")
}
