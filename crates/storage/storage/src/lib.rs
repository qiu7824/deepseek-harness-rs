//! Storage hub (`ctx.storage`). Rust port of `@deepseek-ai/dsh-storage`.

pub mod backend;
pub mod error;
pub mod index;
pub mod invariant;
pub mod registry;

pub use backend::{
    KvFacet, KvUnit, KvUnitDescriptor, KvUnitSnapshot, StorageBackend, closed_error,
    unit_name_matches, version_mismatch_error,
};
pub use error::{StorageError, StorageErrorCode};
pub use index::{
    INVARIANT_INJECT, INVARIANT_NAME, PACKAGE_NAME, Storage, storage_backend_service_key,
};
pub use registry::BackendRegistry;
