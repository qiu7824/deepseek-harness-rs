//! Event-only filesystem observation policy. Rust port of
//! `@deepseek-ai/dsh-fs-observation-policy`.

pub mod index;
pub mod invariant;

pub use index::{FsObservationActorHandle, NAME, ObservedStateGate, OwnerKey, apply};
