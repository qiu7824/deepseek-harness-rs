//! Framework error types (port of `fiber.ts` error definitions).

use std::sync::Arc;
use thiserror::Error;

use crate::util::{ArcValue, arc, error_message};

/// Framework error with a stable machine-readable code.
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct CordisError {
    pub code: CordisErrorCode,
    pub message: String,
}

/// Cordis error codes (port of `CordisError.Code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CordisErrorCode {
    /// Raised when an effect is created on a disposed or unloading fiber.
    InactiveEffect,
}

impl CordisErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InactiveEffect => "INACTIVE_EFFECT",
        }
    }
}

impl CordisError {
    pub fn new(code: CordisErrorCode) -> Self {
        let message = match code {
            CordisErrorCode::InactiveEffect => "cannot create effect on inactive context",
        };
        Self {
            code,
            message: message.to_string(),
        }
    }
}

/// Error raised when plugin configuration fails schema validation
/// (port of `ValidationError`, aggregated from schema issues).
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub issues: Vec<String>,
}

impl ValidationError {
    pub fn new(issues: impl IntoIterator<Item = String>) -> Self {
        Self {
            issues: issues.into_iter().collect(),
        }
    }

    fn fmt_issues(&self) -> String {
        self.issues
            .iter()
            .map(|i| format!("  - {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid config:\n{}", self.fmt_issues())
    }
}

impl std::error::Error for ValidationError {}

/// Aggregated error thrown by `parallel` dispatch when listeners fail
/// (port of `AggregateError`).
#[derive(Debug, Error)]
#[error("{count} listener(s) failed during parallel dispatch")]
pub struct AggregateError {
    pub reasons: Vec<ArcValue>,
    count: usize,
}

impl AggregateError {
    pub fn new(reasons: Vec<ArcValue>) -> Self {
        Self {
            count: reasons.len(),
            reasons,
        }
    }
}

/// Normalized plugin startup/validation failure carried by a fiber.
#[derive(Debug, Clone)]
pub struct PluginError {
    pub value: ArcValue,
}

impl PluginError {
    pub fn new(value: ArcValue) -> Self {
        Self { value }
    }

    pub fn message(&self) -> String {
        error_message(&self.value)
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for PluginError {}

impl From<anyhow::Error> for PluginError {
    fn from(err: anyhow::Error) -> Self {
        Self::new(Arc::new(err))
    }
}

impl From<CordisError> for PluginError {
    fn from(error: CordisError) -> Self {
        Self::new(arc(error))
    }
}

impl From<ValidationError> for PluginError {
    fn from(error: ValidationError) -> Self {
        Self::new(arc(error))
    }
}
