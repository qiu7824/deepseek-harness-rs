//! Local durable attachment backend rooted below `DSH_HOME`. Rust port of
//! `packages/attachment/attachment-local/src/index.ts`.

mod index;
mod image;
pub mod invariant;
pub mod store;

pub use index::*;
