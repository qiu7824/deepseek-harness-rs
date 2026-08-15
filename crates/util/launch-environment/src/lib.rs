//! Immutable launch-time environment snapshot with per-layer provenance.
//! Rust port of `@deepseek-ai/dsh-launch-environment`.

pub mod index;
pub mod invariant;

pub use index::{
    DSH_LAUNCH_ENVIRONMENT_KEY, LaunchEnvironmentEntry, LaunchEnvironmentLayerInput,
    LaunchEnvironmentSnapshot, LaunchEnvironmentSource, create_launch_environment_snapshot,
    launch_environment_of,
};
