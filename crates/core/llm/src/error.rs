//! Harness error base with a stable machine-routable code and chained cause.
//! Rust port of `packages/llm/llm/src/error.ts` (error-chain rendering only;
//! classification regexes are exercised in later milestones).

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
