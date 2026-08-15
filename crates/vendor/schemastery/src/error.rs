//! Validation errors and Standard Schema V1 result types.

use std::fmt;

use crate::meta::{Options, PathSeg};

/// Error thrown when validation fails (port of the `ValidationError` class;
/// the message carries the `$...` path prefix, matching the TS ctor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
    pub path: Vec<PathSeg>,
}

impl ValidationError {
    pub fn new(message: impl Into<String>, options: &Options) -> Self {
        let path = options.path.clone();
        let mut prefix = "$".to_string();
        for segment in &path {
            prefix += &segment.format();
        }
        if prefix.starts_with('.') {
            prefix = prefix[1..].to_string();
        }
        let message = message.into();
        let message = if prefix == "$" {
            message
        } else {
            format!("{prefix} {message}")
        };
        Self { message, path }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ValidationError {}

/// One issue produced by a Standard Schema V1 validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub message: String,
    pub path: Vec<PathSeg>,
}

/// Result of a Standard Schema V1 `validate` call.
#[derive(Debug, Clone)]
pub enum StandardResult {
    Success { value: crate::data::Data },
    Failure { issues: Vec<Issue> },
}

impl StandardResult {
    /// Extract the validated value (TS `result.value`).
    pub fn value(self) -> Option<crate::data::Data> {
        match self {
            StandardResult::Success { value } => Some(value),
            StandardResult::Failure { .. } => None,
        }
    }

    /// Extract validation issues (TS `result.issues`).
    pub fn issues(self) -> Option<Vec<Issue>> {
        match self {
            StandardResult::Failure { issues } => Some(issues),
            StandardResult::Success { .. } => None,
        }
    }
}
