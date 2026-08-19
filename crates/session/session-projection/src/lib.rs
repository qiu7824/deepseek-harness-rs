//! Session-projection seam for the DeepSeek Harness. Rust port of
//! `@deepseek-ai/dsh-session-projection`.

pub mod index;
pub mod invariant;
pub mod types;

pub use index::{
    ProjectionChangeListener, ProjectionCheckpoint, ProjectionCheckpointRow, ProjectionDefinition,
    ProjectionSnapshot, SessionProjectionRegistry,
};
