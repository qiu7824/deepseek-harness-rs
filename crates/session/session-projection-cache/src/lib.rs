//! Persisted projection cache (`ctx.sessionProjectionCache`). Rust port of
//! `@deepseek-ai/dsh-session-projection-cache`.

pub mod index;
pub mod spec;

pub use index::{
    Config, INJECT, NAME, PACKAGE_NAME, SessionProjectionCache, SessionProjectionCachePlugin,
    identity_matches, identity_of,
};
pub use spec::{
    CheckpointIdentity, CheckpointRecord, CheckpointRow, checkpoint_record_schema,
    projection_cache_domain_spec,
};
