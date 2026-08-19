//! Spill storage Service Definition (`ctx.spillStore`). Rust port of
//! `@deepseek-ai/dsh-spill`.

pub mod index;
pub mod invariant;
pub mod types;

pub use index::SpillStore;
pub use types::{
    SaveTextSpill, SpillLocator, SpillLocatorTag, SpillOwner, SpillRef, SpillSource, spill_locator,
};
