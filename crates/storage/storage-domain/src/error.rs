//! Error vocabulary of the domain data form. Rust port of
//! `packages/storage/storage-domain/src/error.ts`.

/// Discriminant codes carried by every [`DomainError`] (TS
/// `DomainErrorCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainErrorCode {
    AlreadyOpen,
    FacetUnsupported,
    InvalidRecord,
    MissingKey,
    Closed,
}

impl DomainErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DomainErrorCode::AlreadyOpen => "already-open",
            DomainErrorCode::FacetUnsupported => "facet-unsupported",
            DomainErrorCode::InvalidRecord => "invalid-record",
            DomainErrorCode::MissingKey => "missing-key",
            DomainErrorCode::Closed => "closed",
        }
    }
}

/// Error thrown by the domain layer (TS `DomainError`). Backend failures
/// (`backend-not-found`, `version-mismatch`, …) pass through as plain
/// strings — the domain layer does not rewrap them.
#[derive(Debug, Clone, PartialEq)]
pub struct DomainError {
    pub code: DomainErrorCode,
    pub message: String,
    /// Present exactly when `code` is `InvalidRecord` (TS `detail`).
    pub detail: Option<(String, String)>,
}

impl DomainError {
    pub fn new(code: DomainErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), detail: None }
    }

    pub fn invalid_record(message: impl Into<String>, table: &str, key: &str) -> Self {
        Self {
            code: DomainErrorCode::InvalidRecord,
            message: message.into(),
            detail: Some((table.to_string(), key.to_string())),
        }
    }
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DomainError {}
