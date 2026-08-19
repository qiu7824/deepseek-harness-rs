//! Error vocabulary for the storage hub and its backends. Rust port of
//! `packages/storage/storage/src/error.ts`.

/// Discriminant codes carried by every [`StorageError`] (TS
/// `StorageErrorCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageErrorCode {
    BackendNotFound,
    FormNotMounted,
    DuplicateBackend,
    DuplicateMount,
    VersionMismatch,
    MalformedMedium,
    Closed,
}

impl StorageErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            StorageErrorCode::BackendNotFound => "backend-not-found",
            StorageErrorCode::FormNotMounted => "form-not-mounted",
            StorageErrorCode::DuplicateBackend => "duplicate-backend",
            StorageErrorCode::DuplicateMount => "duplicate-mount",
            StorageErrorCode::VersionMismatch => "version-mismatch",
            StorageErrorCode::MalformedMedium => "malformed-medium",
            StorageErrorCode::Closed => "closed",
        }
    }
}

/// Error thrown by the hub and by backend implementations (TS
/// `StorageError`).
#[derive(Debug, Clone, PartialEq)]
pub struct StorageError {
    pub code: StorageErrorCode,
    pub message: String,
}

impl StorageError {
    pub fn new(code: StorageErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for StorageError {}
