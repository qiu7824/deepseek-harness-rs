//! Typed failures shared by subagent service and provider operations. Rust
//! port of `packages/subagent/subagent/src/error.ts`.

/// Typed failure for the subagent seam (TS `SubagentError extends
/// HarnessError`; the Rust shape is the closed message + code pair).
#[derive(Debug, Clone)]
pub struct SubagentError {
    pub message: String,
    pub code: String,
}

impl SubagentError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
        }
    }
}

impl std::fmt::Display for SubagentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SubagentError {}
