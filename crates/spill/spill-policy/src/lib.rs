//! The spill-policy plugin. Rust port of `@deepseek-ai/dsh-spill-policy`.

pub mod index;
pub mod invariant;

pub use index::{Config, INJECT, NAME, SpillPolicyPlugin, apply};
